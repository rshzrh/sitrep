use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::{AppView, SortColumn, SwarmViewLevel, TabKind};
use crate::view::RowKind;

use super::state::{resolve_swarm_overview_item, PendingAction, PendingActionKind};
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
    let KeyEvent {
        code, modifiers, ..
    } = key_event;

    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return Some(InputResult::Quit);
    }

    if app.pending_action.is_some() {
        if code == KeyCode::Char('y') || code == KeyCode::Char('Y') {
            let pa = app.pending_action.take().unwrap();
            match pa.kind {
                // Local Docker actions
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
                // Remote Docker actions — dispatched over SSH via RemoteHost
                PendingActionKind::RemoteContainerStart { host_idx, id, .. } => {
                    if let Some(rh) = app.remote_hosts.get(host_idx) {
                        let cmd = crate::remote_host::RemoteHost::container_action_command("start", &id);
                        rh.run_action(Arc::clone(&app.rt), "start".into(), id, cmd);
                    }
                }
                PendingActionKind::RemoteContainerStop { host_idx, id, .. } => {
                    if let Some(rh) = app.remote_hosts.get(host_idx) {
                        let cmd = crate::remote_host::RemoteHost::container_action_command("stop", &id);
                        rh.run_action(Arc::clone(&app.rt), "stop".into(), id, cmd);
                    }
                }
                PendingActionKind::RemoteContainerRestart { host_idx, id, .. } => {
                    if let Some(rh) = app.remote_hosts.get(host_idx) {
                        let cmd = crate::remote_host::RemoteHost::container_action_command("restart", &id);
                        rh.run_action(Arc::clone(&app.rt), "restart".into(), id, cmd);
                    }
                }
                PendingActionKind::RemoteRollingRestart { host_idx, id, .. } => {
                    if let Some(rh) = app.remote_hosts.get(host_idx) {
                        let cmd = crate::remote_host::RemoteHost::swarm_rolling_restart_command(&id);
                        rh.run_action(Arc::clone(&app.rt), "rolling-restart".into(), id, cmd);
                    }
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
        AppView::ContainerLogsMulti(_) => handle_container_logs_multi(app, code),
        AppView::Swarm => handle_swarm(app, code, next_tab, prev_tab),
        AppView::SwarmServiceTasks(_, _) => handle_swarm_tasks(app, code),
        AppView::SwarmServiceLogs(_, _) => handle_service_logs(app, code),
        AppView::FleetOverview => handle_fleet_overview(app, code),
        AppView::Remote { .. } => handle_remote(app, code),
    };

    if let Some(InputResult::Quit) = result {
        return Some(InputResult::Quit);
    }
    if result.is_some() {
        return Some(InputResult::Consumed);
    }
    None
}

/// Build the ordered list of visible top-level tab kinds given what's
/// available right now. System is always present; Containers only if
/// Docker is reachable; Swarm only if swarm mode is detected. This is
/// the single source of truth for tab ordering — next_tab and prev_tab
/// both read from it. See A2-lite in the eng review.
fn visible_tab_kinds(app: &App) -> Vec<TabKind> {
    let mut kinds = vec![TabKind::System];
    if app.docker_monitor.is_available() {
        kinds.push(TabKind::Containers);
    }
    if app.swarm_monitor.is_swarm() {
        kinds.push(TabKind::Swarm);
    }
    kinds
}

/// Landing `AppView` for a given tab kind — i.e. the variant you end
/// up in when you Tab into that top-level tab fresh. Drill-in state
/// (ContainerLogs, SwarmServiceTasks, etc.) is discarded on tab
/// switch, which matches the previous behavior.
fn landing_view_for(kind: TabKind) -> AppView {
    match kind {
        TabKind::System => AppView::System,
        TabKind::Containers => AppView::Containers,
        TabKind::Swarm => AppView::Swarm,
        TabKind::Fleet => AppView::FleetOverview,
        // Remote has no standalone landing view (it's always
        // host-qualified). Tab cycling is a no-op inside Remote mode;
        // callers short-circuit before reaching here.
        TabKind::Remote => AppView::FleetOverview,
    }
}

fn next_tab(app: &App) -> AppView {
    // Fleet + Remote modes don't cycle — Tab is a no-op there.
    if matches!(app.app_view.tab_kind(), TabKind::Fleet | TabKind::Remote) {
        return app.app_view.clone();
    }
    let kinds = visible_tab_kinds(app);
    let current = app.app_view.tab_kind();
    let idx = kinds.iter().position(|k| *k == current).unwrap_or(0);
    landing_view_for(kinds[(idx + 1) % kinds.len()])
}

fn prev_tab(app: &App) -> AppView {
    if matches!(app.app_view.tab_kind(), TabKind::Fleet | TabKind::Remote) {
        return app.app_view.clone();
    }
    let kinds = visible_tab_kinds(app);
    let current = app.app_view.tab_kind();
    let idx = kinds.iter().position(|k| *k == current).unwrap_or(0);
    let n = kinds.len();
    landing_view_for(kinds[(idx + n - 1) % n])
}

/// Fleet overview keyboard handling. Up/Down navigate the host list,
/// Enter drills into the selected host's remote detail view, q quits.
fn handle_fleet_overview(app: &mut App, code: KeyCode) -> Option<InputResult> {
    use crossterm::event::KeyCode::*;
    let fs = app.fleet_state.as_mut()?;
    match code {
        Char('q') | Esc => Some(InputResult::Quit),
        Up => {
            fs.move_selection_up();
            Some(InputResult::Consumed)
        }
        Down => {
            fs.move_selection_down();
            Some(InputResult::Consumed)
        }
        Enter | Right => {
            let idx = fs.selected;
            fs.drill_in();
            app.app_view = AppView::Remote {
                host: idx,
                tab: crate::model::RemoteTab::System,
            };
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

/// Remote drill-in keyboard handling. Dispatches to per-tab sub-handlers.
/// Tab/Shift-Tab cycles tabs within remote mode. Esc/Left returns to the
/// fleet overview. q quits.
fn handle_remote(app: &mut App, code: KeyCode) -> Option<InputResult> {
    use crate::model::RemoteTab;
    use crossterm::event::KeyCode::*;

    // Pull host + tab out (cloned, not a borrow) so we can mutate app.
    let (host_idx, tab) = match &app.app_view {
        AppView::Remote { host, tab } => (*host, tab.clone()),
        _ => return None,
    };

    // Global keys that apply regardless of which remote tab is active.
    // `Left` acts as "back" in nested views (logs / swarm service tasks)
    // — matches the local binding. In the top-level tabs (System /
    // Containers / Swarm), Left is passed through to the sub-handler
    // because the containers tab uses Left to toggle expand/collapse.
    let is_back_key = matches!(code, Esc)
        || (matches!(code, Left)
            && matches!(
                tab,
                RemoteTab::ContainerLogs(_)
                    | RemoteTab::ContainerLogsMulti(_)
                    | RemoteTab::SwarmServiceTasks(_, _)
                    | RemoteTab::SwarmServiceLogs(_, _)
            ));
    match code {
        Char('q') => return Some(InputResult::Quit),
        _ if is_back_key => {
            // Esc / Left from a nested view (logs, service tasks) returns
            // one level. From a top-level tab, Esc returns to fleet.
            match tab {
                RemoteTab::System | RemoteTab::Containers | RemoteTab::Swarm => {
                    if let Some(ref mut fs) = app.fleet_state {
                        fs.drill_out();
                    }
                    // Clean up per-host render state on drill-out so it
                    // doesn't grow unboundedly across repeated drill-ins.
                    if let Some(rh) = app.remote_hosts.get_mut(host_idx) {
                        rh.row_mapping.clear();
                    }
                    app.app_view = AppView::FleetOverview;
                }
                RemoteTab::ContainerLogs(ref id) => {
                    // Stop the log stream, clear cached lines, return to Containers.
                    if let Some(rh) = app.remote_hosts.get_mut(host_idx) {
                        rh.stop_log_stream(id);
                        rh.log_states.remove(id);
                    }
                    app.app_view = AppView::Remote {
                        host: host_idx,
                        tab: RemoteTab::Containers,
                    };
                }
                RemoteTab::ContainerLogsMulti(ref pairs) => {
                    // Stop all active log streams + clear multi-log state.
                    if let Some(rh) = app.remote_hosts.get_mut(host_idx) {
                        for (id, _) in pairs {
                            rh.stop_log_stream(id);
                        }
                        rh.state.lock().multi_log_container_ids.clear();
                        rh.multi_log = None;
                    }
                    app.app_view = AppView::Remote {
                        host: host_idx,
                        tab: RemoteTab::Containers,
                    };
                }
                RemoteTab::SwarmServiceTasks(_, _) => {
                    app.app_view = AppView::Remote {
                        host: host_idx,
                        tab: RemoteTab::Swarm,
                    };
                }
                RemoteTab::SwarmServiceLogs(ref id, _) => {
                    if let Some(rh) = app.remote_hosts.get_mut(host_idx) {
                        rh.stop_log_stream(id);
                        rh.service_log = None;
                    }
                    app.app_view = AppView::Remote {
                        host: host_idx,
                        tab: RemoteTab::Swarm,
                    };
                }
            }
            return Some(InputResult::Consumed);
        }
        Tab => {
            // Cycle System → Containers → Swarm → System.
            let next = match tab {
                RemoteTab::System => RemoteTab::Containers,
                RemoteTab::Containers => RemoteTab::Swarm,
                RemoteTab::Swarm => RemoteTab::System,
                // Nested views don't participate in Tab cycling.
                _ => return Some(InputResult::Consumed),
            };
            app.app_view = AppView::Remote {
                host: host_idx,
                tab: next,
            };
            return Some(InputResult::Consumed);
        }
        BackTab => {
            let prev = match tab {
                RemoteTab::System => RemoteTab::Swarm,
                RemoteTab::Containers => RemoteTab::System,
                RemoteTab::Swarm => RemoteTab::Containers,
                _ => return Some(InputResult::Consumed),
            };
            app.app_view = AppView::Remote {
                host: host_idx,
                tab: prev,
            };
            return Some(InputResult::Consumed);
        }
        _ => {}
    }

    // Tab-specific handlers.
    match tab {
        RemoteTab::System => handle_remote_system(app, host_idx, code),
        RemoteTab::Containers => handle_remote_containers(app, host_idx, code),
        RemoteTab::ContainerLogs(id) => handle_remote_container_logs(app, host_idx, &id, code),
        RemoteTab::ContainerLogsMulti(_) => handle_remote_multi_log(app, host_idx, code),
        RemoteTab::Swarm => handle_remote_swarm(app, host_idx, code),
        RemoteTab::SwarmServiceTasks(service_id, service_name) => {
            handle_remote_swarm_tasks(app, host_idx, &service_id, &service_name, code)
        }
        RemoteTab::SwarmServiceLogs(_, _) => handle_remote_service_logs(app, host_idx, code),
    }
}

/// Remote System tab keyboard handling. Mirrors `handle_system` exactly:
/// Up/Down navigate, Left/Right expand/collapse sections and process
/// groups, c/m/r/w/d/u change sort column. All mutations go through the
/// target host's `RemoteHostState` (locked briefly per mutation) so the
/// state persists across refreshes.
///
/// Row-to-section mapping is resolved via `rh.row_mapping`,
/// populated by the render path each frame.
fn handle_remote_system(app: &mut App, host_idx: usize, code: KeyCode) -> Option<InputResult> {
    use crate::model::SortColumn;
    use crossterm::event::KeyCode::*;

    let rh = app.remote_hosts.get(host_idx)?;

    match code {
        Up => {
            let mut s = rh.state.lock();
            if s.ui_state.selected_index > 0 {
                s.ui_state.selected_index -= 1;
                return Some(InputResult::Consumed);
            }
            None
        }
        Down => {
            let mut s = rh.state.lock();
            if s.ui_state.selected_index + 1 < s.ui_state.total_rows {
                s.ui_state.selected_index += 1;
                return Some(InputResult::Consumed);
            }
            None
        }
        Right => {
            // Right: expand a collapsed section header. ProcessParent
            // expansion is disabled on remote because remote ps data
            // has no child processes — expanding would be a no-op that
            // confusingly triggers the "frozen" warning. Section headers
            // still toggle because they're layout state, not data.
            let row_mapping = rh.row_mapping.clone();
            let mut s = rh.state.lock();
            let idx = s.ui_state.selected_index;
            if idx < row_mapping.len() {
                let (_pid, kind) = row_mapping[idx];
                if let RowKind::SectionHeader(section_id) = kind {
                    if s.layout.is_collapsed(section_id) {
                        s.layout.toggle_section(section_id);
                        return Some(InputResult::Consumed);
                    }
                }
                // ProcessParent: intentionally no-op on remote.
            }
            None
        }
        Left => {
            // Left: collapse an expanded section header. ProcessParent /
            // ProcessChild collapse is disabled on remote (no children).
            let row_mapping = rh.row_mapping.clone();
            let mut s = rh.state.lock();
            let idx = s.ui_state.selected_index;
            if idx < row_mapping.len() {
                let (_pid, kind) = row_mapping[idx];
                if let RowKind::SectionHeader(section_id) = kind {
                    if !s.layout.is_collapsed(section_id) {
                        s.layout.toggle_section(section_id);
                        return Some(InputResult::Consumed);
                    }
                }
            }
            None
        }
        Char('c') => {
            rh.state.lock().ui_state.sort_column = SortColumn::Cpu;
            Some(InputResult::Consumed)
        }
        Char('m') => {
            rh.state.lock().ui_state.sort_column = SortColumn::Memory;
            Some(InputResult::Consumed)
        }
        Char('r') => {
            rh.state.lock().ui_state.sort_column = SortColumn::Read;
            Some(InputResult::Consumed)
        }
        Char('w') => {
            rh.state.lock().ui_state.sort_column = SortColumn::Write;
            Some(InputResult::Consumed)
        }
        Char('d') => {
            rh.state.lock().ui_state.sort_column = SortColumn::NetDown;
            Some(InputResult::Consumed)
        }
        Char('u') => {
            rh.state.lock().ui_state.sort_column = SortColumn::NetUp;
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_containers(
    app: &mut App,
    host_idx: usize,
    code: KeyCode,
) -> Option<InputResult> {
    use crate::model::RemoteTab;
    use crossterm::event::KeyCode::*;

    let rh = app.remote_hosts.get(host_idx)?;
    match code {
        Up => {
            let mut s = rh.state.lock();
            if s.container_ui.selected_index > 0 {
                s.container_ui.selected_index -= 1;
                s.container_ui.selected_id = s
                    .containers
                    .get(s.container_ui.selected_index)
                    .map(|c| c.id.clone());
                s.status_message = None;
            }
            Some(InputResult::Consumed)
        }
        Down => {
            let mut s = rh.state.lock();
            let max = s.containers.len().saturating_sub(1);
            if s.container_ui.selected_index < max {
                s.container_ui.selected_index += 1;
                s.container_ui.selected_id = s
                    .containers
                    .get(s.container_ui.selected_index)
                    .map(|c| c.id.clone());
                s.status_message = None;
            }
            Some(InputResult::Consumed)
        }
        Right => {
            // Open log view for the selected container.
            let s = rh.state.lock();
            let idx = s.container_ui.selected_index;
            let container_id = s.containers.get(idx).map(|c| c.id.clone());
            drop(s);
            if let Some(cid) = container_id {
                let rt = Arc::clone(&app.rt);
                rh.start_container_log_stream(rt, cid.clone());
                app.app_view = AppView::Remote {
                    host: host_idx,
                    tab: RemoteTab::ContainerLogs(cid),
                };
            }
            Some(InputResult::Consumed)
        }
        Left => {
            // Toggle expand/collapse of the selected container's details.
            let mut s = rh.state.lock();
            let idx = s.container_ui.selected_index;
            if let Some(c) = s.containers.get(idx).cloned() {
                if s.container_ui.expanded_ids.contains(&c.id) {
                    s.container_ui.expanded_ids.remove(&c.id);
                } else {
                    s.container_ui.expanded_ids.insert(c.id);
                }
            }
            Some(InputResult::Consumed)
        }
        Char(' ') => {
            // Toggle multi-select marker on the selected container.
            let mut s = rh.state.lock();
            let idx = s.container_ui.selected_index;
            if let Some(c) = s.containers.get(idx).cloned() {
                if s.container_ui.selected_containers.contains(&c.id) {
                    s.container_ui.selected_containers.remove(&c.id);
                } else {
                    s.container_ui.selected_containers.insert(c.id);
                }
            }
            Some(InputResult::Consumed)
        }
        Char('l') | Char('L') => {
            // Open a multi-container log view for all selected containers.
            let s = rh.state.lock();
            let selected: Vec<(String, String)> = s
                .containers
                .iter()
                .filter(|c| s.container_ui.selected_containers.contains(&c.id))
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();
            drop(s);
            if !selected.is_empty() {
                let ids: Vec<String> = selected.iter().map(|(id, _)| id.clone()).collect();
                // Start a log stream for each selected container.
                for (id, _) in &selected {
                    let rt = Arc::clone(&app.rt);
                    rh.start_container_log_stream(rt, id.clone());
                }
                // Record which containers are in the multi-log so the
                // log-poll routine routes their lines into the multi-log
                // state instead of the per-container state.
                {
                    let mut s2 = rh.state.lock();
                    s2.multi_log_container_ids = ids;
                }
                app.app_view = AppView::Remote {
                    host: host_idx,
                    tab: RemoteTab::ContainerLogsMulti(selected),
                };
            }
            Some(InputResult::Consumed)
        }
        // Uppercase S/T/R with y/n confirmation — mirrors the local
        // bindings. Creates a PendingAction with a Remote* variant so
        // the user sees "Restart container 'foo' on root@host?" and has
        // to press y to confirm. The pending-action dispatcher at the
        // top of handle_key routes the confirmed action to
        // RemoteHost::run_action over SSH.
        Char('S') | Char('T') | Char('R') => {
            let s = rh.state.lock();
            let idx = s.container_ui.selected_index;
            let container = s.containers.get(idx).cloned();
            drop(s);
            if let Some(c) = container {
                let host_name = rh.auth.display();
                let kind = match code {
                    Char('S') => PendingActionKind::RemoteContainerStart {
                        host_idx,
                        id: c.id.clone(),
                        name: c.name.clone(),
                    },
                    Char('T') => PendingActionKind::RemoteContainerStop {
                        host_idx,
                        id: c.id.clone(),
                        name: c.name.clone(),
                    },
                    Char('R') => PendingActionKind::RemoteContainerRestart {
                        host_idx,
                        id: c.id.clone(),
                        name: c.name.clone(),
                    },
                    _ => unreachable!(),
                };
                let verb = match code {
                    Char('S') => "Start",
                    Char('T') => "Stop",
                    Char('R') => "Restart",
                    _ => unreachable!(),
                };
                app.pending_action = Some(PendingAction {
                    description: format!(
                        "{} container '{}' on {}? (y/n)",
                        verb, c.name, host_name
                    ),
                    kind,
                    expires: Instant::now() + Duration::from_secs(5),
                });
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_container_logs(
    app: &mut App,
    host_idx: usize,
    container_id: &str,
    code: KeyCode,
) -> Option<InputResult> {
    use crossterm::event::KeyCode::*;
    let rh = app.remote_hosts.get_mut(host_idx)?;

    // ── search mode: when active, intercept character input ──
    if let Some(log_state) = rh.log_states.get_mut(container_id) {
        if log_state.search_mode {
            return match code {
                Enter => {
                    log_state.search_mode = false;
                    Some(InputResult::Consumed)
                }
                Esc => {
                    log_state.search_mode = false;
                    log_state.search_query.clear();
                    Some(InputResult::Consumed)
                }
                Backspace => {
                    log_state.search_query.pop();
                    Some(InputResult::Consumed)
                }
                Char(c) => {
                    log_state.search_query.push(c);
                    Some(InputResult::Consumed)
                }
                _ => Some(InputResult::Consumed), // swallow all keys in search mode
            };
        }
    }

    // ── normal mode ──
    // ALWAYS consume navigation keys even if log_state doesn't exist
    // yet (still waiting for the first line).
    let log_state = rh.log_states.get_mut(container_id);
    match code {
        Char('/') => {
            if let Some(log_state) = log_state {
                log_state.search_mode = true;
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        Up => {
            if let Some(log_state) = log_state {
                if log_state.scroll_offset < log_state.lines.len().saturating_sub(1) {
                    log_state.scroll_offset += 1;
                    log_state.auto_follow = false;
                }
            }
            Some(InputResult::Consumed)
        }
        Down => {
            if let Some(log_state) = log_state {
                if log_state.scroll_offset > 0 {
                    log_state.scroll_offset -= 1;
                }
                if log_state.scroll_offset == 0 {
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        PageUp => {
            if let Some(log_state) = log_state {
                let page = 20;
                log_state.scroll_offset = log_state
                    .scroll_offset
                    .saturating_add(page)
                    .min(log_state.lines.len().saturating_sub(1));
                log_state.auto_follow = false;
            }
            Some(InputResult::Consumed)
        }
        PageDown => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = log_state.scroll_offset.saturating_sub(20);
                if log_state.scroll_offset == 0 {
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        Char('f') | End => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = 0;
                log_state.auto_follow = true;
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_multi_log(app: &mut App, host_idx: usize, code: KeyCode) -> Option<InputResult> {
    use crossterm::event::KeyCode::*;
    let rh = app.remote_hosts.get_mut(host_idx)?;
    let ml = rh.multi_log.as_mut();
    // Always-consume pattern. Multi-log scrolling — same as container logs.
    match code {
        Up => {
            if let Some(ml) = ml {
                if ml.scroll_offset < ml.lines.len().saturating_sub(1) {
                    ml.scroll_offset += 1;
                    ml.auto_follow = false;
                }
            }
            Some(InputResult::Consumed)
        }
        Down => {
            if let Some(ml) = ml {
                if ml.scroll_offset > 0 {
                    ml.scroll_offset -= 1;
                }
                if ml.scroll_offset == 0 {
                    ml.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        Char('f') | End => {
            if let Some(ml) = ml {
                ml.scroll_offset = 0;
                ml.auto_follow = true;
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_swarm(app: &mut App, host_idx: usize, code: KeyCode) -> Option<InputResult> {
    use crate::model::RemoteTab;
    use crossterm::event::KeyCode::*;
    let rh = app.remote_hosts.get(host_idx)?;
    match code {
        Up => {
            let mut s = rh.state.lock();
            if s.swarm_ui.selected_index > 0 {
                s.swarm_ui.selected_index -= 1;
            }
            Some(InputResult::Consumed)
        }
        Down => {
            let mut s = rh.state.lock();
            // Upper bound is the overview row count — use service count
            // as a safe approximation since we don't track row mapping
            // separately for the remote swarm view.
            let max = s.swarm_services.len().saturating_sub(1);
            if s.swarm_ui.selected_index < max {
                s.swarm_ui.selected_index += 1;
            }
            Some(InputResult::Consumed)
        }
        Char('R') => {
            // Rolling-restart with confirmation.
            let s = rh.state.lock();
            let idx = s.swarm_ui.selected_index.min(s.swarm_services.len().saturating_sub(1));
            let svc = s.swarm_services.get(idx).cloned();
            drop(s);
            if let Some(svc) = svc {
                let host_name = rh.auth.display();
                app.pending_action = Some(PendingAction {
                    description: format!(
                        "Rolling-restart service '{}' on {}? (y/n)",
                        svc.name, host_name
                    ),
                    kind: PendingActionKind::RemoteRollingRestart {
                        host_idx,
                        id: svc.id,
                        name: svc.name,
                    },
                    expires: Instant::now() + Duration::from_secs(5),
                });
            }
            Some(InputResult::Consumed)
        }
        Right | Enter => {
            // Drill into the selected service's tasks.
            let s = rh.state.lock();
            let idx = s.swarm_ui.selected_index.min(s.swarm_services.len().saturating_sub(1));
            let svc = s.swarm_services.get(idx).cloned();
            drop(s);
            if let Some(svc) = svc {
                app.app_view = AppView::Remote {
                    host: host_idx,
                    tab: RemoteTab::SwarmServiceTasks(svc.id, svc.name),
                };
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_swarm_tasks(
    app: &mut App,
    host_idx: usize,
    service_id: &str,
    service_name: &str,
    code: KeyCode,
) -> Option<InputResult> {
    use crate::model::RemoteTab;
    use crossterm::event::KeyCode::*;
    match code {
        Char('L') | Right => {
            // Start streaming service logs.
            if let Some(rh) = app.remote_hosts.get(host_idx) {
                let rt = Arc::clone(&app.rt);
                rh.start_service_log_stream(rt, service_id.to_string());
            }
            app.app_view = AppView::Remote {
                host: host_idx,
                tab: RemoteTab::SwarmServiceLogs(service_id.into(), service_name.into()),
            };
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_remote_service_logs(
    app: &mut App,
    host_idx: usize,
    code: KeyCode,
) -> Option<InputResult> {
    use crossterm::event::KeyCode::*;
    let rh = app.remote_hosts.get_mut(host_idx)?;

    // ── search mode ──
    if let Some(log_state) = rh.service_log.as_mut() {
        if log_state.search_mode {
            return match code {
                Enter => {
                    log_state.search_mode = false;
                    Some(InputResult::Consumed)
                }
                Esc => {
                    log_state.search_mode = false;
                    log_state.search_query.clear();
                    Some(InputResult::Consumed)
                }
                Backspace => {
                    log_state.search_query.pop();
                    Some(InputResult::Consumed)
                }
                Char(c) => {
                    log_state.search_query.push(c);
                    Some(InputResult::Consumed)
                }
                _ => Some(InputResult::Consumed),
            };
        }
    }

    // ── normal mode ──
    let log_state = rh.service_log.as_mut();
    match code {
        Char('/') => {
            if let Some(log_state) = log_state {
                log_state.search_mode = true;
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        Char('e') => {
            // Toggle error-only filter (ERROR, panic, fatal, exception).
            if let Some(log_state) = log_state {
                log_state.filter_errors = !log_state.filter_errors;
            }
            Some(InputResult::Consumed)
        }
        Up => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = log_state.scroll_offset.saturating_add(1);
                log_state.auto_follow = false;
            }
            Some(InputResult::Consumed)
        }
        Down => {
            if let Some(log_state) = log_state {
                if log_state.scroll_offset > 0 {
                    log_state.scroll_offset -= 1;
                }
                if log_state.scroll_offset == 0 {
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        PageUp => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = log_state.scroll_offset.saturating_add(20);
                log_state.auto_follow = false;
            }
            Some(InputResult::Consumed)
        }
        PageDown => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = log_state.scroll_offset.saturating_sub(20);
                if log_state.scroll_offset == 0 {
                    log_state.auto_follow = true;
                }
            }
            Some(InputResult::Consumed)
        }
        Char('f') | End => {
            if let Some(log_state) = log_state {
                log_state.scroll_offset = 0;
                log_state.auto_follow = true;
            }
            Some(InputResult::Consumed)
        }
        _ => None,
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
                                app.monitor
                                    .ui_state
                                    .expanded_pids
                                    .remove(&app.row_mapping[idx].0);
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
                app.docker_monitor.ui_state.selected_id = app.docker_monitor
                    .containers.get(app.docker_monitor.ui_state.selected_index)
                    .map(|c| c.id.clone());
                app.docker_monitor.status_message = None;
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Down => {
            if app.docker_monitor.ui_state.selected_index + 1 < app.docker_monitor.containers.len()
            {
                app.docker_monitor.ui_state.selected_index += 1;
                app.docker_monitor.ui_state.selected_id = app.docker_monitor
                    .containers.get(app.docker_monitor.ui_state.selected_index)
                    .map(|c| c.id.clone());
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
        KeyCode::Char(' ') => {
            if let Some(c) = app.docker_monitor.selected_container().cloned() {
                if app
                    .docker_monitor
                    .ui_state
                    .selected_containers
                    .contains(&c.id)
                {
                    app.docker_monitor
                        .ui_state
                        .selected_containers
                        .remove(&c.id);
                } else {
                    app.docker_monitor.ui_state.selected_containers.insert(c.id);
                }
                return Some(InputResult::Consumed);
            }
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            let selected = app.docker_monitor.ui_state.selected_containers.clone();
            if !selected.is_empty() {
                let container_data: Vec<(String, String)> = app
                    .docker_monitor
                    .containers
                    .iter()
                    .filter(|c| selected.contains(&c.id))
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                if !container_data.is_empty() {
                    app.docker_monitor.start_log_stream_multi(&container_data);
                    app.app_view = AppView::ContainerLogsMulti(container_data);
                    return Some(InputResult::Consumed);
                }
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

    if let AppView::ContainerLogs(container_id) = &app.app_view {
        let log_state = app.docker_monitor.get_log_state_mut(container_id);
        if let Some(ref log_state) = log_state {
            if log_state.search_mode {
                return match code {
                    KeyCode::Enter => {
                        if let Some(ref mut ls) = app.docker_monitor.get_log_state_mut(container_id)
                        {
                            ls.search_mode = false;
                        }
                        Some(InputResult::Consumed)
                    }
                    KeyCode::Esc => {
                        if let Some(ref mut ls) = app.docker_monitor.get_log_state_mut(container_id)
                        {
                            ls.search_mode = false;
                            ls.search_query.clear();
                        }
                        Some(InputResult::Consumed)
                    }
                    KeyCode::Backspace => {
                        if let Some(ref mut ls) = app.docker_monitor.get_log_state_mut(container_id)
                        {
                            ls.search_query.pop();
                        }
                        Some(InputResult::Consumed)
                    }
                    KeyCode::Char(c) => {
                        if let Some(ref mut ls) = app.docker_monitor.get_log_state_mut(container_id)
                        {
                            ls.search_query.push(c);
                        }
                        Some(InputResult::Consumed)
                    }
                    _ => None,
                };
            }
        }
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
            // Single-container logs: stop the active stream on exit.
            app.docker_monitor.stop_log_stream();
            app.app_view = AppView::Containers;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    log_state.auto_follow = false;
                    let max_offset = log_state.lines.len().saturating_sub(1);
                    if log_state.scroll_offset < max_offset {
                        log_state.scroll_offset += 1;
                    }
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Down => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    if log_state.scroll_offset > 0 {
                        log_state.scroll_offset -= 1;
                        if log_state.scroll_offset == 0 {
                            log_state.auto_follow = true;
                        }
                    }
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('f') | KeyCode::End => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    log_state.auto_follow = true;
                    log_state.scroll_offset = 0;
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('/') => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    log_state.search_mode = true;
                    log_state.search_query.clear();
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('n') => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    log_state.search_query.clear();
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageUp => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    log_state.auto_follow = false;
                    let max_offset = log_state.lines.len().saturating_sub(1);
                    log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageDown => {
            if let AppView::ContainerLogs(container_id) = &app.app_view.clone() {
                if let Some(ref mut log_state) = app.docker_monitor.get_log_state_mut(&container_id)
                {
                    if log_state.scroll_offset > page_size {
                        log_state.scroll_offset -= page_size;
                    } else {
                        log_state.scroll_offset = 0;
                        log_state.auto_follow = true;
                    }
                }
            }
            Some(InputResult::Consumed)
        }
        _ => None,
    }
}

fn handle_container_logs_multi(app: &mut App, code: KeyCode) -> Option<InputResult> {
    let page_size = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24)
        .saturating_sub(4);

    if app
        .docker_monitor
        .multi_log_state
        .as_ref()
        .map(|s| s.search_mode)
        .unwrap_or(false)
    {
        return match code {
            KeyCode::Enter => {
                if let Some(ref mut ls) = app.docker_monitor.multi_log_state {
                    ls.search_mode = false;
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Esc => {
                if let Some(ref mut ls) = app.docker_monitor.multi_log_state {
                    ls.search_mode = false;
                    ls.search_query.clear();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Backspace => {
                if let Some(ref mut ls) = app.docker_monitor.multi_log_state {
                    ls.search_query.pop();
                }
                Some(InputResult::Consumed)
            }
            KeyCode::Char(c) => {
                if let Some(ref mut ls) = app.docker_monitor.multi_log_state {
                    ls.search_query.push(c);
                }
                Some(InputResult::Consumed)
            }
            _ => None,
        };
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Left => {
            // Multi-container logs: preserve existing streams when exiting the view.
            app.app_view = AppView::Containers;
            Some(InputResult::Consumed)
        }
        KeyCode::Up => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                if log_state.scroll_offset < max_offset {
                    log_state.scroll_offset += 1;
                }
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Down => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
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
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
                log_state.auto_follow = true;
                log_state.scroll_offset = 0;
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('/') => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
                log_state.search_mode = true;
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::Char('n') => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
                log_state.search_query.clear();
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageUp => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
                log_state.auto_follow = false;
                let max_offset = log_state.lines.len().saturating_sub(1);
                log_state.scroll_offset = (log_state.scroll_offset + page_size).min(max_offset);
            }
            Some(InputResult::Consumed)
        }
        KeyCode::PageDown => {
            if let Some(ref mut log_state) = app.docker_monitor.multi_log_state {
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
        KeyCode::Right => match item {
            super::state::SwarmOverviewItem::NodesHeader => {
                app.swarm_monitor
                    .ui_state
                    .expanded_ids
                    .insert("__nodes__".to_string());
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
        },
        KeyCode::Left => match item {
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
        },
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

    if app
        .swarm_monitor
        .log_state
        .as_ref()
        .map_or(false, |s| s.search_mode)
    {
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
