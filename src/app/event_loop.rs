use std::time::Instant;

use crate::controller::DataMonitor;
use crate::model::{AppView, TabKind};

use super::App;

impl App {
    /// Kick the background refresh for whichever monitor is backing
    /// the current top-level tab. Single source of truth for the
    /// "which monitor does this tab use" mapping — both `process_tick`
    /// and `refresh_on_tab_switch` call this. Adding a 4th monitor
    /// means adding one match arm here plus a `TabKind` variant.
    fn dispatch_active_update(&mut self) {
        let mon: Option<&mut dyn DataMonitor> = match self.app_view.tab_kind() {
            TabKind::System => Some(&mut self.monitor),
            TabKind::Containers => Some(&mut self.docker_monitor),
            TabKind::Swarm => Some(&mut self.swarm_monitor),
            TabKind::Fleet | TabKind::Remote => {
                // Fleet refreshes are driven by per-host tokio tasks
                // via mpsc; poll_fleet() / poll_remote_* drain those
                // updates each tick. The SSH I/O runs in the background.
                None
            }
        };
        if let Some(mon) = mon {
            if mon.is_available() {
                mon.update();
            }
        }
    }

    /// Process tick-based data refresh (every 3 seconds).
    pub fn process_tick(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_tick) < self.tick_rate {
            return false;
        }

        self.tick_counter += 1;
        self.dispatch_active_update();

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
            if let AppView::ContainerLogs(container_id) = &self.app_view {
                let had_lines = self
                    .docker_monitor
                    .get_log_state(container_id)
                    .map(|s| s.lines.len())
                    .unwrap_or(0);
                self.docker_monitor.poll_logs();
                let has_lines = self
                    .docker_monitor
                    .get_log_state(container_id)
                    .map(|s| s.lines.len())
                    .unwrap_or(0);
                if has_lines != had_lines {
                    needs_render = true;
                }
            }
        }
        if matches!(self.app_view, AppView::ContainerLogsMulti(_)) {
            let had_total = self
                .docker_monitor
                .multi_log_state
                .as_ref()
                .map(|s| s.lines.len())
                .unwrap_or(0);
            self.docker_monitor.poll_logs();
            let has_total = self
                .docker_monitor
                .multi_log_state
                .as_ref()
                .map(|s| s.lines.len())
                .unwrap_or(0);
            if has_total != had_total || (has_total > 0 && had_total == 0) {
                needs_render = true;
            }
        }
        if matches!(self.app_view, AppView::SwarmServiceLogs(_, _)) {
            let had_lines = self
                .swarm_monitor
                .log_state
                .as_ref()
                .map(|s| s.lines.len())
                .unwrap_or(0);
            self.swarm_monitor.poll_logs();
            let has_lines = self
                .swarm_monitor
                .log_state
                .as_ref()
                .map(|s| s.lines.len())
                .unwrap_or(0);
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
            // Gate expensive background collectors on whether their tab
            // is the active view. Only the macOS `nettop` loop actually
            // listens to this today, but the flag lives on every
            // `DataMonitor` via the trait so future collectors can opt
            // in for free.
            let system_active = matches!(self.app_view, AppView::System);
            self.monitor.set_active(system_active);

            let since_last = now.duration_since(self.last_tab_refresh);
            if since_last >= self.min_refresh_interval {
                self.dispatch_active_update();
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
