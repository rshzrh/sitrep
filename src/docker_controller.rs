use tokio::sync::mpsc;
use std::sync::Arc;

use crate::docker::DockerClient;
use crate::model::{DockerContainerInfo, ContainerUIState, LogViewState};

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
        }
    }

    /// Refresh container list and stats. Called on the 3-second tick.
    pub fn update(&mut self) {
        let Some(ref client) = self.client else { return };

        let mut containers = self.rt.block_on(client.list_containers());

        // Fetch CPU stats for each container (one-shot, fast)
        for c in &mut containers {
            c.cpu_percent = self.rt.block_on(client.get_cpu_percent(&c.id));
        }

        self.containers = containers;

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

    /// Container action: start
    pub fn start_container(&mut self, container_id: &str) {
        let Some(ref client) = self.client else { return };
        match self.rt.block_on(client.start_container(container_id)) {
            Ok(()) => self.status_message = Some(format!("Started {}", container_id)),
            Err(e) => self.status_message = Some(format!("Error: {}", e)),
        }
    }

    /// Container action: stop
    pub fn stop_container(&mut self, container_id: &str) {
        let Some(ref client) = self.client else { return };
        match self.rt.block_on(client.stop_container(container_id)) {
            Ok(()) => self.status_message = Some(format!("Stopped {}", container_id)),
            Err(e) => self.status_message = Some(format!("Error: {}", e)),
        }
    }

    /// Container action: restart
    pub fn restart_container(&mut self, container_id: &str) {
        let Some(ref client) = self.client else { return };
        match self.rt.block_on(client.restart_container(container_id)) {
            Ok(()) => self.status_message = Some(format!("Restarted {}", container_id)),
            Err(e) => self.status_message = Some(format!("Error: {}", e)),
        }
    }
}
