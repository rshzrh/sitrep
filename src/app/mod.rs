mod state;
mod event_loop;
mod render;
mod input;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, Clear, ClearType},
};

use crate::collectors::remote::SshAuth;
use crate::controller::Monitor;
use crate::docker_controller::DockerMonitor;
use crate::remote_host::RemoteHost;
use crate::swarm_controller::SwarmMonitor;
use crate::model::{AppView, FleetState, LogViewState, MultiLogLine, MultiLogViewState, ServiceLogState};
use crate::view::{Presenter, RowKind};
use std::collections::HashMap;
use sysinfo::Pid;

pub use state::{PendingAction, PendingActionKind, SwarmOverviewItem, resolve_swarm_overview_item};

/// Restore the terminal to normal mode. Safe to call multiple times.
pub fn restore_terminal() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// Main application state and event loop.
pub struct App {
    pub monitor: Monitor,
    pub docker_monitor: DockerMonitor,
    pub swarm_monitor: SwarmMonitor,
    pub app_view: AppView,
    pub row_mapping: Vec<(Pid, RowKind)>,
    pub pending_action: Option<PendingAction>,
    pub last_tick: Instant,
    pub tick_counter: u64,
    pub last_tab_refresh: Instant,
    pub prev_app_view: AppView,
    pub tick_rate: Duration,
    pub min_refresh_interval: Duration,
    /// Fleet state — only `Some` when flotop was invoked with one or
    /// more host arguments. None means single-host (legacy) mode.
    pub fleet_state: Option<FleetState>,
    /// Per-host orchestrators. Parallel to `fleet_state.hosts`.
    pub remote_hosts: Vec<RemoteHost>,
    /// Log view states keyed by stream id (container id or service id),
    /// per host. Main loop drains `remote_hosts[i].log_rx` into these
    /// each tick. `LogViewState` is non-Send (contains RefCell), which
    /// is why we can't store it on `RemoteHost` itself.
    pub remote_log_states: HashMap<(usize, String), LogViewState>,
    /// Per-host service log state. One active service log per host.
    pub remote_service_logs: HashMap<usize, ServiceLogState>,
    /// Per-host multi-container log state. Created when user presses
    /// l/L on the remote Containers tab. Keyed by host index.
    pub remote_multi_logs: HashMap<usize, MultiLogViewState>,
    /// Per-host row mapping produced by `Presenter::render` for the
    /// remote System tab. Needed by `handle_remote_system` to resolve
    /// Up/Down/Left/Right key presses into section toggles and process
    /// group expansions (the same way the local path uses `row_mapping`).
    pub remote_row_mappings: HashMap<usize, Vec<(Pid, RowKind)>>,
    /// Tokio runtime shared with the remote tasks (needed to spawn log
    /// streams and action tasks from the input handlers).
    pub rt: Arc<tokio::runtime::Runtime>,
}

impl App {
    pub fn new(rt: Arc<tokio::runtime::Runtime>, tick_rate_secs: u64, no_docker: bool) -> Self {
        let tick_rate = Duration::from_secs(tick_rate_secs);

        let monitor_handle = std::thread::spawn(Monitor::new);
        let swarm_handle = std::thread::spawn(SwarmMonitor::new);
        let rt_clone = Arc::clone(&rt);
        let docker_handle = std::thread::spawn(move || DockerMonitor::new(rt_clone, no_docker));

        let monitor = monitor_handle.join().expect("Monitor init panicked");
        let docker_monitor = docker_handle.join().expect("DockerMonitor init panicked");
        let swarm_monitor = swarm_handle.join().expect("SwarmMonitor init panicked");
        let app_view = AppView::System;

        tracing::info!(
            "Docker available: {}, Swarm mode: {}",
            docker_monitor.docker_available,
            swarm_monitor.is_swarm()
        );

        Self {
            monitor,
            docker_monitor,
            swarm_monitor,
            app_view: app_view.clone(),
            row_mapping: Vec::new(),
            pending_action: None,
            last_tick: Instant::now() - tick_rate,
            tick_counter: 0,
            last_tab_refresh: Instant::now() - Duration::from_millis(500),
            prev_app_view: app_view,
            tick_rate,
            min_refresh_interval: Duration::from_millis(500),
            fleet_state: None,
            remote_hosts: Vec::new(),
            remote_log_states: HashMap::new(),
            remote_service_logs: HashMap::new(),
            remote_multi_logs: HashMap::new(),
            remote_row_mappings: HashMap::new(),
            rt,
        }
    }

