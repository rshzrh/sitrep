use tokio::sync::mpsc;
use std::sync::Arc;
use std::collections::HashMap;

use crate::docker::DockerClient;
use crate::model::{
    ContainerUIState, DockerContainerInfo, LogViewState, MultiLogLine, MultiLogViewState,
};

/// Receiver for background Docker action results.
type ActionReceiver = std::sync::mpsc::Receiver<Result<String, String>>;

/// Manages Docker container data collection and log streaming.
pub struct DockerMonitor {
    client: Option<DockerClient>,
    pub containers: Vec<DockerContainerInfo>,
    cpu_cache: HashMap<String, f64>,
    cpu_refresh_cursor: usize,
    pub ui_state: ContainerUIState,
    pub log_states: HashMap<String, LogViewState>,
    pub multi_log_state: Option<MultiLogViewState>,
    log_receivers: HashMap<String, mpsc::Receiver<String>>,
    multi_log_seq: u64,
    rt: Arc<tokio::runtime::Runtime>,
    pub docker_available: bool,
    pub status_message: Option<String>,
    action_receiver: Option<ActionReceiver>,
    pub action_in_progress: bool,
}

impl DockerMonitor {
    pub fn new(rt: Arc<tokio::runtime::Runtime>) -> Self {
        let client = DockerClient::try_new();

        // Verify daemon is actually reachable
        let docker_available = if let Some(ref c) = client {
            rt.block_on(c.is_available())
        } else {
            false
        };

        Self {
            client: if docker_available { client } else { None },
            containers: Vec::new(),
            cpu_cache: HashMap::new(),
            cpu_refresh_cursor: 0,
            ui_state: ContainerUIState::default(),
            log_states: HashMap::new(),
            multi_log_state: None,
            log_receivers: HashMap::new(),
            multi_log_seq: 0,
            rt,
            docker_available,
            status_message: None,
            action_receiver: None,
            action_in_progress: false,
        }
    }

    /// Refresh container list and stats. Called on the 3-second tick.
    pub fn update(&mut self) {
        let Some(ref client) = self.client else { return };

        match self.rt.block_on(client.list_containers()) {
            Ok(mut containers) => {
                let ids: Vec<String> = containers.iter().map(|c| c.id.clone()).collect();

                // Reuse recent CPU values for the full list, then refresh a
                // small rotating subset to avoid one stats call per container
                // on every tick.
                for c in &mut containers {
                    c.cpu_percent = *self.cpu_cache.get(&c.id).unwrap_or(&0.0);
                }

                const CPU_REFRESH_BATCH_SIZE: usize = 4;
                if !ids.is_empty() {
                    let start = self.cpu_refresh_cursor.min(ids.len());
                    let end = (start + CPU_REFRESH_BATCH_SIZE).min(ids.len());
                    let refresh_ids = if start < end {
                        &ids[start..end]
                    } else {
                        &ids[..ids.len().min(CPU_REFRESH_BATCH_SIZE)]
                    };

                    let refresh_ids_vec: Vec<String> = refresh_ids.to_vec();
                    let cpu_percents = self.rt.block_on(client.get_all_cpu_percents(&refresh_ids_vec));
                    for (id, cpu) in refresh_ids.iter().zip(cpu_percents.into_iter()) {
                        self.cpu_cache.insert(id.clone(), cpu);
                    }

                    self.cpu_refresh_cursor = if end >= ids.len() {
                        0
                    } else {
                        end
                    };
                } else {
                    self.cpu_refresh_cursor = 0;
                }

                self.cpu_cache.retain(|id, _| ids.contains(id));
                for c in &mut containers {
                    c.cpu_percent = *self.cpu_cache.get(&c.id).unwrap_or(&c.cpu_percent);
                }

                self.containers = containers;
            }
            Err(e) => {
                self.status_message = Some(format!("Error: {}", e));
            }
        }

        // Clamp selected index
        let total = self.containers.len();
        self.ui_state.total_rows = total;
        if self.ui_state.selected_index >= total && total > 0 {
            self.ui_state.selected_index = total - 1;
        }
    }

    /// Check if Docker is available (for showing/hiding the tab).
    pub fn is_available(&self) -> bool {
        self.docker_available
    }

    /// Get the currently selected container, if any.
    pub fn selected_container(&self) -> Option<&DockerContainerInfo> {
        self.containers.get(self.ui_state.selected_index)
    }

    /// Start tailing logs for the given container.
    pub fn start_log_stream(&mut self, container_id: &str, container_name: &str) {
        // Create a fresh client for this log stream to avoid any concurrency issues
        let Some(client) = DockerClient::try_new() else { 
            return 
        };

        let handle = self.rt.handle();
        let rx = client.tail_logs(container_id, handle);

        self.log_states.insert(
            container_id.to_string(),
            LogViewState::new(container_id.to_string(), container_name.to_string()),
        );
        self.log_receivers.insert(container_id.to_string(), rx);
    }

    /// Start tailing logs for multiple containers (preserve existing streams).
    pub fn start_log_stream_multi(&mut self, containers: &[(String, String)]) {
        if self.multi_log_state.is_none() {
            self.multi_log_state = Some(MultiLogViewState::new());
        }
        for (container_id, container_name) in containers {
            if self.log_states.contains_key(container_id) {
                continue; // already streaming this container
            }
            self.start_log_stream(container_id, container_name);
        }
    }

