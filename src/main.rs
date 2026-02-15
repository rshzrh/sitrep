pub mod model;
pub mod view;
pub mod controller;
pub mod layout;
pub mod collectors;
pub mod docker;
pub mod docker_controller;
pub mod swarm;
pub mod swarm_controller;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use crossterm::{
    execute,
    event::{self, Event, KeyCode, KeyModifiers, KeyEvent},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, Clear, ClearType},
};
use controller::Monitor;
use docker_controller::DockerMonitor;
use swarm_controller::SwarmMonitor;
use model::{AppView, SortColumn, SwarmViewLevel};
use view::{Presenter, RowKind};
use sysinfo::Pid;

/// Restore the terminal to normal mode. Safe to call multiple times.
fn restore_terminal() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// Pending destructive action awaiting confirmation.
struct PendingAction {
    description: String,
    kind: PendingActionKind,
    expires: Instant,
}

enum PendingActionKind {
    ContainerStart(String),
    ContainerStop(String),
    ContainerRestart(String),
    SwarmRollingRestart(String),
}

fn main() -> io::Result<()> {
    // Install panic hook that restores terminal before printing the panic
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // Register signal handler for SIGTERM / SIGINT (Unix)
    let should_quit = Arc::new(AtomicBool::new(false));
    {
        let quit_flag = Arc::clone(&should_quit);
        let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, quit_flag);
    }
    {
        let quit_flag = Arc::clone(&should_quit);
        let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, quit_flag);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Clear(ClearType::All))?;

    // Create a tokio runtime for async Docker operations
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("Failed to create tokio runtime"),
    );

    let mut monitor = Monitor::new();
    let mut docker_monitor = DockerMonitor::new(Arc::clone(&rt));
    let mut swarm_monitor = SwarmMonitor::new();

    let tick_rate = Duration::from_secs(3);
    let mut last_tick = Instant::now() - tick_rate;
    let mut row_mapping: Vec<(Pid, RowKind)> = Vec::new();
    let mut needs_render = true;
    let mut app_view = AppView::System;
    let mut tick_counter: u64 = 0; // counts 3s ticks
    let mut pending_action: Option<PendingAction> = None;
    let min_refresh_interval = Duration::from_millis(500);
    let mut last_tab_refresh = Instant::now() - min_refresh_interval;
    let mut prev_app_view = app_view.clone();

    loop {
        // Check for OS signals (SIGTERM, SIGINT)
        if should_quit.load(Ordering::Relaxed) {
            break;
        }

        let now = Instant::now();

        // Expire pending confirmation
        if let Some(ref pa) = pending_action {
            if now > pa.expires {
                pending_action = None;
                needs_render = true;
            }
        }

        // --- Tick-based data refresh (every 3 seconds) ---
        if now.duration_since(last_tick) >= tick_rate {
            tick_counter += 1;

            // Only update the active view's monitor to avoid wasted work
            match &app_view {
                AppView::System => {
                    monitor.update();
                }
                AppView::Containers | AppView::ContainerLogs(_) => {
                    if docker_monitor.is_available() {
                        docker_monitor.update();
                    }
                }
                AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                    if swarm_monitor.is_swarm() {
                        swarm_monitor.update();
                    }
                }
            }

            // Re-detect swarm mode every ~30 seconds (10 ticks) when standalone
            if !swarm_monitor.is_swarm() && tick_counter % 10 == 0 {
                swarm_monitor.recheck_swarm();
            }

            last_tick = now;
            needs_render = true;
        }

        // --- Poll logs if in log view (every loop iteration ~100ms) ---
        if matches!(app_view, AppView::ContainerLogs(_)) {
            let had_lines = docker_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            docker_monitor.poll_logs();
            let has_lines = docker_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            if has_lines != had_lines {
                needs_render = true;
            }
        }
        if matches!(app_view, AppView::SwarmServiceLogs(_, _)) {
            let had_lines = swarm_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            swarm_monitor.poll_logs();
            let has_lines = swarm_monitor.log_state.as_ref()
                .map(|s| s.lines.len()).unwrap_or(0);
            if has_lines != had_lines {
                needs_render = true;
            }
        }

        // --- Poll background actions (container start/stop/restart, rolling restart, scale) ---
        if docker_monitor.action_in_progress && docker_monitor.poll_action() {
            needs_render = true;
        }
        if swarm_monitor.action_in_progress && swarm_monitor.poll_action() {
            needs_render = true;
        }

        // --- Immediate refresh on tab switch (avoid stale data) ---
        if app_view != prev_app_view {
            let since_last = now.duration_since(last_tab_refresh);
            if since_last >= min_refresh_interval {
                match &app_view {
                    AppView::System => { monitor.update(); }
                    AppView::Containers | AppView::ContainerLogs(_) => {
                        if docker_monitor.is_available() { docker_monitor.update(); }
                    }
                    AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                        if swarm_monitor.is_swarm() { swarm_monitor.update(); }
                    }
                }
                last_tab_refresh = now;
            }
            prev_app_view = app_view.clone();
            needs_render = true;
        }

        // --- Render ---
        if needs_render {
            // Check minimum terminal size
            if Presenter::render_size_guard()? {
                needs_render = false;
                // Still need to handle input so we don't skip the event loop
                let timeout = tick_rate.saturating_sub(now.elapsed());
                if event::poll(timeout.min(Duration::from_millis(100)))? {
                    let _ = event::read()?; // consume event
                }
                continue;
            }
            let time_str = monitor.last_data.as_ref()
                .map(|d| d.time.clone())
                .unwrap_or_else(|| "...".to_string());

            let swarm_active = swarm_monitor.is_swarm();
            let swarm_node_count = swarm_monitor.cluster_info.as_ref()
                .map(|c| c.nodes_total).unwrap_or(0);

            match &app_view {
                AppView::System => {
                    let mut out = io::stdout();
                    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
                    Presenter::render_tab_bar(
                        &mut out, &app_view,
                        docker_monitor.is_available(),
                        docker_monitor.containers.len(),
                        swarm_active, swarm_node_count,
                        &time_str,
                    )?;
                    if let Some(ref data) = monitor.last_data {
                        row_mapping = Presenter::render(data, &mut monitor.ui_state, &monitor.layout)?;
                    }
                }
                AppView::Containers => {
                    let mut out = io::stdout();
                    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
                    Presenter::render_tab_bar(
                        &mut out, &app_view,
                        docker_monitor.is_available(),
                        docker_monitor.containers.len(),
                        swarm_active, swarm_node_count,
                        &time_str,
                    )?;
                    Presenter::render_containers(
                        &docker_monitor.containers,
                        &docker_monitor.ui_state,
                        &docker_monitor.status_message,
                    )?;
                }
                AppView::ContainerLogs(_) => {
                    if let Some(ref log_state) = docker_monitor.log_state {
                        Presenter::render_logs(log_state)?;
                    }
                }
                AppView::Swarm | AppView::SwarmServiceTasks(_, _) => {
                    let mut out = io::stdout();
                    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
                    Presenter::render_tab_bar(
                        &mut out, &app_view,
                        docker_monitor.is_available(),
                        docker_monitor.containers.len(),
                        swarm_active, swarm_node_count,
                        &time_str,
                    )?;
                    match &swarm_monitor.ui_state.view_level {
                        SwarmViewLevel::Overview => {
                            Presenter::render_swarm_overview(
                                &swarm_monitor.cluster_info,
                                &swarm_monitor.nodes,
                                &swarm_monitor.stacks,
                                &swarm_monitor.services,
                                &swarm_monitor.ui_state,
                                &swarm_monitor.warnings,
                                &swarm_monitor.status_message,
                            )?;
                        }
                        SwarmViewLevel::ServiceTasks(_, name) => {
                            Presenter::render_swarm_tasks(
                                name,
                                &swarm_monitor.tasks,
                                swarm_monitor.ui_state.selected_index,
                                &swarm_monitor.status_message,
                            )?;
                        }
                        SwarmViewLevel::ServiceLogs(_, _) => {
                            // Handled below
                        }
                    }
                }
                AppView::SwarmServiceLogs(_, _) => {
                    if let Some(ref log_state) = swarm_monitor.log_state {
                        Presenter::render_service_logs(log_state)?;
                    }
                }
            }
            // Render confirmation banner if there's a pending action
            if let Some(ref pa) = pending_action {
                Presenter::render_confirmation(&pa.description)?;
            }

            needs_render = false;
        }

        // --- Input handling ---
        let timeout = tick_rate.saturating_sub(now.elapsed());
        if event::poll(timeout.min(Duration::from_millis(100)))? {
            if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
                // Global: Ctrl+C always quits
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }

                // Handle pending confirmation
                if pending_action.is_some() {
                    if code == KeyCode::Char('y') || code == KeyCode::Char('Y') {
                        let pa = pending_action.take().unwrap();
                        match pa.kind {
                            PendingActionKind::ContainerStart(id) => {
                                docker_monitor.start_container(&id);
                            }
                            PendingActionKind::ContainerStop(id) => {
                                docker_monitor.stop_container(&id);
                            }
                            PendingActionKind::ContainerRestart(id) => {
                                docker_monitor.restart_container(&id);
                            }
                            PendingActionKind::SwarmRollingRestart(id) => {
                                swarm_monitor.force_restart_service(&id);
                            }
                        }
                    } else {
                        pending_action = None;
                    }
                    needs_render = true;
                    continue;
                }

                // Helper: compute next/prev tab
                let next_tab = |current: &AppView| -> AppView {
                    match current {
                        AppView::System => {
                            if docker_monitor.is_available() {
                                AppView::Containers
                            } else if swarm_monitor.is_swarm() {
                                AppView::Swarm
                            } else {
                                AppView::System
                            }
                        }
                        AppView::Containers | AppView::ContainerLogs(_) => {
                            if swarm_monitor.is_swarm() {
                                AppView::Swarm
                            } else {
                                AppView::System
                            }
                        }
                        AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                            AppView::System
                        }
                    }
                };
                let prev_tab = |current: &AppView| -> AppView {
                    match current {
                        AppView::System => {
                            if swarm_monitor.is_swarm() {
                                AppView::Swarm
                            } else if docker_monitor.is_available() {
                                AppView::Containers
                            } else {
                                AppView::System
                            }
                        }
                        AppView::Containers | AppView::ContainerLogs(_) => {
                            AppView::System
                        }
                        AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
                            if docker_monitor.is_available() {
                                AppView::Containers
                            } else {
                                AppView::System
                            }
                        }
                    }
                };

                match &app_view {
                    // ==========================================================
                    // System view
                    // ==========================================================
                    AppView::System => {
                        match code {
                            KeyCode::Char('q') => break,
                            KeyCode::Tab => {
                                app_view = next_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::BackTab => {
                                app_view = prev_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if monitor.ui_state.selected_index > 0 {
                                    monitor.ui_state.selected_index -= 1;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                if monitor.ui_state.selected_index + 1 < monitor.ui_state.total_rows {
                                    monitor.ui_state.selected_index += 1;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Right => {
                                if monitor.ui_state.selected_index < row_mapping.len() {
                                    let (pid, kind) = row_mapping[monitor.ui_state.selected_index];
                                    match kind {
                                        RowKind::SectionHeader(section_id) => {
                                            if monitor.layout.is_collapsed(section_id) {
                                                monitor.layout.toggle_section(section_id);
                                                needs_render = true;
                                            }
                                        }
                                        RowKind::ProcessParent => {
                                            monitor.ui_state.expanded_pids.insert(pid);
                                            needs_render = true;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            KeyCode::Left => {
                                if monitor.ui_state.selected_index < row_mapping.len() {
                                    let (pid, kind) = row_mapping[monitor.ui_state.selected_index];
                                    match kind {
                                        RowKind::SectionHeader(section_id) => {
                                            if !monitor.layout.is_collapsed(section_id) {
                                                monitor.layout.toggle_section(section_id);
                                                needs_render = true;
                                            }
                                        }
                                        RowKind::ProcessParent => {
                                            monitor.ui_state.expanded_pids.remove(&pid);
                                            needs_render = true;
                                        }
                                        RowKind::ProcessChild => {
                                            let mut idx = monitor.ui_state.selected_index;
                                            while idx > 0 {
                                                idx -= 1;
                                                if row_mapping[idx].1 == RowKind::ProcessParent {
                                                    monitor.ui_state.expanded_pids.remove(&row_mapping[idx].0);
                                                    monitor.ui_state.selected_index = idx;
                                                    needs_render = true;
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('c') => {
                                monitor.ui_state.sort_column = SortColumn::Cpu;
                                needs_render = true;
                            }
                            KeyCode::Char('m') => {
                                monitor.ui_state.sort_column = SortColumn::Memory;
                                needs_render = true;
                            }
                            KeyCode::Char('r') => {
                                monitor.ui_state.sort_column = SortColumn::Read;
                                needs_render = true;
                            }
                            KeyCode::Char('w') => {
                                monitor.ui_state.sort_column = SortColumn::Write;
                                needs_render = true;
                            }
                            KeyCode::Char('d') => {
                                monitor.ui_state.sort_column = SortColumn::NetDown;
                                needs_render = true;
                            }
                            KeyCode::Char('u') => {
                                monitor.ui_state.sort_column = SortColumn::NetUp;
                                needs_render = true;
                            }
                            _ => {}
                        }
                    }

                    // ==========================================================
                    // Container list view
                    // ==========================================================
                    AppView::Containers => {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                app_view = AppView::System;
                                needs_render = true;
                            }
                            KeyCode::Tab => {
                                app_view = next_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::BackTab => {
                                app_view = prev_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if docker_monitor.ui_state.selected_index > 0 {
                                    docker_monitor.ui_state.selected_index -= 1;
                                    docker_monitor.status_message = None;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                if docker_monitor.ui_state.selected_index + 1 < docker_monitor.containers.len() {
                                    docker_monitor.ui_state.selected_index += 1;
                                    docker_monitor.status_message = None;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Right => {
                                // Enter log view for selected container
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    docker_monitor.start_log_stream(&c.id, &c.name);
                                    app_view = AppView::ContainerLogs(c.id.clone());
                                    needs_render = true;
                                }
                            }
                            KeyCode::Left => {
                                // Toggle expand/collapse for selected container detail
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    if docker_monitor.ui_state.expanded_ids.contains(&c.id) {
                                        docker_monitor.ui_state.expanded_ids.remove(&c.id);
                                    } else {
                                        docker_monitor.ui_state.expanded_ids.insert(c.id);
                                    }
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('S') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    pending_action = Some(PendingAction {
                                        description: format!("Start container '{}'?", c.name),
                                        kind: PendingActionKind::ContainerStart(c.id),
                                        expires: Instant::now() + Duration::from_secs(5),
                                    });
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('T') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    pending_action = Some(PendingAction {
                                        description: format!("Stop container '{}'?", c.name),
                                        kind: PendingActionKind::ContainerStop(c.id),
                                        expires: Instant::now() + Duration::from_secs(5),
                                    });
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('R') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    pending_action = Some(PendingAction {
                                        description: format!("Restart container '{}'?", c.name),
                                        kind: PendingActionKind::ContainerRestart(c.id),
                                        expires: Instant::now() + Duration::from_secs(5),
                                    });
                                    needs_render = true;
                                }
                            }
                            _ => {}
                        }
                    }

                    // ==========================================================
                    // Log viewer (full screen)
                    // ==========================================================
                    AppView::ContainerLogs(_) => {
                        // Handle search mode input first
                        if docker_monitor.log_state.as_ref().map_or(false, |s| s.search_mode) {
                            match code {
                                KeyCode::Enter => {
                                    if let Some(ref mut log_state) = docker_monitor.log_state {
                                        log_state.search_mode = false;
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Esc => {
                                    if let Some(ref mut log_state) = docker_monitor.log_state {
                                        log_state.search_mode = false;
                                        log_state.search_query.clear();
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let Some(ref mut log_state) = docker_monitor.log_state {
                                        log_state.search_query.pop();
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Char(c) => {
                                    if let Some(ref mut log_state) = docker_monitor.log_state {
                                        log_state.search_query.push(c);
                                        needs_render = true;
                                    }
                                }
                                _ => {}
                            }
                        } else {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
                                docker_monitor.stop_log_stream();
                                app_view = AppView::Containers;
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    log_state.auto_follow = false;
                                    let max_offset = log_state.lines.len().saturating_sub(1);
                                    if log_state.scroll_offset < max_offset {
                                        log_state.scroll_offset += 1;
                                    }
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    if log_state.scroll_offset > 0 {
                                        log_state.scroll_offset -= 1;
                                        if log_state.scroll_offset == 0 {
                                            log_state.auto_follow = true;
                                        }
                                    }
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('f') | KeyCode::End => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    log_state.auto_follow = true;
                                    log_state.scroll_offset = 0;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('/') => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    log_state.search_mode = true;
                                    log_state.search_query.clear();
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('n') => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    log_state.search_query.clear();
                                    needs_render = true;
                                }
                            }
                            KeyCode::PageUp => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    log_state.auto_follow = false;
                                    let page_size = crossterm::terminal::size()
                                        .map(|(_, h)| h as usize)
                                        .unwrap_or(24)
                                        .saturating_sub(4);
                                    let max_offset = log_state.lines.len().saturating_sub(1);
                                    log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
                                    needs_render = true;
                                }
                            }
                            KeyCode::PageDown => {
                                if let Some(ref mut log_state) = docker_monitor.log_state {
                                    let page_size = crossterm::terminal::size()
                                        .map(|(_, h)| h as usize)
                                        .unwrap_or(24)
                                        .saturating_sub(4);
                                    if log_state.scroll_offset > page_size {
                                        log_state.scroll_offset -= page_size;
                                    } else {
                                        log_state.scroll_offset = 0;
                                        log_state.auto_follow = true;
                                    }
                                    needs_render = true;
                                }
                            }
                            _ => {}
                        }
                        }
                    }

                    // ==========================================================
                    // Swarm overview
                    // ==========================================================
                    AppView::Swarm => {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                app_view = AppView::System;
                                needs_render = true;
                            }
                            KeyCode::Tab => {
                                app_view = next_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::BackTab => {
                                app_view = prev_tab(&app_view);
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if swarm_monitor.ui_state.selected_index > 0 {
                                    swarm_monitor.ui_state.selected_index -= 1;
                                    swarm_monitor.status_message = None;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                let max = swarm_monitor.overview_row_count();
                                if swarm_monitor.ui_state.selected_index + 1 < max {
                                    swarm_monitor.ui_state.selected_index += 1;
                                    swarm_monitor.status_message = None;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Right => {
                                // Determine what's at the current selection
                                let sel = swarm_monitor.ui_state.selected_index;
                                let item = resolve_swarm_overview_item(&swarm_monitor, sel);
                                match item {
                                    SwarmOverviewItem::NodesHeader => {
                                        swarm_monitor.ui_state.expanded_ids.insert("__nodes__".to_string());
                                        needs_render = true;
                                    }
                                    SwarmOverviewItem::StackHeader(name) => {
                                        swarm_monitor.ui_state.expanded_ids.insert(name);
                                        needs_render = true;
                                    }
                                    SwarmOverviewItem::Service(id, name) => {
                                        swarm_monitor.enter_task_view(&id, &name);
                                        app_view = AppView::SwarmServiceTasks(id, name);
                                        needs_render = true;
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Left => {
                                let sel = swarm_monitor.ui_state.selected_index;
                                let item = resolve_swarm_overview_item(&swarm_monitor, sel);
                                match item {
                                    SwarmOverviewItem::NodesHeader => {
                                        swarm_monitor.ui_state.expanded_ids.remove("__nodes__");
                                        needs_render = true;
                                    }
                                    SwarmOverviewItem::StackHeader(name) => {
                                        swarm_monitor.ui_state.expanded_ids.remove(&name);
                                        needs_render = true;
                                    }
                                    SwarmOverviewItem::Node => {
                                        // Collapse nodes section and jump to header
                                        swarm_monitor.ui_state.expanded_ids.remove("__nodes__");
                                        swarm_monitor.ui_state.selected_index = 0;
                                        needs_render = true;
                                    }
                                    SwarmOverviewItem::Service(_, _) => {
                                        // No collapse action for individual services
                                    }
                                    SwarmOverviewItem::None => {}
                                }
                            }
                            KeyCode::Char('R') => {
                                let sel = swarm_monitor.ui_state.selected_index;
                                let item = resolve_swarm_overview_item(&swarm_monitor, sel);
                                if let SwarmOverviewItem::Service(id, name) = item {
                                    pending_action = Some(PendingAction {
                                        description: format!("Rolling restart service '{}'?", name),
                                        kind: PendingActionKind::SwarmRollingRestart(id),
                                        expires: Instant::now() + Duration::from_secs(5),
                                    });
                                    needs_render = true;
                                }
                            }
                            _ => {}
                        }
                    }

                    // ==========================================================
                    // Swarm task/replica list
                    // ==========================================================
                    AppView::SwarmServiceTasks(_, _) => {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
                                swarm_monitor.go_back();
                                app_view = AppView::Swarm;
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if swarm_monitor.ui_state.selected_index > 0 {
                                    swarm_monitor.ui_state.selected_index -= 1;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                if swarm_monitor.ui_state.selected_index + 1 < swarm_monitor.tasks.len() {
                                    swarm_monitor.ui_state.selected_index += 1;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => {
                                // Enter service logs
                                if let SwarmViewLevel::ServiceTasks(ref svc_id, ref svc_name) = swarm_monitor.ui_state.view_level.clone() {
                                    swarm_monitor.start_service_log_stream(svc_id, svc_name);
                                    app_view = AppView::SwarmServiceLogs(svc_id.clone(), svc_name.clone());
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('R') => {
                                if let SwarmViewLevel::ServiceTasks(ref svc_id, ref svc_name) = swarm_monitor.ui_state.view_level.clone() {
                                    pending_action = Some(PendingAction {
                                        description: format!("Rolling restart service '{}'?", svc_name),
                                        kind: PendingActionKind::SwarmRollingRestart(svc_id.clone()),
                                        expires: Instant::now() + Duration::from_secs(5),
                                    });
                                    needs_render = true;
                                }
                            }
                            _ => {}
                        }
                    }

                    // ==========================================================
                    // Service log viewer (full screen)
                    // ==========================================================
                    AppView::SwarmServiceLogs(_, _) => {
                        // Handle search mode input first
                        if swarm_monitor.log_state.as_ref().map_or(false, |s| s.search_mode) {
                            match code {
                                KeyCode::Enter => {
                                    if let Some(ref mut log_state) = swarm_monitor.log_state {
                                        log_state.search_mode = false;
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Esc => {
                                    if let Some(ref mut log_state) = swarm_monitor.log_state {
                                        log_state.search_mode = false;
                                        log_state.search_query.clear();
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let Some(ref mut log_state) = swarm_monitor.log_state {
                                        log_state.search_query.pop();
                                        needs_render = true;
                                    }
                                }
                                KeyCode::Char(c) => {
                                    if let Some(ref mut log_state) = swarm_monitor.log_state {
                                        log_state.search_query.push(c);
                                        needs_render = true;
                                    }
                                }
                                _ => {}
                            }
                        } else {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
                                swarm_monitor.stop_log_stream();
                                swarm_monitor.ui_state.view_level = SwarmViewLevel::Overview;
                                app_view = AppView::Swarm;
                                needs_render = true;
                            }
                            KeyCode::Up => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.auto_follow = false;
                                    let max_offset = log_state.lines.len().saturating_sub(1);
                                    if log_state.scroll_offset < max_offset {
                                        log_state.scroll_offset += 1;
                                    }
                                    needs_render = true;
                                }
                            }
                            KeyCode::Down => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    if log_state.scroll_offset > 0 {
                                        log_state.scroll_offset -= 1;
                                        if log_state.scroll_offset == 0 {
                                            log_state.auto_follow = true;
                                        }
                                    }
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('f') | KeyCode::End => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.auto_follow = true;
                                    log_state.scroll_offset = 0;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('e') => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.filter_errors = !log_state.filter_errors;
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('/') => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.search_mode = true;
                                    log_state.search_query.clear();
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('n') => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.search_query.clear();
                                    needs_render = true;
                                }
                            }
                            KeyCode::PageUp => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    log_state.auto_follow = false;
                                    let page_size = crossterm::terminal::size()
                                        .map(|(_, h)| h as usize)
                                        .unwrap_or(24)
                                        .saturating_sub(4);
                                    let max_offset = log_state.lines.len().saturating_sub(1);
                                    log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
                                    needs_render = true;
                                }
                            }
                            KeyCode::PageDown => {
                                if let Some(ref mut log_state) = swarm_monitor.log_state {
                                    let page_size = crossterm::terminal::size()
                                        .map(|(_, h)| h as usize)
                                        .unwrap_or(24)
                                        .saturating_sub(4);
                                    if log_state.scroll_offset > page_size {
                                        log_state.scroll_offset -= page_size;
                                    } else {
                                        log_state.scroll_offset = 0;
                                        log_state.auto_follow = true;
                                    }
                                    needs_render = true;
                                }
                            }
                            _ => {}
                        }
                        }
                    }
                }
            }
        }
    }

    restore_terminal();
    Ok(())
}

/// What kind of item is at a given row index in the Swarm overview
enum SwarmOverviewItem {
    NodesHeader,
    Node,
    StackHeader(String),
    Service(String, String), // (service_id, service_name)
    None,
}

/// Resolve which item is at the given row index in the Swarm overview.
fn resolve_swarm_overview_item(
    monitor: &SwarmMonitor,
    selected: usize,
) -> SwarmOverviewItem {
    let mut row_idx: usize = 0;

    // Nodes header
    if selected == row_idx {
        return SwarmOverviewItem::NodesHeader;
    }
    row_idx += 1;

    // Nodes (if expanded)
    if monitor.ui_state.expanded_ids.contains("__nodes__") {
        for _node in &monitor.nodes {
            if selected == row_idx {
                return SwarmOverviewItem::Node;
            }
            row_idx += 1;
        }
    }

    // Stacks
    for stack in &monitor.stacks {
        if selected == row_idx {
            return SwarmOverviewItem::StackHeader(stack.name.clone());
        }
        row_idx += 1;

        if monitor.ui_state.expanded_ids.contains(&stack.name) {
            for &idx in &stack.service_indices {
                if selected == row_idx {
                    let svc = &monitor.services[idx];
                    return SwarmOverviewItem::Service(svc.id.clone(), svc.name.clone());
                }
                row_idx += 1;
            }
        }
    }

    SwarmOverviewItem::None
}