    /// Drain the per-host `RemoteHostState` into `FleetState.hosts` so
    /// the fleet-overview row reflects the latest data. Returns true
    /// if anything changed this tick.
    pub fn poll_fleet(&mut self) -> bool {
        let Some(state) = self.fleet_state.as_mut() else {
            return false;
        };
        let mut changed = false;
        for (i, rh) in self.remote_hosts.iter().enumerate() {
            let host = match state.hosts.get_mut(i) {
                Some(h) => h,
                None => continue,
            };
            let rh_state = rh.state.lock().unwrap();
            use crate::collectors::remote::ConnState;
            let new_status = match rh_state.conn_state {
                ConnState::Connected => crate::model::HostStatus::Up,
                ConnState::Degraded => crate::model::HostStatus::Degraded,
                ConnState::Disconnected => crate::model::HostStatus::Disconnected,
            };
            if host.status != new_status {
                host.status = new_status;
                changed = true;
            }
            if host.last_error != rh_state.last_error {
                host.last_error = rh_state.last_error.clone();
                changed = true;
            }
            if host.last_refresh != rh_state.last_refresh {
                host.last_refresh = rh_state.last_refresh;
                changed = true;
            }
            if host.consecutive_failures != rh_state.consecutive_failures {
                host.consecutive_failures = rh_state.consecutive_failures;
                changed = true;
            }
            // Copy LCD vitals from monitor_data + containers count.
            if let Some(ref md) = rh_state.monitor_data {
                host.vitals.load_1m = Some(md.load_avg.0);
                host.vitals.load_5m = Some(md.load_avg.1);
                host.vitals.load_15m = Some(md.load_avg.2);
                host.vitals.mem_used_bytes = md.memory.used;
                host.vitals.mem_total_bytes = md.memory.total;
                host.vitals.swap_used_bytes = md.memory.swap_used;
                host.vitals.swap_total_bytes = md.memory.swap_total;
                // Busiest disk % from disk_space list (percent_free → used).
                host.vitals.disk_busiest_pct = md
                    .disk_space
                    .iter()
                    .map(|d| 100.0 - d.percent_free)
                    .fold(None, |acc: Option<f64>, p| {
                        Some(acc.map_or(p, |a| a.max(p)))
                    });
                host.vitals.top_proc = md
                    .historical_top
                    .first()
                    .map(|p| (p.name.clone(), p.cpu as f32));
                changed = true;
            }
            host.vitals.container_count = Some(rh_state.containers.len() as u32);
            host.vitals.established_conns =
                Some(rh_state.monitor_data
                    .as_ref()
                    .map(|m| m.socket_overview.established)
                    .unwrap_or(0));
        }
        changed
    }

    /// Drain remote log streams into per-(host, stream) `LogViewState`
    /// buffers. Creates the LogViewState on first line.
    ///
    /// Routing priority for each line:
    /// 1. If the host has an active multi-log view AND the line's container
    ///    id is in the multi-log set → push into `remote_multi_logs[host]`.
    /// 2. If the line matches a container id → push into
    ///    `remote_log_states[(host, id)]`.
    /// 3. Otherwise → push into `remote_service_logs[host]` (service log).
    pub fn poll_remote_logs(&mut self) -> bool {
        let mut changed = false;
        for (i, rh) in self.remote_hosts.iter_mut().enumerate() {
            let Some(rx) = rh.log_rx.as_mut() else {
                continue;
            };

            // Snapshot the multi-log container ids (under a brief lock)
            // so we know how to route lines for this tick.
            let multi_ids: Vec<String> = {
                let s = rh.state.lock().unwrap();
                s.multi_log_container_ids.clone()
            };

            while let Ok(line) = rx.try_recv() {
                // Route 1: multi-log view active and this container is in it.
                if !multi_ids.is_empty() && multi_ids.contains(&line.stream_id) {
                    // Look up the container name for the prefix.
                    let name = {
                        let s = rh.state.lock().unwrap();
                        s.containers
                            .iter()
                            .find(|c| c.id == line.stream_id)
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| line.stream_id.clone())
                    };
                    let multi = self
                        .remote_multi_logs
                        .entry(i)
                        .or_insert_with(MultiLogViewState::new);
                    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    multi.push_line(MultiLogLine {
                        container_id: line.stream_id.clone(),
                        container_name: name,
                        line: line.line,
                        seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    });
                    changed = true;
                    continue;
                }

                // Route 2: single-container log.
                let rh_state = rh.state.lock().unwrap();
                let is_container = rh_state.containers.iter().any(|c| c.id == line.stream_id);
                drop(rh_state);
                if is_container {
                    let key = (i, line.stream_id.clone());
                    let log_state = self
                        .remote_log_states
                        .entry(key)
                        .or_insert_with(|| {
                            LogViewState::new(line.stream_id.clone(), line.stream_id.clone())
                        });
                    log_state.push_line(line.line);
                } else {
                    // Route 3: service log.
                    let svc_state = self
                        .remote_service_logs
                        .entry(i)
                        .or_insert_with(|| {
                            ServiceLogState::new(line.stream_id.clone(), line.stream_id.clone())
                        });
                    svc_state.push_line(line.line);
                }
                changed = true;
            }
        }
        changed
    }

    /// Clean up all remote state. Called when the user quits or the
    /// App is dropped. Stops all active log streams, clears all
    /// cached per-host view state, and drops the tokio tasks.
    pub fn cleanup_remote(&mut self) {
        for rh in &self.remote_hosts {
            rh.stop_all_log_streams();
        }
        self.remote_log_states.clear();
        self.remote_service_logs.clear();
        self.remote_multi_logs.clear();
        self.remote_row_mappings.clear();
    }

    /// Drain remote action results. Sets the status message on the
    /// corresponding host's state.
    pub fn poll_remote_actions(&mut self) -> bool {
        let mut changed = false;
        for rh in self.remote_hosts.iter_mut() {
            let Some(rx) = rh.action_rx.as_mut() else {
                continue;
            };
            while let Ok(result) = rx.try_recv() {
                let mut s = rh.state.lock().unwrap();
                let prefix = if result.success { "OK" } else { "FAIL" };
                s.status_message = Some(format!(
                    "[{}] {} {}: {}",
                    prefix, result.kind, result.target_id, result.message
                ));
                changed = true;
            }
        }
        changed
    }
}

