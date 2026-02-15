use std::time::Instant;

use crate::model::AppView;

use super::App;

impl App {
    /// Process tick-based data refresh (every 3 seconds).
    pub fn process_tick(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_tick) < self.tick_rate {
            return false;
        }

        self.tick_counter += 1;

        match &self.app_view {
            AppView::System => {
                self.monitor.update();
            }
            AppView::Containers | AppView::ContainerLogs(_) => {
                if self.docker_monitor.is_available() {
                    self.docker_monitor.update();
                }
            }
            AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                if self.swarm_monitor.is_swarm() {
                    self.swarm_monitor.update();
                }
            }
        }

        if !self.swarm_monitor.is_swarm() && self.tick_counter % 10 == 0 {
            self.swarm_monitor.recheck_swarm();
        }

        self.last_tick = now;
        true
    }

    /// Poll logs if in log view.
    pub fn poll_logs(&mut self) -> bool {
        let mut needs_render = false;

        if matches!(self.app_view, AppView::ContainerLogs(_)) {
            let had_lines = self.docker_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            self.docker_monitor.poll_logs();
            let has_lines = self.docker_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            if has_lines != had_lines {
                needs_render = true;
            }
        }
        if matches!(self.app_view, AppView::SwarmServiceLogs(_, _)) {
            let had_lines = self.swarm_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            self.swarm_monitor.poll_logs();
            let has_lines = self.swarm_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            if has_lines != had_lines {
                needs_render = true;
            }
        }

        needs_render
    }

    /// Poll background actions (container start/stop/restart, rolling restart, scale).
    pub fn poll_actions(&mut self) -> bool {
        let mut needs_render = false;
        if self.docker_monitor.action_in_progress && self.docker_monitor.poll_action() {
            needs_render = true;
        }
        if self.swarm_monitor.action_in_progress && self.swarm_monitor.poll_action() {
            needs_render = true;
        }
        needs_render
    }

    /// Immediate refresh on tab switch.
    pub fn refresh_on_tab_switch(&mut self) -> bool {
        let now = Instant::now();
        if self.app_view != self.prev_app_view {
            let since_last = now.duration_since(self.last_tab_refresh);
            if since_last >= self.min_refresh_interval {
                match &self.app_view {
                    AppView::System => {
                        self.monitor.update();
                    }
                    AppView::Containers | AppView::ContainerLogs(_) => {
                        if self.docker_monitor.is_available() {
                            self.docker_monitor.update();
                        }
                    }
                    AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                        if self.swarm_monitor.is_swarm() {
                            self.swarm_monitor.update();
                        }
                    }
                }
                self.last_tab_refresh = now;
            }
            self.prev_app_view = self.app_view.clone();
            return true;
        }
        false
    }

    /// Expire pending confirmation if timed out.
    pub fn expire_pending_action(&mut self) -> bool {
        let now = Instant::now();
        if let Some(ref pa) = self.pending_action {
            if now > pa.expires {
                self.pending_action = None;
                return true;
            }
        }
        false
    }
}
