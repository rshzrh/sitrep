use std::io;

use crossterm::{cursor::MoveTo, execute, terminal::Clear, terminal::ClearType};

use crate::model::{AppView, RemoteTab, SwarmViewLevel};
use crate::view::Presenter;

use super::App;

pub fn render(app: &mut App) -> io::Result<()> {
    let time_str = app
        .monitor
        .last_data
        .as_ref()
        .map(|d| d.time.clone())
        .unwrap_or_else(|| "...".to_string());

    let swarm_active = app.swarm_monitor.is_swarm();
    let swarm_node_count = app
        .swarm_monitor
        .cluster_info
        .as_ref()
        .map(|c| c.nodes_total)
        .unwrap_or(0);

    let mut out = io::stdout();

    // Snapshot Remote { host, tab } before the match so we can pass
    // `app` mutably to render_remote_tab without overlapping borrows.
    let remote_target = match &app.app_view {
        AppView::Remote { host, tab } => Some((*host, tab.clone())),
        _ => None,
    };

    match &app.app_view {
        crate::model::AppView::System => {
            execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
            Presenter::render_tab_bar(
                &mut out,
                &app.app_view,
                app.docker_monitor.is_available(),
                app.docker_monitor.containers.len(),
                swarm_active,
                swarm_node_count,
                &time_str,
            )?;
            if let Some(ref data) = app.monitor.last_data {
                app.row_mapping =
                    Presenter::render(data, &mut app.monitor.ui_state, &app.monitor.layout)?;
            }
        }
        crate::model::AppView::Containers => {
            execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
            Presenter::render_tab_bar(
                &mut out,
                &app.app_view,
                app.docker_monitor.is_available(),
                app.docker_monitor.containers.len(),
                swarm_active,
                swarm_node_count,
                &time_str,
            )?;
            Presenter::render_containers(
                &app.docker_monitor.containers,
                &app.docker_monitor.ui_state,
                &app.docker_monitor.status_message,
            )?;
        }
        crate::model::AppView::ContainerLogs(_) => {
            if let AppView::ContainerLogs(container_id) = &app.app_view {
                if let Some(ref log_state) = app.docker_monitor.get_log_state(container_id) {
                    Presenter::render_logs(log_state)?;
                }
            }
        }
        crate::model::AppView::ContainerLogsMulti(_) => {
            // `active_log_names` is maintained incrementally by
            // start_log_stream / stop_log_stream — no per-frame rebuild.
            if let Some(ref multi_state) = app.docker_monitor.multi_log_state {
                Presenter::render_multi_container_logs(
                    multi_state,
                    &app.docker_monitor.active_log_names,
                )?;
            }
        }
        crate::model::AppView::Swarm | crate::model::AppView::SwarmServiceTasks(_, _) => {
            execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
            Presenter::render_tab_bar(
                &mut out,
                &app.app_view,
                app.docker_monitor.is_available(),
                app.docker_monitor.containers.len(),
                swarm_active,
                swarm_node_count,
                &time_str,
            )?;
            match &app.swarm_monitor.ui_state.view_level {
                SwarmViewLevel::Overview => {
                    Presenter::render_swarm_overview(
                        &app.swarm_monitor.cluster_info,
                        &app.swarm_monitor.nodes,
                        &app.swarm_monitor.stacks,
                        &app.swarm_monitor.services,
                        &app.swarm_monitor.ui_state,
                        &app.swarm_monitor.warnings,
                        &app.swarm_monitor.status_message,
                        &app.swarm_monitor.service_tasks,
                    )?;
                }
                SwarmViewLevel::ServiceTasks(_, name) => {
                    Presenter::render_swarm_tasks(
                        name,
                        &app.swarm_monitor.tasks,
                        &app.swarm_monitor.nodes,
                        app.swarm_monitor.ui_state.selected_index,
                        &app.swarm_monitor.status_message,
                    )?;
                }
                SwarmViewLevel::ServiceLogs(_, _) => {}
            }
        }
        crate::model::AppView::SwarmServiceLogs(_, _) => {
            if let Some(ref log_state) = app.swarm_monitor.log_state {
                Presenter::render_service_logs(log_state)?;
            }
        }
        crate::model::AppView::FleetOverview => {
            execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;
            if let Some(ref fleet_state) = app.fleet_state {
                let (cols, rows) = crossterm::terminal::size()?;
                let lines = crate::view::format_fleet_overview(fleet_state, cols);
                for (i, line) in lines.iter().enumerate() {
                    if (i as u16) >= rows {
                        break;
                    }
                    execute!(out, MoveTo(0, i as u16))?;
                    use std::io::Write;
                    write!(out, "{}", line)?;
                }
                use std::io::Write;
                out.flush()?;
            }
        }
        crate::model::AppView::Remote { .. } => {
            // handled after the match drops the immutable borrow
        }
    }

    if let Some((host, tab)) = remote_target {
        render_remote_tab(app, host, &tab)?;
    }

    Ok(())
}

