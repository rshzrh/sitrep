use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::{AppView, SortColumn, SwarmViewLevel};
use crate::view::RowKind;

use super::state::{PendingAction, PendingActionKind, resolve_swarm_overview_item};
use super::App;

/// Result of handling a key: Quit the app, or key was consumed (needs render).
/// None means the key was not handled.
pub enum InputResult {
    Quit,
    Consumed,
}

/// Handle a key event. Returns Some(Quit) to exit, Some(Consumed) if key was handled and
/// a render is needed, None if the key was not handled.
pub fn handle_key(app: &mut App, key_event: KeyEvent) -> Option<InputResult> {
    let KeyEvent { code, modifiers, .. } = key_event;

    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return Some(InputResult::Quit);
    }

    if app.pending_action.is_some() {
        if code == KeyCode::Char('y') || code == KeyCode::Char('Y') {
            let pa = app.pending_action.take().unwrap();
            match pa.kind {
                PendingActionKind::ContainerStart(id) => {
                    app.docker_monitor.start_container(&id);
                }
                PendingActionKind::ContainerStop(id) => {
                    app.docker_monitor.stop_container(&id);
                }
                PendingActionKind::ContainerRestart(id) => {
                    app.docker_monitor.restart_container(&id);
                }
                PendingActionKind::SwarmRollingRestart(id) => {
                    app.swarm_monitor.force_restart_service(&id);
                }
            }
        } else {
            app.pending_action = None;
        }
        return Some(InputResult::Consumed);
    }

    let next_tab = next_tab(app);
    let prev_tab = prev_tab(app);

    let result = match &app.app_view {
        AppView::System => handle_system(app, code, next_tab, prev_tab),
        AppView::Containers => handle_containers(app, code, next_tab, prev_tab),
        AppView::ContainerLogs(_) => handle_container_logs(app, code),
        AppView::Swarm => handle_swarm(app, code, next_tab, prev_tab),
        AppView::SwarmServiceTasks(_, _) => handle_swarm_tasks(app, code),
        AppView::SwarmServiceLogs(_, _) => handle_service_logs(app, code),
    };

    if let Some(InputResult::Quit) = result {
        return Some(InputResult::Quit);
    }
    if result.is_some() {
        return Some(InputResult::Consumed);
    }
    None
}

fn next_tab(app: &App) -> AppView {
    match &app.app_view {
        AppView::System => {
            if app.docker_monitor.is_available() {
                AppView::Containers
            } else if app.swarm_monitor.is_swarm() {
                AppView::Swarm
            } else {
                AppView::System
            }
        }
        AppView::Containers | AppView::ContainerLogs(_) => {
            if app.swarm_monitor.is_swarm() {
                AppView::Swarm
            } else {
                AppView::System
            }
        }
        AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
            AppView::System
        }
    }
}

fn prev_tab(app: &App) -> AppView {
    match &app.app_view {
        AppView::System => {
            if app.swarm_monitor.is_swarm() {
                AppView::Swarm
            } else if app.docker_monitor.is_available() {
                AppView::Containers
            } else {
                AppView::System
            }
        }
        AppView::Containers | AppView::ContainerLogs(_) => AppView::System,
        AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _) => {
            if app.docker_monitor.is_available() {
                AppView::Containers
            } else {
                AppView::System
            }
        }
    }
}

