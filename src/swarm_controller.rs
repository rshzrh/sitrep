use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

use crate::model::{
    SwarmMode, SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo,
    SwarmTaskInfo, SwarmStackInfo, SwarmUIState, SwarmViewLevel,
    ServiceLogState,
};
use crate::swarm;
use crate::swarm::LogStreamHandle;

/// Manages Docker Swarm data collection, state, and actions.
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
    pub docker_cli_available: bool,
    /// Receiver for background action results (rolling restart, scale).
    action_receiver: Option<mpsc::Receiver<Result<String, String>>>,
    /// True while a background action is in flight.
    pub action_in_progress: bool,
}

impl SwarmMonitor {
    pub fn new() -> Self {
        let docker_cli_available = swarm::is_docker_cli_available();
        let cluster_info = if docker_cli_available {
            swarm::detect_swarm()
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
            docker_cli_available,
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
            docker_cli_available: false,
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
        self.docker_cli_available = swarm::is_docker_cli_available();
        if !self.docker_cli_available {
            return;
        }
        self.cluster_info = swarm::detect_swarm();
        if self.cluster_info.is_some() {
            self.mode = SwarmMode::Swarm;
        }
    }

    /// Refresh cluster data. Called on the tick interval only when Swarm tab is active.
    pub fn update(&mut self) {
        if !self.is_swarm() {
            return;
        }

        match swarm::list_nodes() {
            Ok(mut nodes) => {
                let ips = swarm::batch_get_node_ips(&nodes);
                for node in &mut nodes {
                    if let Some(ip) = ips.get(&node.id) {
                        node.ip_address = ip.clone();
                    }
                }
                self.nodes = nodes;
            }
            Err(e) => {
                self.status_message = Some(format!("Error: {}", e));
            }
        }

        match swarm::list_services() {
            Ok(services) => {
                self.services = services;
                self.build_stacks();
            }
            Err(e) => {
                self.status_message = Some(format!("Error: {}", e));
            }
        }

        // Refresh tasks if we're in task view
        if let SwarmViewLevel::ServiceTasks(ref svc_id, _) = self.ui_state.view_level {
            match swarm::list_service_tasks(svc_id) {
                Ok(tasks) => self.tasks = tasks,
                Err(e) => {
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
                match swarm::list_tasks_for_services(&id_refs) {
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
        use std::collections::HashMap;
        let mut stack_map: HashMap<String, Vec<usize>> = HashMap::new();

        for (i, svc) in self.services.iter().enumerate() {
            let stack_name = if svc.stack.is_empty() {
                "(no stack)".to_string()
            } else {
                svc.stack.clone()
            };
            stack_map.entry(stack_name).or_default().push(i);
        }

        let mut stacks: Vec<SwarmStackInfo> = stack_map
            .into_iter()
            .map(|(name, indices)| SwarmStackInfo { name, service_indices: indices })
            .collect();

        // Sort: named stacks first, then "(no stack)" last
        stacks.sort_by(|a, b| {
            if a.name == "(no stack)" {
                std::cmp::Ordering::Greater
            } else if b.name == "(no stack)" {
                std::cmp::Ordering::Less
            } else {
                a.name.cmp(&b.name)
            }
        });

        self.stacks = stacks;
    }

    /// Generate smart warnings about cluster health.
    fn generate_warnings(&mut self) {
        self.warnings.clear();

        if !self.docker_cli_available {
            self.warnings.push("docker CLI not found in PATH — Swarm data unavailable".to_string());
            return;
        }

        // Check for down nodes
        let down_nodes: Vec<&str> = self.nodes.iter()
            .filter(|n| n.status.to_lowercase().contains("down"))
            .map(|n| n.hostname.as_str())
            .collect();
        if !down_nodes.is_empty() {
            self.warnings.push(format!(
                "NODE DOWN: {} node(s) unreachable: {}",
                down_nodes.len(),
                down_nodes.join(", ")
            ));
        }

        // Check for drained nodes
        let drain_nodes: Vec<&str> = self.nodes.iter()
            .filter(|n| n.availability.to_lowercase().contains("drain"))
            .map(|n| n.hostname.as_str())
            .collect();
        if !drain_nodes.is_empty() {
            self.warnings.push(format!(
                "DRAINED: {} node(s) in drain mode: {}",
                drain_nodes.len(),
                drain_nodes.join(", ")
            ));
        }

        // Check for services with incomplete replicas
        for svc in &self.services {
            if svc.replicas.contains('/') {
                let parts: Vec<&str> = svc.replicas.split('/').collect();
                if parts.len() == 2 {
                    let current: u32 = parts[0].trim().parse().unwrap_or(0);
                    let desired: u32 = parts[1].trim().parse().unwrap_or(0);
                    if desired > 0 && current < desired {
                        self.warnings.push(format!(
                            "SERVICE DEGRADED: {} has {}/{} replicas",
                            svc.name, current, desired
                        ));
                    }
                }
            }
        }

        // Check manager count
        if let Some(ref info) = self.cluster_info {
            if info.managers < 3 && info.nodes_total > 3 {
                self.warnings.push(format!(
                    "LOW MANAGERS: Only {} manager(s) for {} nodes (recommend 3+)",
                    info.managers, info.nodes_total
                ));
            }
        }
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
        match swarm::list_service_tasks(service_id) {
            Ok(tasks) => self.tasks = tasks,
            Err(e) => {
                self.tasks.clear();
                self.status_message = Some(format!("Error: {}", e));
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

        let handle = swarm::tail_service_logs(service_id);
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
        let Some(ref handle) = self.log_handle else { return };
        let Some(ref mut log_state) = self.log_state else { return };

        for _ in 0..200 {
            match handle.receiver.try_recv() {
                Ok(line) => {
                    log_state.push_line(line);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
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

        thread::spawn(move || {
            let result = match swarm::force_update_service(&id) {
                Ok(()) => Ok(format!("Rolling restart initiated for {}", id)),
                Err(e) => Err(format!("Error: {}", e.trim())),
            };
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

        thread::spawn(move || {
            let result = match swarm::scale_service(&id, replicas) {
                Ok(()) => Ok(format!("Scaled {} to {} replicas", id, replicas)),
                Err(e) => Err(format!("Error: {}", e.trim())),
            };
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