/// Everything the render path needs from one `RemoteHost` in a single
/// locked snapshot. Producing this up front guarantees the tab bar and
/// the tab body render the same data (no torn reads where the refresh
/// task runs between lock acquisitions).
struct RemoteRenderSnapshot {
    time_str: String,
    status_msg: Option<String>,
    monitor_data: Option<crate::model::MonitorData>,
    /// Heavy data Vecs are held as `Arc<Vec<T>>` so cloning them out of
    /// the state mutex is a pointer bump, not a deep copy. Call sites
    /// that pass `&[T]` to view functions use `&*snap.containers` etc.
    containers: std::sync::Arc<Vec<crate::model::DockerContainerInfo>>,
    container_ui: crate::model::ContainerUIState,
    swarm_info: Option<crate::model::SwarmClusterInfo>,
    swarm_nodes: std::sync::Arc<Vec<crate::model::SwarmNodeInfo>>,
    swarm_stacks: std::sync::Arc<Vec<crate::model::SwarmStackInfo>>,
    swarm_services: std::sync::Arc<Vec<crate::model::SwarmServiceInfo>>,
    swarm_warnings: std::sync::Arc<Vec<String>>,
    swarm_service_tasks: std::sync::Arc<
        std::collections::HashMap<String, Vec<crate::model::SwarmTaskInfo>>,
    >,
    swarm_ui: crate::model::SwarmUIState,
    container_count: usize,
    swarm_active: bool,
    node_count: u32,
    ui_state: crate::model::UIState,
    layout: crate::layout::Layout,
    prev_selected_pid: Option<sysinfo::Pid>,
}

/// Acquire a single lock on `rh.state` and clone everything the render
/// path could possibly need. The mutex is held for microseconds — long
/// enough to memcpy a dozen Vecs, not long enough to block the refresh
/// task in any meaningful way.
fn snapshot_for_render(rh: &crate::remote_host::RemoteHost) -> RemoteRenderSnapshot {
    let s = rh.state.lock();
    let time_str = s
        .monitor_data
        .as_ref()
        .map(|m| m.time.clone())
        .unwrap_or_else(|| "—".into());
    RemoteRenderSnapshot {
        time_str,
        status_msg: s.status_message.clone(),
        monitor_data: s.monitor_data.clone(),
        containers: s.containers.clone(),
        container_ui: s.container_ui.clone(),
        swarm_info: s.swarm_info.clone(),
        swarm_nodes: s.swarm_nodes.clone(),
        swarm_stacks: s.swarm_stacks.clone(),
        swarm_services: s.swarm_services.clone(),
        swarm_warnings: s.swarm_warnings.clone(),
        swarm_service_tasks: s.swarm_service_tasks.clone(),
        swarm_ui: s.swarm_ui.clone(),
        container_count: s.containers.len(),
        swarm_active: s.swarm_info.is_some(),
        node_count: s.swarm_info.as_ref().map(|ci| ci.nodes_total).unwrap_or(0),
        ui_state: s.ui_state.clone(),
        layout: s.layout.clone(),
        prev_selected_pid: s.prev_selected_pid,
    }
}