/// Run the application. Sets up terminal, runs the main loop, restores terminal on exit.
pub fn run(should_quit: Arc<AtomicBool>, cli: &crate::cli::Cli) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Clear(ClearType::All))?;

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .build()
            .expect("Failed to create tokio runtime"),
    );

    Presenter::render_splash()?;

    let mut app = App::new(Arc::clone(&rt), cli.refresh_rate, cli.no_docker);

    // ── multi-host mode: spawn per-host orchestrators + show fleet overview ──
    if !cli.hosts.is_empty() {
        let display_names: Vec<String> = cli.hosts.clone();
        app.fleet_state = Some(FleetState::new(display_names));
        app.app_view = AppView::FleetOverview;
        app.prev_app_view = AppView::FleetOverview;

        let keys = cli.resolve_ssh_keys();
        let remote_refresh = Duration::from_secs(cli.remote_refresh);
        for (i, raw) in cli.hosts.iter().enumerate() {
            let mut auth = SshAuth::parse(raw, &cli.user);
            auth.key_candidates = keys.clone();
            let mut rh = RemoteHost::new(i, auth);
            rh.spawn_refresh_loop(Arc::clone(&rt), remote_refresh);
            app.remote_hosts.push(rh);
        }
    }

    let mut needs_render = true;

    loop {
        if should_quit.load(Ordering::Relaxed) {
            break;
        }

        let now = Instant::now();

        if app.expire_pending_action() {
            needs_render = true;
        }
        if app.monitor.poll_update() {
            needs_render = true;
        }
        if app.docker_monitor.poll_update() {
            needs_render = true;
        }
        if app.process_tick() {
            needs_render = true;
        }
        if app.poll_logs() {
            needs_render = true;
        }
        if app.poll_actions() {
            needs_render = true;
        }
        if app.refresh_on_tab_switch() {
            needs_render = true;
        }
        if app.poll_fleet() {
            needs_render = true;
        }
        if app.poll_remote_logs() {
            needs_render = true;
        }
        if app.poll_remote_actions() {
            needs_render = true;
        }

        if needs_render {
            if Presenter::render_size_guard()? {
                needs_render = false;
                let timeout = app.tick_rate.saturating_sub(now.elapsed());
                if crossterm::event::poll(timeout.min(Duration::from_millis(100)))? {
                    let _ = crossterm::event::read()?;
                }
                continue;
            }

            render::render(&mut app)?;

            if let Some(ref pa) = app.pending_action {
                Presenter::render_confirmation(&pa.description)?;
            }

            needs_render = false;
        }

        let timeout = app.tick_rate.saturating_sub(now.elapsed());
        if crossterm::event::poll(timeout.min(Duration::from_millis(100)))? {
            if let crossterm::event::Event::Key(key_event) = crossterm::event::read()? {
                match input::handle_key(&mut app, key_event) {
                    Some(input::InputResult::Quit) => break,
                    Some(input::InputResult::Consumed) => needs_render = true,
                    None => {}
                }
            }
        }
    }

    app.cleanup_remote();
    restore_terminal();
    Ok(())
}
