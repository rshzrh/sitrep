use crate::model::{
    MonitorData, UIState, AppView, DockerContainerInfo, ContainerUIState, LogViewState,
    SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo, SwarmTaskInfo,
    SwarmUIState, ServiceLogState,
};
use crate::layout::{Layout, SectionId};
use std::io::{self, Write, stdout};
use crossterm::{
    cursor::{self, MoveTo}, execute, queue,
    style::{Color, SetForegroundColor, SetBackgroundColor, ResetColor, Attribute, SetAttribute, Print},
    terminal::{self, Clear, ClearType},
};
use sysinfo::Pid;

/// What kind of row this is in the row mapping
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    SectionHeader(SectionId),
    ProcessParent,
    ProcessChild,
}

pub struct Presenter;

impl Presenter {
    // =====================================================================
    // Tab bar (rendered at the top of every view except full-screen logs)
    // =====================================================================

    pub fn render_tab_bar(
        out: &mut impl Write,
        current_view: &AppView,
        docker_available: bool,
        container_count: usize,
        swarm_active: bool,
        node_count: u32,
        time: &str,
    ) -> io::Result<()> {
        write!(out, "  ")?;

        // System tab
        let system_active = matches!(current_view, AppView::System);
        if system_active {
            queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
        } else {
            queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        }
        write!(out, " System ")?;
        queue!(io::stdout(), ResetColor)?;

        if docker_available {
            write!(out, "  ")?;
            let containers_active = matches!(current_view, AppView::Containers | AppView::ContainerLogs(_));
            if containers_active {
                queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
            } else {
                queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            }
            write!(out, " Containers ({}) ", container_count)?;
            queue!(io::stdout(), ResetColor)?;
        }

        if swarm_active {
            write!(out, "  ")?;
            let swarm_tab_active = matches!(
                current_view,
                AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _)
            );
            if swarm_tab_active {
                queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
            } else {
                queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            }
            write!(out, " Swarm ({} nodes) ", node_count)?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Right-align the time
        let size = terminal::size()?;
        let time_str = format!("sitrep - {} ", time);
        let col = (size.0 as usize).saturating_sub(time_str.len());
        queue!(io::stdout(), cursor::MoveTo(col as u16, 0))?;
        queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        write!(out, "{}", time_str)?;
        queue!(io::stdout(), ResetColor)?;

        write!(out, "\r\n")?;
        // Separator
        let sep: String = "─".repeat(size.0 as usize);
        queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        write!(out, "{}\r\n", sep)?;
        queue!(io::stdout(), ResetColor)?;

        Ok(())
    }

    // =====================================================================
    // System view (the original render, now refactored)
    // =====================================================================

