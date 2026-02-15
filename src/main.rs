pub mod model;
pub mod view;
pub mod controller;
pub mod layout;
pub mod collectors;
pub mod docker;
pub mod docker_controller;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossterm::{
    execute,
    event::{self, Event, KeyCode, KeyModifiers, KeyEvent},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, Clear, ClearType},
};
use controller::Monitor;
use docker_controller::DockerMonitor;
use model::{AppView, SortColumn};
use view::{Presenter, RowKind};
use sysinfo::Pid;

fn main() -> io::Result<()> {
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

    let tick_rate = Duration::from_secs(3);
    let mut last_tick = Instant::now() - tick_rate;
    let mut row_mapping: Vec<(Pid, RowKind)> = Vec::new();
    let mut needs_render = true;
    let mut app_view = AppView::System;

    loop {
        let now = Instant::now();

        // --- Tick-based data refresh (every 3 seconds) ---
        if now.duration_since(last_tick) >= tick_rate {
            monitor.update();
            if docker_monitor.is_available() {
                docker_monitor.update();
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

        // --- Render ---
        if needs_render {
            let time_str = monitor.last_data.as_ref()
                .map(|d| d.time.clone())
                .unwrap_or_else(|| "...".to_string());

            match &app_view {
                AppView::System => {
                    let mut out = io::stdout();
                    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
                    Presenter::render_tab_bar(
                        &mut out,
                        &app_view,
                        docker_monitor.is_available(),
                        docker_monitor.containers.len(),
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
                        &mut out,
                        &app_view,
                        docker_monitor.is_available(),
                        docker_monitor.containers.len(),
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

                match &app_view {
                    // ==========================================================
                    // System view
                    // ==========================================================
                    AppView::System => {
                        match code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Tab => {
                                if docker_monitor.is_available() {
                                    app_view = AppView::Containers;
                                    needs_render = true;
                                }
                            }
                            KeyCode::BackTab => {
                                if docker_monitor.is_available() {
                                    app_view = AppView::Containers;
                                    needs_render = true;
                                }
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
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Tab => {
                                app_view = AppView::System;
                                needs_render = true;
                            }
                            KeyCode::BackTab => {
                                app_view = AppView::System;
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
                            KeyCode::Char('s') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    docker_monitor.start_container(&c.id);
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('t') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    docker_monitor.stop_container(&c.id);
                                    needs_render = true;
                                }
                            }
                            KeyCode::Char('r') => {
                                if let Some(c) = docker_monitor.selected_container().cloned() {
                                    docker_monitor.restart_container(&c.id);
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
                        match code {
                            KeyCode::Char('q') => break,
                            KeyCode::Esc | KeyCode::Left => {
                                // Go back to container list
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
            }
        }
    }

    execute!(stdout, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