/// Render a single remote drill-in tab.
///
/// ```text
/// 1. Take ONE snapshot of everything (snapshot_for_render)
/// 2. Render tab bar from snapshot.time_str + counts
/// 3. Render the specific tab from the same snapshot
/// 4. If the tab mutated UIState (System render does that), write it back
///    to rh.state in a second brief lock
/// ```
///
/// No lock is ever held across a crossterm I/O call.
fn render_remote_tab(app: &mut App, host_idx: usize, tab: &RemoteTab) -> io::Result<()> {
    let mut out = io::stdout();
    execute!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    // Mutable borrow held for the whole function so we can write back
    // `row_mapping` / `ui_state` at the end. `app.app_view` is a
    // separate field, so Rust's split-borrow lets us still read it
    // for the tab bar.
    let Some(rh) = app.remote_hosts.get_mut(host_idx) else {
        return Ok(());
    };

    // ── Step 1: one lock, take everything we need ──
    let snap = snapshot_for_render(rh);

    // ── Step 2: tab bar ──
    Presenter::render_tab_bar(
        &mut out,
        &app.app_view,
        /* docker_available = */ true,
        snap.container_count,
        snap.swarm_active,
        snap.node_count,
        &snap.time_str,
    )?;

    // ── Step 3: tab-specific body ──
    match tab {
        RemoteTab::System => {
            if let Some(md) = snap.monitor_data {
                let mut ui_clone = snap.ui_state;
                let layout_clone = snap.layout;

                // Remote data has no child processes, so expanded_pids
                // is meaningless. Clear it so the "Expanded section data
                // frozen" warning at the bottom of render_system never
                // shows for remote hosts (it was confusing users because
                // no expansion is visually apparent and the data isn't
                // actually frozen — the refresh task keeps writing).
                ui_clone.expanded_pids.clear();

                let row_mapping = Presenter::render(&md, &mut ui_clone, &layout_clone)?;

                // ── PID-based selection tracking ──
                // After render, re-resolve selected_index to the row
                // containing the previously-selected PID. This prevents
                // the cursor from jumping to a different process when a
                // 5s refresh reorders the process list.
                if let Some(prev_pid) = snap.prev_selected_pid {
                    if let Some(new_idx) = row_mapping.iter().position(|(pid, _)| *pid == prev_pid) {
                        ui_clone.selected_index = new_idx;
                    }
                }
                ui_clone.selected_index = ui_clone
                    .selected_index
                    .min(ui_clone.total_rows.saturating_sub(1));

                // Record which PID is currently selected so next frame
                // can re-resolve.
                let current_pid = row_mapping
                    .get(ui_clone.selected_index)
                    .map(|(pid, _)| *pid);

                {
                    let mut s = rh.state.lock();
                    s.ui_state = ui_clone;
                    s.prev_selected_pid = current_pid;
                }
                rh.row_mapping = row_mapping;
            } else {
                use std::io::Write;
                write!(out, "  (connecting...)\r\n")?;
                out.flush()?;
            }
        }
        RemoteTab::Containers => {
            Presenter::render_containers(
                snap.containers.as_slice(),
                &snap.container_ui,
                &snap.status_msg,
            )?;
        }
        RemoteTab::ContainerLogs(container_id) => {
            if let Some(log_state) = rh.log_states.get(container_id) {
                Presenter::render_logs(log_state)?;
            } else {
                use std::io::Write;
                write!(
                    out,
                    "  REMOTE LOGS — {} (waiting for first line...)\r\n",
                    container_id
                )?;
                out.flush()?;
            }
        }
        RemoteTab::ContainerLogsMulti(pairs) => {
            if let Some(multi_state) = rh.multi_log.as_ref() {
                let mut names: Vec<String> = pairs.iter().map(|(_, n)| n.clone()).collect();
                names.sort();
                names.dedup();
                Presenter::render_multi_container_logs(multi_state, &names)?;
            } else {
                use std::io::Write;
                write!(out, "  REMOTE MULTI-LOG (waiting for first line...)\r\n")?;
                out.flush()?;
            }
        }
        RemoteTab::Swarm => {
            Presenter::render_swarm_overview(
                &snap.swarm_info,
                snap.swarm_nodes.as_slice(),
                snap.swarm_stacks.as_slice(),
                snap.swarm_services.as_slice(),
                &snap.swarm_ui,
                snap.swarm_warnings.as_slice(),
                &snap.status_msg,
                snap.swarm_service_tasks.as_ref(),
            )?;
        }
        RemoteTab::SwarmServiceTasks(_, service_name) => {
            let flat: Vec<crate::model::SwarmTaskInfo> = snap
                .swarm_service_tasks
                .values()
                .flat_map(|v| v.iter().cloned())
                .filter(|t| t.name.starts_with(service_name.as_str()))
                .collect();
            Presenter::render_swarm_tasks(
                service_name,
                &flat,
                snap.swarm_nodes.as_slice(),
                0,
                &snap.status_msg,
            )?;
        }
        RemoteTab::SwarmServiceLogs(_, _) => {
            if let Some(svc_state) = rh.service_log.as_ref() {
                Presenter::render_service_logs(svc_state)?;
            } else {
                use std::io::Write;
                write!(out, "  REMOTE SERVICE LOGS — (waiting for first line...)\r\n")?;
                out.flush()?;
            }
        }
    }
    Ok(())
}
