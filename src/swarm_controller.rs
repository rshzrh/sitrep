use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use bollard::Docker;

use crate::model::{
    SwarmMode, SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo,
    SwarmTaskInfo, SwarmStackInfo, SwarmUIState, SwarmViewLevel,
    ServiceLogState,
};
use crate::swarm;
use crate::swarm::LogStreamHandle;

/// Manages Docker Swarm data collection, state, and actions.
///
/// As of the A4 refactor (eng review block 4), all swarm reads go
/// through bollard instead of shelling out to the `docker` CLI. The
/// bollard client is `async`, so each public sync method here calls
/// `rt.block_on(swarm::some_async_fn(&client, ...))`. The client
/// is cached for the life of the monitor; if it fails to connect
/// at construction time we fall back to standalone mode and report
/// the error via `status_message`.
pub struct SwarmMonitor {
    pub mode: SwarmMode,
    pub cluster_info: Option<SwarmClusterInfo>,
    pub nodes: Vec<SwarmNodeInfo>,
    pub services: Vec<SwarmServiceInfo>,
    pub stacks: Vec<SwarmStackInfo>,
    pub tasks: Vec<SwarmTaskInfo>,
    /// Per-service running tasks, keyed by service ID (for inline replica sub-rows).
    pub service_tasks: HashMap<String, Vec<SwarmTaskInfo>>,
    pub ui_state: SwarmUIState,
    pub log_state: Option<ServiceLogState>,
    log_handle: Option<LogStreamHandle>,
    pub status_message: Option<String>,
    pub warnings: Vec<String>,
    /// Tokio runtime shared with DockerMonitor / App. Used to
    /// `block_on` the async bollard calls from the sync tick loop.
    rt: Arc<tokio::runtime::Runtime>,
    /// Cached bollard client. `None` if the daemon isn't reachable.
    client: Option<Docker>,
    /// Receiver for background action results (rolling restart, scale).
    action_receiver: Option<mpsc::Receiver<Result<String, String>>>,
    /// True while a background action is in flight.
    pub action_in_progress: bool,
}

impl SwarmMonitor {
    pub fn new(rt: Arc<tokio::runtime::Runtime>) -> Self {
        // Try to connect bollard to the local daemon. If that fails,
        // the caller sees `mode == Standalone` and `client == None`
        // and all swarm calls will short-circuit.
        let client = Docker::connect_with_local_defaults().ok();
        let reachable = client
            .as_ref()
            .map(|c| rt.block_on(swarm::is_docker_available(c)))
            .unwrap_or(false);

        let cluster_info = if reachable {
            if let Some(ref c) = client {
                rt.block_on(swarm::detect_swarm(c))
            } else {
                None
            }
        } else {
            None
        };
        let mode = if cluster_info.is_some() {
            SwarmMode::Swarm
        } else {
            SwarmMode::Standalone
        };

        Self {
            mode,
            cluster_info,
            nodes: Vec::new(),
            services: Vec::new(),
            stacks: Vec::new(),
            tasks: Vec::new(),
            service_tasks: HashMap::new(),
            ui_state: SwarmUIState::default(),
            log_state: None,
            log_handle: None,
            status_message: None,
            warnings: Vec::new(),
            rt,
            client: if reachable { client } else { None },
            action_receiver: None,
            action_in_progress: false,
        }
    }

    /// Build a minimal SwarmMonitor for unit testing (no I/O).
    #[cfg(test)]
    pub fn new_minimal(
        nodes: Vec<SwarmNodeInfo>,
        services: Vec<SwarmServiceInfo>,
        stacks: Vec<SwarmStackInfo>,
        expanded_ids: std::collections::HashSet<String>,
    ) -> Self {
        let mut ui_state = SwarmUIState::default();
        ui_state.expanded_ids = expanded_ids;
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        Self {
            mode: SwarmMode::Swarm,
            cluster_info: None,
            nodes,
            services,
            stacks,
            tasks: Vec::new(),
            service_tasks: HashMap::new(),
            ui_state,
            log_state: None,
            log_handle: None,
            status_message: None,
            warnings: Vec::new(),
            rt,
            client: None,
            action_receiver: None,
            action_in_progress: false,
        }
    }