fn handle_system(
    app: &mut App,
    code: KeyCode,
    next_tab: AppView,
    prev_tab: AppView,
) -> Option<InputResult> {
    match code {
        KeyCode::Char('q') => return Some(InputResult::Quit),
        KeyCode::Tab => {
            app.app_view = next_tab;
            return Some(InputResult::Consumed);
        }
        KeyCode::BackTab => {
            app.app_view = prev_tab;
            return Some(InputResult::Consumed);
        }
        KeyCode::Up => {
            if app.monitor.ui_state.selected_index > 0 {
                app.monitor.ui_state.selected_index -= 1;
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Down => {
            if app.monitor.ui_state.selected_index + 1 < app.monitor.ui_state.total_rows {
                app.monitor.ui_state.selected_index += 1;
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Right => {
            if app.monitor.ui_state.selected_index < app.row_mapping.len() {
                let (pid, kind) = app.row_mapping[app.monitor.ui_state.selected_index];
                match kind {
                    RowKind::SectionHeader(section_id) => {
                        if app.monitor.layout.is_collapsed(section_id) {
                            app.monitor.layout.toggle_section(section_id);
                            return Some(InputResult::Consumed);
                        }
                    }
                    RowKind::ProcessParent => {
                        app.monitor.ui_state.expanded_pids.insert(pid);
                        return Some(InputResult::Consumed);
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Left => {
            if app.monitor.ui_state.selected_index < app.row_mapping.len() {
                let (pid, kind) = app.row_mapping[app.monitor.ui_state.selected_index];
                match kind {
                    RowKind::SectionHeader(section_id) => {
                        if !app.monitor.layout.is_collapsed(section_id) {
                            app.monitor.layout.toggle_section(section_id);
                            return Some(InputResult::Consumed);
                        }
                    }
                    RowKind::ProcessParent => {
                        app.monitor.ui_state.expanded_pids.remove(&pid);
                        return Some(InputResult::Consumed);
                    }
                    RowKind::ProcessChild => {
                        let mut idx = app.monitor.ui_state.selected_index;
                        while idx > 0 {
                            idx -= 1;
                            if app.row_mapping[idx].1 == RowKind::ProcessParent {
                                app.monitor.ui_state.expanded_pids.remove(&app.row_mapping[idx].0);
                                app.monitor.ui_state.selected_index = idx;
                                return Some(InputResult::Consumed);
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Char('c') => {
            app.monitor.ui_state.sort_column = SortColumn::Cpu;
            return Some(InputResult::Consumed);
        }
        KeyCode::Char('m') => {
            app.monitor.ui_state.sort_column = SortColumn::Memory;
            return Some(InputResult::Consumed);
        }
        KeyCode::Char('r') => {
            app.monitor.ui_state.sort_column = SortColumn::Read;
            return Some(InputResult::Consumed);
        }
        KeyCode::Char('w') => {
            app.monitor.ui_state.sort_column = SortColumn::Write;
            return Some(InputResult::Consumed);
        }
        KeyCode::Char('d') => {
            app.monitor.ui_state.sort_column = SortColumn::NetDown;
            return Some(InputResult::Consumed);
        }
        KeyCode::Char('u') => {
            app.monitor.ui_state.sort_column = SortColumn::NetUp;
            return Some(InputResult::Consumed);
        }
        _ => {}
    }
    None
}

fn handle_containers(
    app: &mut App,
    code: KeyCode,
    next_tab: AppView,
    prev_tab: AppView,
) -> Option<InputResult> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.app_view = AppView::System;
            return Some(InputResult::Consumed);
        }
        KeyCode::Tab => {
            app.app_view = next_tab;
            return Some(InputResult::Consumed);
        }
        KeyCode::BackTab => {
            app.app_view = prev_tab;
            return Some(InputResult::Consumed);
        }
        KeyCode::Up => {
            if app.docker_monitor.ui_state.selected_index > 0 {
                app.docker_monitor.ui_state.selected_index -= 1;
                app.docker_monitor.status_message = None;
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Down => {
            if app.docker_monitor.ui_state.selected_index + 1 < app.docker_monitor.containers.len() {
                app.docker_monitor.ui_state.selected_index += 1;
                app.docker_monitor.status_message = None;
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Right => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                app.docker_monitor.start_log_stream(&c.id, &c.name);
                app.app_view = AppView::ContainerLogs(c.id.clone());
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Left => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                if app.docker_monitor.ui_state.expanded_ids.contains(&c.id) {
                    app.docker_monitor.ui_state.expanded_ids.remove(&c.id);
                } else {
                    app.docker_monitor.ui_state.expanded_ids.insert(c.id);
                }
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Char('S') => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                app.pending_action = Some(PendingAction {
                    description: format!("Start container '{}'?", c.name),
                    kind: PendingActionKind::ContainerStart(c.id),
                    expires: Instant::now() + Duration::from_secs(5),
                });
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Char('T') => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                app.pending_action = Some(PendingAction {
                    description: format!("Stop container '{}'?", c.name),
                    kind: PendingActionKind::ContainerStop(c.id),
                    expires: Instant::now() + Duration::from_secs(5),
                });
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Char('R') => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                app.pending_action = Some(PendingAction {
                    description: format!("Restart container '{}'?", c.name),
                    kind: PendingActionKind::ContainerRestart(c.id),
                    expires: Instant::now() + Duration::from_secs(5),
                });
                return Some(InputResult::Consumed);
            }
        }
        _ => {}
    }
    None
}

fn handle_container_logs(app: &mut App, code: KeyCode) -> Option<InputResult> {
    let page_size = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24)
        .saturating_sub(4);

    if app.docker_monitor.log_state.as_ref().map_or(false, |s| s.search_mode) {
        return match code {
            KeyCode::Enter => {
                if let Some(ref mut log_state) = app.docker_monitor.log_state {
                    log_state.search_mode = false;
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Esc => {
                if let Some(ref mut log_state) = app.docker_monitor.log_state {
                    log_state.search_mode = false;
                    log_state.search_query.clear();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Backspace => {
                if let Some(ref mut log_state) = app.docker_monitor.log_state {
                    log_state.search_query.pop();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Char(c) => {
                if let Some(ref mut log_state) = app.docker_monitor.log_state {
                    log_state.search_query.push(c);
                }
                Some(InputResult::Consumed)
            }
            _ => None,
        };
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
            app.docker_monitor.stop_log_stream();
            app.app_view = AppView::Containers;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                if log_state.scroll_offset < max_offset {
                    log_state.scroll_offset += 1;
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Down => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                if log_state.scroll_offset > 0 {
                    log_state.scroll_offset -= 1;
                    if log_state.scroll_offset == 0 {
                        log_state.auto_follow = true;
                    }
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('f') | KeyCode::End => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                log_state.auto_follow = true;
                log_state.scroll_offset = 0;
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('/') => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                log_state.search_mode = true;
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('n') => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageUp => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageDown => {
            if let Some(ref mut log_state) = app.docker_monitor.log_state {
                if log_state.scroll_offset > page_size {
                    log_state.scroll_offset -= page_size;
                } else {
                    log_state.scroll_offset = 0;
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_swarm(
    app: &mut App,
    code: KeyCode,
    next_tab: AppView,
    prev_tab: AppView,
) -> Option<InputResult> {
    let sel = app.swarm_monitor.ui_state.selected_index;
    let item = resolve_swarm_overview_item(&app.swarm_monitor, sel);

    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.app_view = AppView::System;
            Some(InputResult::Consumed)
        }
        KeyCode::Tab => {
            app.app_view = next_tab;
            Some(InputResult::Consumed)
        }
        KeyCode::BackTab => {
            app.app_view = prev_tab;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if app.swarm_monitor.ui_state.selected_index > 0 {
                app.swarm_monitor.ui_state.selected_index -= 1;
                app.swarm_monitor.status_message = None;
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        KeyCode::Down => {
            let max = app.swarm_monitor.overview_row_count();
            if app.swarm_monitor.ui_state.selected_index + 1 < max {
                app.swarm_monitor.ui_state.selected_index += 1;
                app.swarm_monitor.status_message = None;
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        KeyCode::Right => {
            match item {
                super::state::SwarmOverviewItem::NodesHeader => {
                    app.swarm_monitor.ui_state.expanded_ids.insert("__nodes__".to_string());
                    Some(InputResult::Consumed)
                }
                super::state::SwarmOverviewItem::StackHeader(name) => {
                    app.swarm_monitor.ui_state.expanded_ids.insert(name);
                    Some(InputResult::Consumed)
                }
                super::state::SwarmOverviewItem::Service(id, name) => {
                    app.swarm_monitor.enter_task_view(&id, &name);
                    app.app_view = AppView::SwarmServiceTasks(id, name);
                    Some(InputResult::Consumed)
                }
                _ => None,
            }
        }
        KeyCode::Left => {
            match item {
                super::state::SwarmOverviewItem::NodesHeader => {
                    app.swarm_monitor.ui_state.expanded_ids.remove("__nodes__");
                    Some(InputResult::Consumed)
                }
                super::state::SwarmOverviewItem::StackHeader(name) => {
                    app.swarm_monitor.ui_state.expanded_ids.remove(&name);
                    Some(InputResult::Consumed)
                }
                super::state::SwarmOverviewItem::Node => {
                    app.swarm_monitor.ui_state.expanded_ids.remove("__nodes__");
                    app.swarm_monitor.ui_state.selected_index = 0;
                    Some(InputResult::Consumed)
                }
                _ => None,
            }
        }
        KeyCode::Char('R') => {
            if let super::state::SwarmOverviewItem::Service(id, name) = item {
                app.pending_action = Some(PendingAction {
                    description: format!("Rolling restart service '{}'?", name),
                    kind: PendingActionKind::SwarmRollingRestart(id),
                    expires: Instant::now() + Duration::from_secs(5),
                });
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn handle_swarm_tasks(app: &mut App, code: KeyCode) -> Option<InputResult> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
            app.swarm_monitor.go_back();
            app.app_view = AppView::Swarm;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if app.swarm_monitor.ui_state.selected_index > 0 {
                app.swarm_monitor.ui_state.selected_index -= 1;
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        KeyCode::Down => {
            if app.swarm_monitor.ui_state.selected_index + 1 < app.swarm_monitor.tasks.len() {
                app.swarm_monitor.ui_state.selected_index += 1;
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') => {
            if let SwarmViewLevel::ServiceTasks(ref svc_id, ref svc_name) =
                app.swarm_monitor.ui_state.view_level.clone()
            {
                app.swarm_monitor.start_service_log_stream(svc_id, svc_name);
                app.app_view = AppView::SwarmServiceLogs(svc_id.clone(), svc_name.clone());
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        KeyCode::Char('R') => {
            if let SwarmViewLevel::ServiceTasks(ref svc_id, ref svc_name) =
                app.swarm_monitor.ui_state.view_level.clone()
            {
                app.pending_action = Some(PendingAction {
                    description: format!("Rolling restart service '{}'?", svc_name),
                    kind: PendingActionKind::SwarmRollingRestart(svc_id.clone()),
                    expires: Instant::now() + Duration::from_secs(5),
                });
                Some(InputResult::Consumed)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn handle_service_logs(app: &mut App, code: KeyCode) -> Option<InputResult> {
    let page_size = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24)
        .saturating_sub(4);

    if app.swarm_monitor.log_state.as_ref().map_or(false, |s| s.search_mode) {
        return match code {
            KeyCode::Enter => {
                if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                    log_state.search_mode = false;
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Esc => {
                if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                    log_state.search_mode = false;
                    log_state.search_query.clear();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Backspace => {
                if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                    log_state.search_query.pop();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Char(c) => {
                if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                    log_state.search_query.push(c);
                }
                Some(InputResult::Consumed)
            }
            _ => None,
        };
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
            app.swarm_monitor.stop_log_stream();
            app.swarm_monitor.ui_state.view_level = SwarmViewLevel::Overview;
            app.app_view = AppView::Swarm;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                if log_state.scroll_offset < max_offset {
                    log_state.scroll_offset += 1;
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Down => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                if log_state.scroll_offset > 0 {
                    log_state.scroll_offset -= 1;
                    if log_state.scroll_offset == 0 {
                        log_state.auto_follow = true;
                    }
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('f') | KeyCode::End => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.auto_follow = true;
                log_state.scroll_offset = 0;
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('e') => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.filter_errors = !log_state.filter_errors;
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('/') => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.search_mode = true;
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('n') => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageUp => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageDown => {
            if let Some(ref mut log_state) = app.swarm_monitor.log_state {
                if log_state.scroll_offset > page_size {
                    log_state.scroll_offset -= page_size;
                } else {
                    log_state.scroll_offset = 0;
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}