    /// Stop the log stream and return to container list.
    pub fn stop_log_stream(&mut self) {
        self.log_receivers.clear();
        self.log_states.clear();
        self.multi_log_state = None;
        self.multi_log_seq = 0;
    }

    // Note: per-container stream teardown is handled implicitly when receivers
    // disconnect; we no longer expose a separate public API for stopping one.

    /// Get a single log state by container ID.
    pub fn get_log_state(&self, container_id: &str) -> Option<&LogViewState> {
        self.log_states.get(container_id)
    }

    /// Get a mutable single log state by container ID.
    pub fn get_log_state_mut(&mut self, container_id: &str) -> Option<&mut LogViewState> {
        self.log_states.get_mut(container_id)
    }

    /// Drain any pending log lines from the channel into LogViewState.
    /// Should be called frequently (~100ms) when in log view.
    pub fn poll_logs(&mut self) {
        let container_ids: Vec<String> = self.log_receivers.keys().cloned().collect();
        let mut disconnected_ids: Vec<String> = Vec::new();

        // Fair round-robin fan-in: at most one line per container each round.
        // This prevents one noisy stream from dominating the visible tail.
        for _ in 0..100 {
            let mut made_progress = false;

            for container_id in &container_ids {
                let maybe_line = if let Some(ref mut rx) = self.log_receivers.get_mut(container_id) {
                    match rx.try_recv() {
                        Ok(line) => Some(Ok(line)),
                        Err(mpsc::error::TryRecvError::Empty) => None,
                        Err(mpsc::error::TryRecvError::Disconnected) => Some(Err(())),
                    }
                } else {
                    None
                };

                match maybe_line {
                    Some(Ok(line)) => {
                        made_progress = true;
                        if let Some(ref mut log_state) = self.log_states.get_mut(container_id) {
                            let container_name = log_state.container_name.clone();
                            log_state.push_line(line.clone());
                            if let Some(ref mut multi) = self.multi_log_state {
                                self.multi_log_seq += 1;
                                multi.push_line(MultiLogLine {
                                    container_id: container_id.clone(),
                                    container_name,
                                    line,
                                    seq: self.multi_log_seq,
                                });
                            }
                        }
                    }
                    Some(Err(())) => {
                        disconnected_ids.push(container_id.clone());
                        if let Some(ref mut log_state) = self.log_states.get_mut(container_id) {
                            let container_name = log_state.container_name.clone();
                            let stream_ended = "[log stream ended]".to_string();
                            log_state.push_line(stream_ended.clone());
                            if let Some(ref mut multi) = self.multi_log_state {
                                self.multi_log_seq += 1;
                                multi.push_line(MultiLogLine {
                                    container_id: container_id.clone(),
                                    container_name,
                                    line: stream_ended,
                                    seq: self.multi_log_seq,
                                });
                            }
                        }
                    }
                    None => {}
                }
            }

            if !made_progress {
                break;
            }
        }

        // Drop disconnected receivers so we don't emit repeated "ended" lines.
        for id in disconnected_ids {
            self.log_receivers.remove(&id);
        }
    }

    /// Container action: start (non-blocking).
    pub fn start_container(&mut self, container_id: &str) {
        self.run_container_action(container_id, "start");
    }

    /// Container action: stop (non-blocking).
    pub fn stop_container(&mut self, container_id: &str) {
        self.run_container_action(container_id, "stop");
    }

    /// Container action: restart (non-blocking).
    pub fn restart_container(&mut self, container_id: &str) {
        self.run_container_action(container_id, "restart");
    }

    /// Run a container action in a background thread to keep the TUI responsive.
    fn run_container_action(&mut self, container_id: &str, action: &str) {
        if self.action_in_progress {
            self.status_message = Some("An action is already in progress...".to_string());
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.action_receiver = Some(rx);
        self.action_in_progress = true;
        self.status_message = Some(format!("{}ing container {}...", capitalize(action), container_id));

        let rt = Arc::clone(&self.rt);
        let id = container_id.to_string();
        let act = action.to_string();
        // We need a fresh client connection since DockerClient isn't Send across threads.
        // Instead, spawn on the existing tokio runtime from a new std::thread.
        std::thread::spawn(move || {
            let result = rt.block_on(async {
                let client = match crate::docker::DockerClient::try_new() {
                    Some(c) => c,
                    None => return Err("Failed to connect to Docker".to_string()),
                };
                match act.as_str() {
                    "start" => client.start_container(&id).await.map(|_| format!("Started {}", id)),
                    "stop" => client.stop_container(&id).await.map(|_| format!("Stopped {}", id)),
                    "restart" => client.restart_container(&id).await.map(|_| format!("Restarted {}", id)),
                    _ => Err("Unknown action".to_string()),
                }
            });
            let _ = tx.send(result);
        });
    }

    /// Poll for background action completion. Returns true if status changed.
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
                self.status_message = Some(format!("Error: {}", msg));
                self.action_in_progress = false;
                self.action_receiver = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_message = Some("Action failed unexpectedly".to_string());
                self.action_in_progress = false;
                self.action_receiver = None;
                true
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().to_string() + c.as_str(),
    }
}
