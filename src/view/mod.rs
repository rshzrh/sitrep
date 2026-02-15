mod shared;
mod tab_bar;
mod system;
mod containers;
mod swarm;
mod logs;
mod confirmation;

use std::io::{self, Write};
use crossterm::{execute, cursor, queue, style::{Color, SetForegroundColor, ResetColor}, terminal};
use crate::layout::SectionId;
use sysinfo::Pid;

pub use shared::{truncate_str, safe_truncate};

/// What kind of row this is in the row mapping
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    SectionHeader(SectionId),
    ProcessParent,
    ProcessChild,
}

pub struct Presenter;

/// Minimum terminal dimensions for usable rendering.
pub const MIN_COLS: u16 = 80;
pub const MIN_ROWS: u16 = 10;

impl Presenter {
    /// Check if the terminal is large enough. If not, render a "too small"
    /// message and return `true` (meaning "skip normal rendering").
    pub fn render_size_guard() -> io::Result<bool> {
        let (cols, rows) = terminal::size()?;
        if cols < MIN_COLS || rows < MIN_ROWS {
            let mut out = std::io::stdout();
            execute!(out, terminal::Clear(terminal::ClearType::All), cursor::MoveTo(0, 0))?;
            let msg = format!(
                "Terminal too small ({}x{}). Resize to at least {}x{}.",
                cols, rows, MIN_COLS, MIN_ROWS
            );
            let y = rows / 2;
            let x = cols.saturating_sub(msg.len() as u16) / 2;
            queue!(out, cursor::MoveTo(x, y), SetForegroundColor(Color::Yellow))?;
            write!(out, "{}", msg)?;
            queue!(out, ResetColor)?;
            out.flush()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn render_tab_bar(
        out: &mut impl std::io::Write,
        current_view: &crate::model::AppView,
        docker_available: bool,
        container_count: usize,
        swarm_active: bool,
        node_count: u32,
        time: &str,
    ) -> io::Result<()> {
        tab_bar::render_tab_bar(out, current_view, docker_available, container_count, swarm_active, node_count, time)
    }

    pub fn render(
        data: &crate::model::MonitorData,
        ui_state: &mut crate::model::UIState,
        layout: &crate::layout::Layout,
    ) -> io::Result<Vec<(Pid, RowKind)>> {
        system::render(data, ui_state, layout)
    }

    pub fn render_containers(
        containers: &[crate::model::DockerContainerInfo],
        ui_state: &crate::model::ContainerUIState,
        status_message: &Option<String>,
    ) -> io::Result<()> {
        containers::render_containers(containers, ui_state, status_message)
    }

    pub fn render_logs(log_state: &crate::model::LogViewState) -> io::Result<()> {
        logs::render_logs(log_state)
    }

    pub fn render_swarm_overview(
        cluster_info: &Option<crate::model::SwarmClusterInfo>,
        nodes: &[crate::model::SwarmNodeInfo],
        stacks: &[crate::model::SwarmStackInfo],
        services: &[crate::model::SwarmServiceInfo],
        ui_state: &crate::model::SwarmUIState,
        warnings: &[String],
        status_message: &Option<String>,
    ) -> io::Result<()> {
        swarm::render_swarm_overview(cluster_info, nodes, stacks, services, ui_state, warnings, status_message)
    }

    pub fn render_swarm_tasks(
        service_name: &str,
        tasks: &[crate::model::SwarmTaskInfo],
        selected_index: usize,
        status_message: &Option<String>,
    ) -> io::Result<()> {
        swarm::render_swarm_tasks(service_name, tasks, selected_index, status_message)
    }

    pub fn render_service_logs(log_state: &crate::model::ServiceLogState) -> io::Result<()> {
        logs::render_service_logs(log_state)
    }

    pub fn render_confirmation(prompt: &str) -> io::Result<()> {
        confirmation::render_confirmation(prompt)
    }
}
