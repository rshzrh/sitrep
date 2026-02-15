use tokio::sync::mpsc;
use std::sync::Arc;

use crate::docker::DockerClient;
use crate::model::{DockerContainerInfo, ContainerUIState, LogViewState};

/// Receiver for background Docker action results.
type ActionReceiver = std::sync::mpsc::Receiver<Result<String, String>>;

/// Manages Docker container data collection and log streaming.
pub struct DockerMonitor {
    client: Option<DockerClient>,
    pub containers: Vec<DockerContainerInfo>,
    pub ui_state: ContainerUIState,
    pub log_state: Option<LogViewState>,
    log_receiver: Option<mpsc::Receiver<String>>,
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
            ui_state: ContainerUIState::default(),
            log_state: None,
            log_receiver: None,
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
                // Fetch CPU stats for all containers concurrently
                let ids: Vec<String> = containers.iter().map(|c| c.id.clone()).collect();
                let cpu_percents = self.rt.block_on(client.get_all_cpu_percents(&ids));
                for (c, cpu) in containers.iter_mut().zip(cpu_percents.into_iter()) {
                    c.cpu_percent = cpu;
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
        let Some(ref client) = self.client else { return };

        let handle = self.rt.handle();
        let rx = client.tail_logs(container_id, handle);

        self.log_state = Some(LogViewState::new(
            container_id.to_string(),
            container_name.to_string(),
        ));
        self.log_receiver = Some(rx);
    }

    /// Stop the log stream and return to container list.
    pub fn stop_log_stream(&mut self) {
        self.log_receiver = None;
        self.log_state = None;
    }

    /// Drain any pending log lines from the channel into LogViewState.
    /// Should be called frequently (~100ms) when in log view.
    pub fn poll_logs(&mut self) {
        let Some(ref mut rx) = self.log_receiver else { return };
        let Some(ref mut log_state) = self.log_state else { return };

        // Drain up to 100 lines per poll to avoid blocking
        for _ in 0..100 {
            match rx.try_recv() {
                Ok(line) => {
                    log_state.push_line(line);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    log_state.push_line("[log stream ended]".to_string());
                    break;
                }
            }
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