    pub fn render(
        data: &MonitorData,
        ui_state: &mut UIState,
        layout: &Layout,
    ) -> io::Result<Vec<(Pid, RowKind)>> {
        let mut out = stdout();
        // Tab bar + clear is handled by main.rs before calling this

        let mut rows: Vec<(Pid, RowKind)> = Vec::new();
        let mut current_row: usize = 0;

        // Time header
        Self::writeln(&mut out, &format!("  sitrep — {}  |  Cores: {}", data.time, data.core_count))?;
        Self::writeln(&mut out, "")?;

        for section in &layout.sections {
            if section.id == SectionId::Summary {
                // Summary renders its own header/separator
                rows.push((Pid::from(0), RowKind::SectionHeader(section.id)));
                current_row += 1;
                Self::render_summary(&mut out, data)?;
                continue;
            }

            let indicator = if section.collapsed { "▶" } else { "▼" };
            let header = format!("{} --- {} ---", indicator, section.title);

            Self::write_section_header(&mut out, &header, current_row == ui_state.selected_index)?;
            rows.push((Pid::from(0), RowKind::SectionHeader(section.id)));
            current_row += 1;

            if section.collapsed {
                continue;
            }

            match section.id {
                SectionId::Summary => {}, // Handled above
                SectionId::Processes => {
                    Self::render_processes(&mut out, data, ui_state, &mut rows, &mut current_row)?;
                }
                SectionId::Network => Self::render_network(&mut out, data)?,
                SectionId::FileDescriptors => Self::render_fd_info(&mut out, data)?,
                SectionId::SocketOverview => Self::render_socket_overview(&mut out, data)?,
            }
            Self::writeln(&mut out, "")?;
        }

        ui_state.total_rows = current_row;

        // Footer with help
        let help = "q: Quit | Ctrl+C: Force Quit | Tab: Switch | ↑/↓: Navigate | →/←: Expand/Collapse | Sort: (c)pu (m)em (r)ead (w)rite (d)ownload (u)pload";
        let size = terminal::size()?;
        let help_y = size.1.saturating_sub(1);
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = size.0 as usize)),
            ResetColor
        )?;

        if ui_state.has_expansions() {
            queue!(out, SetForegroundColor(Color::Yellow))?;
            Self::writeln(&mut out, "(Expanded section data frozen)")?;
            queue!(out, ResetColor)?;
        }

        out.flush()?;
        Ok(rows)
    }

    // =====================================================================
    // Container list view
    // =====================================================================

    pub fn render_containers(
        containers: &[DockerContainerInfo],
        ui_state: &ContainerUIState,
        status_message: &Option<String>,
    ) -> io::Result<()> {
        let mut out = stdout();
        // Tab bar already rendered; we start from row 2
        queue!(out, cursor::MoveTo(0, 2))?;

        let size = terminal::size()?;

        if containers.is_empty() {
            Self::writeln(&mut out, "")?;
            Self::writeln(&mut out, "  No running containers found.")?;
            Self::writeln(&mut out, "")?;
            Self::writeln(&mut out, "  Make sure Docker is running and you have containers up.")?;
        } else {
            Self::writeln(&mut out, "")?;

            // Column header
            queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
            write!(out, "  {:<14} {:<20} {:<12} {:<10} {:<8} {:<26} {}",
                "CONTAINER ID", "NAME", "STATUS", "UPTIME", "CPU %", "PORTS", "IP")?;
            queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
            write!(out, "\r\n")?;

            for (idx, c) in containers.iter().enumerate() {
                let selected = idx == ui_state.selected_index;

                let line = format!("  {:<14} {:<20} {:<12} {:<10} {:<8.1} {:<26} {}",
                    c.id,
                    truncate_str(&c.name, 18),
                    truncate_str(&c.state, 10),
                    c.uptime,
                    c.cpu_percent,
                    truncate_str(&c.ports, 24),
                    c.ip_address,
                );

                Self::write_selectable(&mut out, &line, selected)?;

                // Expanded detail
                if ui_state.expanded_ids.contains(&c.id) {
                    queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
                    Self::writeln(&mut out, &format!("    Image:  {}", c.image))?;
                    Self::writeln(&mut out, &format!("    Status: {}", c.status))?;
                    Self::writeln(&mut out, &format!("    Ports:  {}", c.ports))?;
                    Self::writeln(&mut out, &format!("    IP:     {}", c.ip_address))?;
                    queue!(io::stdout(), ResetColor)?;
                }
            }
        }

        // Status message (action feedback)
        if let Some(msg) = status_message {
            Self::writeln(&mut out, "")?;
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            Self::writeln(&mut out, &format!("  {}", msg))?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Footer
        let help = "q/Esc: Back | Tab: Switch | ↑/↓: Navigate | →: Logs | S: Start | T: Stop | R: Restart (confirm with y)";
        let help_y = size.1.saturating_sub(1);
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = size.0 as usize)),
            ResetColor
        )?;

        out.flush()?;
        Ok(())
    }

    // =====================================================================
    // Full-screen log viewer
    // =====================================================================

    pub fn render_logs(log_state: &LogViewState) -> io::Result<()> {
        let mut out = stdout();
        execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

        let size = terminal::size()?;
        let width = size.0 as usize;
        let height = size.1 as usize;

        // Header
        let follow_indicator = if log_state.auto_follow { "FOLLOWING" } else { "PAUSED" };
        let search_indicator = if !log_state.search_query.is_empty() {
            format!(" | SEARCH: \"{}\"", log_state.search_query)
        } else {
            String::new()
        };
        let header = format!("  Logs: {} ({}) - {}{}",
            log_state.container_name, log_state.container_id, follow_indicator, search_indicator);

        queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        if !log_state.auto_follow {
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        }
        write!(out, "{}\r\n", header)?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

        // Search prompt line (if active)
        if log_state.search_mode {
            queue!(io::stdout(), SetForegroundColor(Color::Cyan))?;
            write!(out, "  Search: {}_\r\n", log_state.search_query)?;
            queue!(io::stdout(), ResetColor)?;
        } else {
            // Separator
            let sep: String = "─".repeat(width);
            queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            write!(out, "{}\r\n", sep)?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Log content area
        let log_area_height = height.saturating_sub(4);

        // Filter lines by search query if set
        let has_search = !log_state.search_query.is_empty();
        let query_lower = log_state.search_query.to_lowercase();

        let display_lines: Vec<&String> = if has_search {
            log_state.lines.iter()
                .filter(|l| l.to_lowercase().contains(&query_lower))
                .collect()
        } else {
            log_state.lines.iter().collect()
        };

        let total_lines = display_lines.len();

        let start_line = if log_state.auto_follow {
            total_lines.saturating_sub(log_area_height)
        } else {
            let bottom_start = total_lines.saturating_sub(log_area_height);
            bottom_start.saturating_sub(log_state.scroll_offset)
        };

        let end_line = (start_line + log_area_height).min(total_lines);

        let mut lines_printed = 0;
        for i in start_line..end_line {
            if let Some(line) = display_lines.get(i) {
                let display_line = safe_truncate(line, width);

                // Highlight search matches
                if has_search {
                    let lower_line = display_line.to_lowercase();
                    if lower_line.contains(&query_lower) {
                        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                        write!(out, "{}\r\n", display_line)?;
                        queue!(io::stdout(), ResetColor)?;
                    } else {
                        write!(out, "{}\r\n", display_line)?;
                    }
                } else {
                    write!(out, "{}\r\n", display_line)?;
                }
                lines_printed += 1;
            }
        }

        // Fill remaining space
        for _ in lines_printed..log_area_height {
            write!(out, "\r\n")?;
        }

        // Footer
        let help = if log_state.search_mode {
            "Type to search | Enter: Confirm | Esc: Cancel"
        } else if has_search {
            "q/Esc/←: Back | ↑/↓: Scroll | f/End: Follow | /: Search | n: Clear search"
        } else {
            "q/Esc/←: Back | ↑/↓: Scroll (pauses follow) | f/End: Resume follow | /: Search"
        };
        let help_y = (height.saturating_sub(1)) as u16;
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = width)),
            ResetColor
        )?;

        out.flush()?;
        Ok(())
    }

    // =====================================================================
    // Swarm overview (nodes + stacks/services)
    // =====================================================================

    pub fn render_swarm_overview(
        cluster_info: &Option<SwarmClusterInfo>,
        nodes: &[SwarmNodeInfo],
        stacks: &[SwarmStackInfo],
        services: &[SwarmServiceInfo],
        ui_state: &SwarmUIState,
        warnings: &[String],
        status_message: &Option<String>,
    ) -> io::Result<()> {
        let mut out = stdout();
        queue!(out, cursor::MoveTo(0, 2))?;

        let size = terminal::size()?;

        // Cluster header
        if let Some(info) = cluster_info {
            queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
            write!(out, "  Cluster: {} nodes ({} managers) | This node: {}\r\n",
                info.nodes_total, info.managers,
                if info.is_manager { "manager" } else { "worker" })?;
            queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
        }
        Self::writeln(&mut out, "")?;

        // Warnings
        if !warnings.is_empty() {
            for w in warnings {
                queue!(io::stdout(), SetForegroundColor(Color::Red), SetAttribute(Attribute::Bold))?;
                Self::writeln(&mut out, &format!("  ⚠ {}", w))?;
                queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
            }
            Self::writeln(&mut out, "")?;
        }

        let mut row_idx: usize = 0;

        // --- Nodes section ---
        let nodes_expanded = ui_state.expanded_ids.contains("__nodes__");
        let node_indicator = if nodes_expanded { "▼" } else { "▶" };
        let node_header = format!("  {} NODES ({})", node_indicator, nodes.len());
        Self::write_selectable(&mut out, &node_header,
            row_idx == ui_state.selected_index)?;
        row_idx += 1;

        if nodes_expanded {
            // Node column headers
            queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            write!(out, "    {:<14} {:<20} {:<10} {:<12} {:<14} {}\r\n",
                "ID", "HOSTNAME", "STATUS", "AVAIL", "ROLE", "ENGINE")?;
            queue!(io::stdout(), ResetColor)?;

            for node in nodes {
                let role = if !node.manager_status.is_empty() {
                    &node.manager_status
                } else {
                    "worker"
                };

                let self_marker = if node.is_self { " *" } else { "" };

                let line = format!("    {:<14} {:<20} {:<10} {:<12} {:<14} {}{}",
                    truncate_str(&node.id, 12),
                    truncate_str(&node.hostname, 18),
                    &node.status,
                    &node.availability,
                    role,
                    &node.engine_version,
                    self_marker,
                );

                // Color by status
                if node.status.to_lowercase().contains("down") {
                    queue!(io::stdout(), SetForegroundColor(Color::Red))?;
                } else if node.availability.to_lowercase().contains("drain") {
                    queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                }

                Self::write_selectable(&mut out, &line,
                    row_idx == ui_state.selected_index)?;
                queue!(io::stdout(), ResetColor)?;
                row_idx += 1;
            }
        }

        Self::writeln(&mut out, "")?;

        // --- Stacks/Services ---
        for stack in stacks {
            let stack_expanded = ui_state.expanded_ids.contains(&stack.name);
            let indicator = if stack_expanded { "▼" } else { "▶" };
            let svc_count = stack.service_indices.len();
            let stack_header = format!("  {} STACK: {} ({} services)", indicator, stack.name, svc_count);

            Self::write_selectable(&mut out, &stack_header,
                row_idx == ui_state.selected_index)?;
            row_idx += 1;

            if stack_expanded {
                // Service column headers
                queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
                write!(out, "    {:<14} {:<28} {:<12} {:<10} {:<20} {}\r\n",
                    "ID", "NAME", "MODE", "REPLICAS", "IMAGE", "PORTS")?;
                queue!(io::stdout(), ResetColor)?;

                for &idx in &stack.service_indices {
                    let svc = &services[idx];
                    let line = format!("    {:<14} {:<28} {:<12} {:<10} {:<20} {}",
                        truncate_str(&svc.id, 12),
                        truncate_str(&svc.name, 26),
                        &svc.mode,
                        &svc.replicas,
                        truncate_str(&svc.image, 18),
                        truncate_str(&svc.ports, 20),
                    );

                    // Color degraded services
                    let is_degraded = Self::is_replica_degraded(&svc.replicas);
                    if is_degraded {
                        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                    }

                    Self::write_selectable(&mut out, &line,
                        row_idx == ui_state.selected_index)?;
                    queue!(io::stdout(), ResetColor)?;
                    row_idx += 1;
                }
            }
        }

        // Status message
        if let Some(msg) = status_message {
            Self::writeln(&mut out, "")?;
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            Self::writeln(&mut out, &format!("  {}", msg))?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Footer
        let help = "q/Esc: Back | Tab: Switch | ↑/↓: Navigate | →: Expand/Drill | ←: Collapse/Back | R: Rolling Restart (confirm with y)";
        let help_y = size.1.saturating_sub(1);
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = size.0 as usize)),
            ResetColor
        )?;

        out.flush()?;
        Ok(())
    }

    // =====================================================================
    // Swarm task/replica list for a service
    // =====================================================================

    pub fn render_swarm_tasks(
        service_name: &str,
        tasks: &[SwarmTaskInfo],
        selected_index: usize,
        status_message: &Option<String>,
    ) -> io::Result<()> {
        let mut out = stdout();
        queue!(out, cursor::MoveTo(0, 2))?;

        let size = terminal::size()?;

        // Header
        queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        Self::writeln(&mut out, &format!("  Service: {} — Tasks/Replicas", service_name))?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
        Self::writeln(&mut out, "")?;

        if tasks.is_empty() {
            Self::writeln(&mut out, "  No tasks found for this service.")?;
        } else {
            // Column headers
            queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            write!(out, "  {:<14} {:<28} {:<18} {:<12} {:<24} {}\r\n",
                "ID", "NAME", "NODE", "DESIRED", "CURRENT STATE", "ERROR")?;
            queue!(io::stdout(), ResetColor)?;

            for (idx, task) in tasks.iter().enumerate() {
                let line = format!("  {:<14} {:<28} {:<18} {:<12} {:<24} {}",
                    truncate_str(&task.id, 12),
                    truncate_str(&task.name, 26),
                    truncate_str(&task.node, 16),
                    &task.desired_state,
                    truncate_str(&task.current_state, 22),
                    truncate_str(&task.error, 30),
                );

                // Color by state
                let state_lower = task.current_state.to_lowercase();
                if state_lower.contains("failed") || state_lower.contains("rejected") {
                    queue!(io::stdout(), SetForegroundColor(Color::Red))?;
                } else if state_lower.contains("shutdown") || state_lower.contains("complete") {
                    queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
                } else if state_lower.contains("running") {
                    queue!(io::stdout(), SetForegroundColor(Color::Green))?;
                }

                Self::write_selectable(&mut out, &line, idx == selected_index)?;
                queue!(io::stdout(), ResetColor)?;
            }
        }

        // Status message
        if let Some(msg) = status_message {
            Self::writeln(&mut out, "")?;
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            Self::writeln(&mut out, &format!("  {}", msg))?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Footer
        let help = "q/Esc/←: Back | ↑/↓: Navigate | →/L: Service Logs | R: Rolling Restart (confirm with y)";
        let help_y = size.1.saturating_sub(1);
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = size.0 as usize)),
            ResetColor
        )?;

        out.flush()?;
        Ok(())
    }

    // =====================================================================
    // Service log viewer (full-screen, like container logs)
    // =====================================================================

    pub fn render_service_logs(log_state: &ServiceLogState) -> io::Result<()> {
        let mut out = stdout();
        execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

        let size = terminal::size()?;
        let width = size.0 as usize;
        let height = size.1 as usize;

        // Header
        let follow_indicator = if log_state.auto_follow { "FOLLOWING" } else { "PAUSED" };
        let filter_indicator = if log_state.filter_errors { " | ERRORS ONLY" } else { "" };
        let search_indicator = if !log_state.search_query.is_empty() {
            format!(" | SEARCH: \"{}\"", log_state.search_query)
        } else {
            String::new()
        };
        let header = format!("  Service Logs: {} ({}) - {}{}{}",
            log_state.service_name, log_state.service_id, follow_indicator, filter_indicator, search_indicator);

        queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        if !log_state.auto_follow {
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        }
        write!(out, "{}\r\n", header)?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

        // Search prompt line (if active)
        if log_state.search_mode {
            queue!(io::stdout(), SetForegroundColor(Color::Cyan))?;
            write!(out, "  Search: {}_\r\n", log_state.search_query)?;
            queue!(io::stdout(), ResetColor)?;
        } else {
            // Separator
            let sep: String = "─".repeat(width);
            queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            write!(out, "{}\r\n", sep)?;
            queue!(io::stdout(), ResetColor)?;
        }

        // Filter lines by error and/or search
        let has_search = !log_state.search_query.is_empty();
        let query_lower = log_state.search_query.to_lowercase();

        let display_lines: Vec<&String> = log_state.lines.iter()
            .filter(|l| {
                if log_state.filter_errors {
                    let lower = l.to_lowercase();
                    if !(lower.contains("error") || lower.contains("err")
                        || lower.contains("panic") || lower.contains("fatal")
                        || lower.contains("exception") || lower.contains("fail")) {
                        return false;
                    }
                }
                if has_search {
                    if !l.to_lowercase().contains(&query_lower) {
                        return false;
                    }
                }
                true
            })
            .collect();

        let log_area_height = height.saturating_sub(4);
        let total_lines = display_lines.len();

        let start_line = if log_state.auto_follow {
            total_lines.saturating_sub(log_area_height)
        } else {
            let bottom_start = total_lines.saturating_sub(log_area_height);
            bottom_start.saturating_sub(log_state.scroll_offset)
        };

        let end_line = (start_line + log_area_height).min(total_lines);

        let mut lines_printed = 0;
        for i in start_line..end_line {
            if let Some(line) = display_lines.get(i) {
                // Highlight error lines
                let is_error = {
                    let lower = line.to_lowercase();
                    lower.contains("error") || lower.contains("panic")
                        || lower.contains("fatal") || lower.contains("exception")
                };

                // Highlight search matches
                let is_match = has_search && line.to_lowercase().contains(&query_lower);

                if is_error {
                    queue!(io::stdout(), SetForegroundColor(Color::Red))?;
                } else if is_match {
                    queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                }

                let display_line = safe_truncate(line, width);
                write!(out, "{}\r\n", display_line)?;

                if is_error || is_match {
                    queue!(io::stdout(), ResetColor)?;
                }
                lines_printed += 1;
            }
        }

        for _ in lines_printed..log_area_height {
            write!(out, "\r\n")?;
        }

        // Footer
        let help = if log_state.search_mode {
            "Type to search | Enter: Confirm | Esc: Cancel"
        } else if has_search {
            "q/Esc/←: Back | ↑/↓: Scroll | f/End: Follow | e: Errors | /: Search | n: Clear search"
        } else {
            "q/Esc/←: Back | ↑/↓: Scroll | f/End: Follow | e: Toggle Error Filter | /: Search"
        };
        let help_y = (height.saturating_sub(1)) as u16;
        queue!(
            out,
            MoveTo(1, help_y),
            SetForegroundColor(Color::DarkGrey),
            Print(format!("{:<width$}", help, width = width)),
            ResetColor
        )?;

        out.flush()?;
        Ok(())
    }

    /// Helper: check if a replica string like "2/3" indicates degraded state.
    fn is_replica_degraded(replicas: &str) -> bool {
        if replicas.contains('/') {
            let parts: Vec<&str> = replicas.split('/').collect();
            if parts.len() == 2 {
                let current: u32 = parts[0].trim().parse().unwrap_or(0);
                let desired: u32 = parts[1].trim().parse().unwrap_or(0);
                return desired > 0 && current < desired;
            }
        }
        false
    }

    // --- Section renderers ---

    fn render_summary(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        // Separator line
        Self::writeln(out, "────────────────────────────────────────────────────────────────────────────────")?;
        
        // LOAD line
        let (l1, l5, l15) = data.load_avg;
        let cores = data.core_count;
        let mut load_line = String::from("LOAD   ");
        
        // Build load values with color
        for (label, val) in [("1m:", l1), ("5m:", l5), ("15m:", l15)] {
            load_line.push_str(&format!("{} ", label));
            if val > cores {
                load_line.push_str(&format!("\x1b[31m{:.2}\x1b[0m  ", val)); // red
            } else {
                load_line.push_str(&format!("{:.2}  ", val));
            }
        }
        
        // Add cores and overload status
        let overload_status = if l1 > cores && l5 > cores && l15 > cores {
            "\x1b[31moverload: all\x1b[0m"
        } else if l1 > cores && l5 > cores {
            "\x1b[31moverload: 1m+5m\x1b[0m"
        } else if l1 > cores {
            "\x1b[31moverload: 1m\x1b[0m"
        } else {
            "overload: none"
        };
        load_line.push_str(&format!("│ cores: {}  │ {}", cores as u32, overload_status));
        Self::writeln(out, &load_line)?;
        
        // MEM line
        let m = &data.memory;
        let total_gb = m.total as f64 / 1_073_741_824.0;
        let used_gb = m.used as f64 / 1_073_741_824.0;
        let avail_gb = m.available as f64 / 1_073_741_824.0;
        let pct_used = if m.total > 0 { (m.used as f64 / m.total as f64) * 100.0 } else { 0.0 };
        
        let mem_bar = Self::progress_bar(pct_used, 20);
        let mem_color = if pct_used > 85.0 {
            "\x1b[31m" // red
        } else if pct_used > 70.0 {
            "\x1b[33m" // yellow
        } else {
            ""
        };
        Self::writeln(out, &format!(
            "MEM    {}{}\x1b[0m {:.1}/{:.1} GB ({:.0}%)  │ avail: {:.1} GB",
            mem_color, mem_bar, used_gb, total_gb, pct_used, avail_gb
        ))?;
        
        // SWAP line
        if m.swap_total > 0 {
            let swap_total_gb = m.swap_total as f64 / 1_073_741_824.0;
            let swap_used_gb = m.swap_used as f64 / 1_073_741_824.0;
            let swap_pct = (m.swap_used as f64 / m.swap_total as f64) * 100.0;
            let swap_bar = Self::progress_bar(swap_pct, 20);
            
            let swap_color = if swap_pct > 50.0 {
                "\x1b[31m" // red - swap usage is bad
            } else if swap_pct > 20.0 {
                "\x1b[33m" // yellow
            } else {
                ""
            };
            Self::writeln(out, &format!(
                "SWAP   {}{}\x1b[0m {:.1}/{:.1} GB ({:.0}%)",
                swap_color, swap_bar, swap_used_gb, swap_total_gb, swap_pct
            ))?;
        } else {
            Self::writeln(out, "SWAP   none")?;
        }
        
        // DISK lines
        if data.disk_space.is_empty() {
            Self::writeln(out, "DISK   all disks > 10% free")?;
        } else {
            for (idx, d) in data.disk_space.iter().enumerate() {
                let label = if idx == 0 { "DISK   " } else { "       " };
                let used_gb = d.total_gb - d.available_gb;
                let pct_used = 100.0 - d.percent_free;
                let disk_bar = Self::progress_bar(pct_used, 20);
                
                let warning = if d.percent_free < 10.0 {
                    " \x1b[31m⚠ LOW\x1b[0m"
                } else if d.percent_free < 20.0 {
                    " \x1b[33m⚠ LOW\x1b[0m"
                } else {
                    ""
                };
                
                Self::writeln(out, &format!(
                    "{}{:<10} {} {:.1}/{:.1} GB ({:.0}% free){}",
                    label, d.mount_point, disk_bar, used_gb, d.total_gb, d.percent_free, warning
                ))?;
            }
        }
        
        // Disk Busy
        Self::writeln(out, &format!("IO     Busy: {:.1} MB/s", data.disk_busy_pct))?;
        
        // FD line
        let f = &data.fd_info;
        let fd_pct = if f.system_max > 0 { (f.system_used as f64 / f.system_max as f64) * 100.0 } else { 0.0 };
        let fd_color = if fd_pct > 80.0 {
            "\x1b[31m"
        } else if fd_pct > 60.0 {
            "\x1b[33m"
        } else {
            ""
        };
        
        let top_fd = f.top_processes.iter()
            .take(3)
            .map(|(name, count)| format!("{}({})", name, count))
            .collect::<Vec<_>>()
            .join(" ");
        
        Self::writeln(out, &format!(
            "FD     {}{} / {} ({:.0}%)\x1b[0m  │ top: {}",
            fd_color, Self::format_number(f.system_used), Self::format_number(f.system_max), fd_pct, top_fd
        ))?;
        
        // SOCK line
        let s = &data.socket_overview;
        let time_wait_color = if s.time_wait > 100 { "\x1b[33m" } else { "" };
        let close_wait_color = if s.close_wait > 10 { "\x1b[31m" } else { "" };
        
        Self::writeln(out, &format!(
            "SOCK   EST: {}  LISTEN: {}  {}TIME_WAIT: {}\x1b[0m  {}CLOSE_WAIT: {}\x1b[0m  FIN_WAIT: {}",
            s.established, s.listen, time_wait_color, s.time_wait, 
            close_wait_color, s.close_wait, s.fin_wait
        ))?;
        
        // NET lines (Network & Bandwidth)
        let n = &data.network;
        if n.interfaces.is_empty() {
            Self::writeln(out, "NET    no active traffic")?;
        } else {
            for (idx, iface) in n.interfaces.iter().enumerate() {
                let label = if idx == 0 { "NET    " } else { "       " };
                Self::writeln(out, &format!(
                    "{}{:<10} ↓ {}  ↑ {}",
                    label, iface.name,
                    Self::format_bytes_rate(iface.rx_rate),
                    Self::format_bytes_rate(iface.tx_rate)
                ))?;
            }
        }
        
        // Separator line
        Self::writeln(out, "────────────────────────────────────────────────────────────────────────────────")?;
        
        Ok(())
    }








    fn render_processes(
        out: &mut impl Write, data: &MonitorData, ui_state: &UIState,
        rows: &mut Vec<(Pid, RowKind)>, current_row: &mut usize,
    ) -> io::Result<()> {
        // Header with highlighting
        write!(out, "  {:<18} ", "PID[CHILDREN]")?;

        let headers = [
            ("CPU %", 8, Some(crate::model::SortColumn::Cpu)),
            ("MEM (MB)", 10, Some(crate::model::SortColumn::Memory)),
            ("READ/s", 10, Some(crate::model::SortColumn::Read)),
            ("WRITE/s", 10, Some(crate::model::SortColumn::Write)),
            ("NET ↓", 10, Some(crate::model::SortColumn::NetDown)),
            ("NET ↑", 10, Some(crate::model::SortColumn::NetUp)),
            ("Name", 0, None),
        ];

        for (text, width, col) in headers {
            let is_sorted = col.map_or(false, |c| c == ui_state.sort_column);
            if is_sorted {
                queue!(out, SetAttribute(Attribute::Reverse))?;
            }
            if width > 0 {
                write!(out, "{:<width$} ", text, width = width)?;
            } else {
                write!(out, "{}", text)?;
            }
            if is_sorted {
                queue!(out, SetAttribute(Attribute::Reset))?;
            }
        }
        write!(out, "\r\n")?;
            
        for g in &data.historical_top {
            let pid_label = format!("{}[{}]", g.pid, g.child_count);
            let mem_mb = g.mem as f64 / 1_048_576.0;
            let read_fmt = Self::format_bytes_rate(g.read_bytes);
            let write_fmt = Self::format_bytes_rate(g.written_bytes);
            let net_rx_fmt = Self::format_bytes_rate(g.net_rx_bytes);
            let net_tx_fmt = Self::format_bytes_rate(g.net_tx_bytes);
            
            let line = format!("  {:<18} {:<8.1} {:<10.1} {:<10} {:<10} {:<10} {:<10} {}", 
                pid_label, g.cpu, mem_mb, read_fmt, write_fmt, net_rx_fmt, net_tx_fmt, g.name);
                
            Self::write_selectable(out, &line, *current_row == ui_state.selected_index)?;
            rows.push((g.pid, RowKind::ProcessParent));
            *current_row += 1;

            if ui_state.expanded_pids.contains(&g.pid) {
                for child in &g.children {
                    let child_read = Self::format_bytes_rate(child.read_bytes);
                    let child_write = Self::format_bytes_rate(child.written_bytes);
                    let child_net_rx = Self::format_bytes_rate(child.net_rx_bytes);
                    let child_net_tx = Self::format_bytes_rate(child.net_tx_bytes);
                    
                    let child_line = format!("    {:<16} {:<8.1} {:<10.1} {:<10} {:<10} {:<10} {:<10} {}",
                        child.pid, child.cpu, child.mem as f64 / 1_048_576.0, 
                        child_read, child_write, child_net_rx, child_net_tx, child.name);
                        
                    Self::write_selectable(out, &child_line, *current_row == ui_state.selected_index)?;
                    rows.push((child.pid, RowKind::ProcessChild));
                    *current_row += 1;
                }
            }
        }
        Ok(())
    }

    fn render_network(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let n = &data.network;
        Self::writeln(out, &format!("  Connections:  ESTABLISHED: {}  TIME_WAIT: {}  CLOSE_WAIT: {}",
            n.established, n.time_wait, n.close_wait))?;

        // Interface bandwidth
        if n.interfaces.is_empty() {
            Self::writeln(out, "  Bandwidth: No active traffic")?;
        } else {
            for iface in &n.interfaces {
                Self::writeln(out, &format!("  {:<10}  ↓ {}  ↑ {}",
                    iface.name,
                    Self::format_bytes_rate(iface.rx_rate),
                    Self::format_bytes_rate(iface.tx_rate)))?;
            }
        }

        // Top bandwidth processes
        if !n.top_bandwidth_processes.is_empty() {
            Self::writeln(out, "  Top processes by network connections:")?;
            for p in &n.top_bandwidth_processes {
                Self::writeln(out, &format!("    {:<25} {} connections", p.name, p.bandwidth))?;
            }
        }
        Ok(())
    }

    fn render_fd_info(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let f = &data.fd_info;
        let pct = if f.system_max > 0 { (f.system_used as f64 / f.system_max as f64) * 100.0 } else { 0.0 };
        let color = if pct > 80.0 { Color::Red } else if pct > 50.0 { Color::Yellow } else { Color::Green };
        queue!(io::stdout(), SetForegroundColor(color))?;
        Self::writeln(out, &format!("  System: {} / {} ({:.1}%)", f.system_used, f.system_max, pct))?;
        queue!(io::stdout(), ResetColor)?;
        if !f.top_processes.is_empty() {
            Self::writeln(out, "  Top processes by open files:")?;
            for (name, count) in &f.top_processes {
                Self::writeln(out, &format!("    {:<25} {}", name, count))?;
            }
        }
        Ok(())
    }



    fn render_socket_overview(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let s = &data.socket_overview;
        Self::writeln(out, &format!(
            "  ESTABLISHED: {}  LISTEN: {}  TIME_WAIT: {}  CLOSE_WAIT: {}  FIN_WAIT: {}",
            s.established, s.listen, s.time_wait, s.close_wait, s.fin_wait
        ))?;

        if s.close_wait > 10 {
            queue!(io::stdout(), SetForegroundColor(Color::Red))?;
            Self::writeln(out, &format!("  ⚠ High CLOSE_WAIT count ({}) — possible connection leak", s.close_wait))?;
            queue!(io::stdout(), ResetColor)?;
        }
        if s.time_wait > 100 {
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            Self::writeln(out, &format!("  ⚠ High TIME_WAIT count ({}) — high connection churn", s.time_wait))?;
            queue!(io::stdout(), ResetColor)?;
        }

        if !s.top_processes.is_empty() {
            Self::writeln(out, "  Top processes by open connections:")?;
            for (name, count) in &s.top_processes {
                Self::writeln(out, &format!("    {:<25} {}", name, count))?;
            }
        }
        Ok(())
    }

    // =====================================================================
    // Confirmation banner (rendered over the status bar area)
    // =====================================================================

    pub fn render_confirmation(prompt: &str) -> io::Result<()> {
        let mut out = stdout();
        let size = terminal::size()?;
        let y = size.1.saturating_sub(3);
        let width = size.0 as usize;

        // Clear the confirmation area
        queue!(out, MoveTo(0, y))?;
        queue!(out, SetBackgroundColor(Color::DarkRed), SetForegroundColor(Color::White), SetAttribute(Attribute::Bold))?;
        let line = format!("  {} (y to confirm, any other key to cancel)  ", prompt);
        write!(out, "{:<width$}", line, width = width)?;
        queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
        out.flush()?;
        Ok(())
    }

    // --- Helpers ---

    fn writeln(out: &mut impl Write, text: &str) -> io::Result<()> {
        write!(out, "{}\r\n", text)
    }

    fn write_section_header(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
        if selected {
            queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
        } else {
            queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        }
        write!(out, "{}\r\n", text)?;
        queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
        Ok(())
    }

    fn write_selectable(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
        if selected {
            queue!(io::stdout(), SetBackgroundColor(Color::DarkGrey), SetForegroundColor(Color::White))?;
        }
        write!(out, "{}\r\n", text)?;
        if selected {
            queue!(io::stdout(), ResetColor)?;
        }
        Ok(())
    }

    fn format_bytes_rate(bytes: u64) -> String {
        if bytes > 1_048_576 {
            format!("{:.2} MB/s", bytes as f64 / 1_048_576.0)
        } else if bytes > 1024 {
            format!("{:.2} KB/s", bytes as f64 / 1024.0)
        } else {
            format!("{} B/s", bytes)
        }
    }

    fn progress_bar(percent: f64, width: usize) -> String {
        let filled = ((percent / 100.0) * width as f64).round() as usize;
        let empty = width.saturating_sub(filled);
        format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
    }

    fn format_number(n: u64) -> String {
        let s = n.to_string();
        let bytes = s.as_bytes();
        let mut result = String::new();
        for (i, &b) in bytes.iter().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(b as char);
        }
        result.chars().rev().collect()
    }
}

/// Truncate a string to at most `max_len` characters (not bytes), appending "..."
/// if truncated. Safe for multi-byte UTF-8.
fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let keep = max_len.saturating_sub(3);
        let truncated: String = s.chars().take(keep).collect();
        format!("{}...", truncated)
    }
}

/// Truncate a string to at most `max_len` characters for display. Returns a &str
/// slice up to the last valid char boundary within `max_len` bytes.
fn safe_truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
