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

use crate::controller::Monitor;
use crate::docker_controller::DockerMonitor;
use crate::swarm_controller::SwarmMonitor;
use crate::model::AppView;
use crate::view::{Presenter, RowKind};
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
        }
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
            .worker_threads(2)
            .build()
            .expect("Failed to create tokio runtime"),
    );

    Presenter::render_splash()?;

    let mut app = App::new(Arc::clone(&rt), cli.refresh_rate, cli.no_docker);
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

    restore_terminal();
    Ok(())
}