    pub fn is_swarm(&self) -> bool {
        self.mode == SwarmMode::Swarm
    }

    /// Recheck swarm mode (called infrequently, e.g. every 30s, when standalone)
    pub fn recheck_swarm(&mut self) {
        if self.is_swarm() {
            return;
        }
        // Re-attempt connection if we don't have a client yet. The
        // daemon may have been started after flotop launched.
        if self.client.is_none() {
            self.client = Docker::connect_with_local_defaults().ok();
        }
        let Some(ref client) = self.client else {
            return;
        };
        if !self.rt.block_on(swarm::is_docker_available(client)) {
            return;
        }
        self.cluster_info = self.rt.block_on(swarm::detect_swarm(client));
        if self.cluster_info.is_some() {
            self.mode = SwarmMode::Swarm;
        }
    }

    /// Refresh cluster data. Called on the tick interval only when Swarm tab is active.
    pub fn update(&mut self) {
        if !self.is_swarm() {
            return;
        }
        // `Docker` is Clone (it's an Arc around the inner HTTP client),
        // so cloning here gives us a self-contained handle that
        // doesn't hold a borrow on `self`. Cheap.
        let Some(client) = self.client.clone() else {
            return;
        };

        // Snapshot the manager node id so we can re-compute the
        // `is_self` flag after each nodes fetch (bollard doesn't
        // expose it per-node).
        let self_node_id = self
            .cluster_info
            .as_ref()
            .map(|ci| ci.node_id.clone())
            .unwrap_or_default();

        match self.rt.block_on(swarm::list_nodes(&client)) {
            Ok(mut nodes) => {
                if !self_node_id.is_empty() {
                    for n in &mut nodes {
                        if n.id == self_node_id {
                            n.is_self = true;
                        }
                    }
                }
                self.nodes = nodes;
            }
            Err(e) => {
                tracing::warn!("Swarm node list failed: {}", e);
                self.status_message = Some(format!("Error: {}", e));
            }
        }

        match self.rt.block_on(swarm::list_services(&client)) {
            Ok(services) => {
                self.services = services;
                self.build_stacks();
            }
            Err(e) => {
                tracing::warn!("Swarm service list failed: {}", e);
                self.status_message = Some(format!("Error: {}", e));
            }
        }

        // Refresh tasks if we're in task view
        if let SwarmViewLevel::ServiceTasks(ref svc_id, _) = self.ui_state.view_level {
            match self.rt.block_on(swarm::list_service_tasks(&client, svc_id)) {
                Ok(tasks) => self.tasks = tasks,
                Err(e) => {
                    tracing::warn!("Swarm task list failed: {}", e);
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }

        // Fetch running tasks for services in expanded stacks (for inline replica sub-rows)
        self.service_tasks.clear();
        if !self.ui_state.expanded_ids.is_empty() {
            // Collect service IDs from expanded stacks
            let mut svc_ids: Vec<String> = Vec::new();
            for stack in &self.stacks {
                if self.ui_state.expanded_ids.contains(&stack.name) {
                    for &idx in &stack.service_indices {
                        if let Some(svc) = self.services.get(idx) {
                            svc_ids.push(svc.id.clone());
                        }
                    }
                }
            }

            if !svc_ids.is_empty() {
                let id_refs: Vec<&str> = svc_ids.iter().map(|s| s.as_str()).collect();
                match self.rt.block_on(swarm::list_tasks_for_services(&client, &id_refs)) {
                    Ok(tasks) => {
                        // Build a name->id lookup from services
                        let name_to_id: HashMap<String, String> = self.services.iter()
                            .map(|s| (s.name.clone(), s.id.clone()))
                            .collect();

                        // Group tasks by service ID
                        for task in tasks {
                            // Task name is e.g. "stack_service.1", extract service name
                            // by stripping the last ".N" suffix
                            let svc_name = if let Some(dot_pos) = task.name.rfind('.') {
                                &task.name[..dot_pos]
                            } else {
                                &task.name
                            };
                            if let Some(svc_id) = name_to_id.get(svc_name) {
                                self.service_tasks
                                    .entry(svc_id.clone())
                                    .or_insert_with(Vec::new)
                                    .push(task);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Swarm task fetch failed: {}", e);
                        self.status_message = Some(format!("Task fetch error: {}", e));
                    }
                }
            }
        }

        // Generate warnings
        self.generate_warnings();
    }

    /// Build stack groupings from services.
    /// Uses indices into self.services to avoid cloning service data.
    fn build_stacks(&mut self) {
        self.stacks = crate::swarm_helpers::build_stacks(&self.services);
    }

    /// Generate smart warnings about cluster health. Delegates the pure
    /// computation to `swarm_helpers` so both the local and remote paths
    /// get the same warnings.
    fn generate_warnings(&mut self) {
        self.warnings.clear();
        if self.client.is_none() {
            self.warnings
                .push("docker daemon not reachable — Swarm data unavailable".to_string());
            return;
        }
        self.warnings = crate::swarm_helpers::compute_warnings(
            &self.nodes,
            &self.services,
            self.cluster_info.as_ref(),
        );
    }

    /// Get the total number of selectable rows in the current overview.
    pub fn overview_row_count(&self) -> usize {
        let mut count = 0;
        // Nodes section header + nodes
        count += 1; // "Nodes" header
        if self.ui_state.expanded_ids.contains("__nodes__") {
            count += self.nodes.len();
        }
        // Each stack header + services
        for stack in &self.stacks {
            count += 1; // stack header
            if self.ui_state.expanded_ids.contains(&stack.name) {
                count += stack.service_indices.len();
            }
        }
        count
    }

    /// Enter task view for a specific service.
    pub fn enter_task_view(&mut self, service_id: &str, service_name: &str) {
        if let Some(ref client) = self.client {
            match self.rt.block_on(swarm::list_service_tasks(client, service_id)) {
                Ok(tasks) => self.tasks = tasks,
                Err(e) => {
                    self.tasks.clear();
                    self.status_message = Some(format!("Error: {}", e));
                }
            }
        }
        self.ui_state.view_level = SwarmViewLevel::ServiceTasks(
            service_id.to_string(),
            service_name.to_string(),
        );
        self.ui_state.selected_index = 0;
    }

    /// Start streaming logs for a service.
    pub fn start_service_log_stream(&mut self, service_id: &str, service_name: &str) {
        // Kill any existing log stream first
        self.stop_log_stream();

        let Some(ref client) = self.client else { return };
        let handle = swarm::tail_service_logs(client, self.rt.handle(), service_id);
        self.log_state = Some(ServiceLogState::new(
            service_id.to_string(),
            service_name.to_string(),
        ));
        self.log_handle = Some(handle);
        self.ui_state.view_level = SwarmViewLevel::ServiceLogs(
            service_id.to_string(),
            service_name.to_string(),
        );
    }

    /// Stop the log stream and kill the child process to avoid zombies.
    pub fn stop_log_stream(&mut self) {
        if let Some(ref handle) = self.log_handle {
            handle.kill();
        }
        self.log_handle = None;
        self.log_state = None;
    }

    /// Drain pending log lines from the channel.
    pub fn poll_logs(&mut self) {
        let Some(ref mut handle) = self.log_handle else { return };
        let Some(ref mut log_state) = self.log_state else { return };

        use tokio::sync::mpsc::error::TryRecvError;
        for _ in 0..200 {
            match handle.receiver.try_recv() {
                Ok(line) => {
                    log_state.push_line(line);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    log_state.push_line("[log stream ended]".to_string());
                    break;
                }
            }
        }
    }

    /// Force-update (rolling restart) a service in a background thread.
    /// Shows "in progress" status until the operation completes.
    pub fn force_restart_service(&mut self, service_id: &str) {
        if self.action_in_progress {
            self.status_message = Some("An action is already in progress...".to_string());
            return;
        }

        let id = service_id.to_string();
        let (tx, rx) = mpsc::channel();
        self.action_receiver = Some(rx);
        self.action_in_progress = true;
        self.status_message = Some(format!("Rolling restart in progress for {}...", service_id));

        // Spawn a fresh bollard client on the worker thread — Docker
        // clients are cheap to create (just an Arc around the HTTP
        // client) and moving the cached one across threads is fine
        // but less self-contained.
        let rt = Arc::clone(&self.rt);
        thread::spawn(move || {
            let result = rt.block_on(async {
                let client = match Docker::connect_with_local_defaults() {
                    Ok(c) => c,
                    Err(e) => return Err(format!("Error: {}", e)),
                };
                match swarm::force_update_service(&client, &id).await {
                    Ok(()) => Ok(format!("Rolling restart initiated for {}", id)),
                    Err(e) => Err(format!("Error: {}", e.trim())),
                }
            });
            let _ = tx.send(result);
        });
    }

    /// Scale a service in a background thread.
    pub fn scale_service(&mut self, service_id: &str, replicas: u32) {
        if self.action_in_progress {
            self.status_message = Some("An action is already in progress...".to_string());
            return;
        }

        let id = service_id.to_string();
        let (tx, rx) = mpsc::channel();
        self.action_receiver = Some(rx);
        self.action_in_progress = true;
        self.status_message = Some(format!("Scaling {} to {} replicas...", service_id, replicas));

        let rt = Arc::clone(&self.rt);
        thread::spawn(move || {
            let result = rt.block_on(async {
                let client = match Docker::connect_with_local_defaults() {
                    Ok(c) => c,
                    Err(e) => return Err(format!("Error: {}", e)),
                };
                match swarm::scale_service(&client, &id, replicas).await {
                    Ok(()) => Ok(format!("Scaled {} to {} replicas", id, replicas)),
                    Err(e) => Err(format!("Error: {}", e.trim())),
                }
            });
            let _ = tx.send(result);
        });
    }

    /// Poll for background action completion. Returns true if the status changed.
    pub fn poll_action(&mut self) -> bool {
        let Some(ref rx) = self.action_receiver else { return false };
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                self.status_message = Some(msg);
                self.action_in_progress = false;
                self.action_receiver = None;
                true
            }
            Ok(Err(msg)) => {
                tracing::error!("Swarm action failed: {}", msg);
                self.status_message = Some(msg);
                self.action_in_progress = false;
                self.action_receiver = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status_message = Some("Action failed unexpectedly".to_string());
                self.action_in_progress = false;
                self.action_receiver = None;
                true
            }
        }
    }

    /// Go back one level in the view hierarchy.
    pub fn go_back(&mut self) {
        match &self.ui_state.view_level {
            SwarmViewLevel::ServiceLogs(_, _) => {
                self.stop_log_stream();
                // Return to tasks or overview depending on context
                self.ui_state.view_level = SwarmViewLevel::Overview;
                self.ui_state.selected_index = 0;
            }
            SwarmViewLevel::ServiceTasks(_, _) => {
                self.tasks.clear();
                self.ui_state.view_level = SwarmViewLevel::Overview;
                self.ui_state.selected_index = 0;
            }
            SwarmViewLevel::Overview => {
                // Already at top level
            }
        }
    }
}

impl crate::controller::DataMonitor for SwarmMonitor {
    fn update(&mut self) {
        SwarmMonitor::update(self);
    }

    fn poll_update(&mut self) -> bool {
        // SwarmMonitor::update is synchronous — data is ready when
        // update() returns, so there's nothing async to drain.
        false
    }

    fn is_available(&self) -> bool {
        self.is_swarm()
    }
    // set_active: trait default.
}
