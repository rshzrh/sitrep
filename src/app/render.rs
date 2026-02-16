use std::io;

use crossterm::{execute, cursor::MoveTo, terminal::Clear, terminal::ClearType};

use crate::model::SwarmViewLevel;
use crate::view::Presenter;

use super::App;

pub fn render(app: &mut App) -> io::Result<()> {
    let time_str = app.monitor.last_data.as_ref()
        .map(|d| d.time.clone())
        .unwrap_or_else(|| "...".to_string());

    let swarm_active = app.swarm_monitor.is_swarm();
    let swarm_node_count = app.swarm_monitor.cluster_info.as_ref()
        .map(|c| c.nodes_total).unwrap_or(0);

    let mut out = io::stdout();

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
                app.row_mapping = Presenter::render(
                    data,
                    &mut app.monitor.ui_state,
                    &app.monitor.layout,
                )?;
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
            if let Some(ref log_state) = app.docker_monitor.log_state {
                Presenter::render_logs(log_state)?;
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
    }

    Ok(())
}
