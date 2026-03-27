use crossterm::{
    cursor, queue,
    style::{Attribute, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use std::collections::HashMap;
use std::io::{self, stdout, Write};

use super::shared::{truncate_str, write_selectable, writeln};
use super::theme::theme;
use crate::model::{
    SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo, SwarmTaskInfo, SwarmUIState,
};

/// Check if a replica string like "2/3" indicates degraded state.
pub fn is_replica_degraded(replicas: &str) -> bool {
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

/// Check if replicas are completely failed (0/N where N > 0).
fn is_replica_failed(replicas: &str) -> bool {
    if replicas.contains('/') {
        let parts: Vec<&str> = replicas.split('/').collect();
        if parts.len() == 2 {
            let current: u32 = parts[0].trim().parse().unwrap_or(0);
            let desired: u32 = parts[1].trim().parse().unwrap_or(0);
            return desired > 0 && current == 0;
        }
    }
    false
}

/// Render a themed help footer at the last row.
fn render_help_footer(out: &mut impl Write, items: &[(&str, &str)], width: u16, y: u16) -> io::Result<()> {
    let t = theme();
    queue!(out, cursor::MoveTo(1, y))?;
    for (i, (key, desc)) in items.iter().enumerate() {
        if i > 0 {
            queue!(out, SetForegroundColor(t.help_desc))?;
            write!(out, "  ")?;
        }
        queue!(out, SetForegroundColor(t.help_key))?;
        write!(out, "{}", key)?;
        queue!(out, SetForegroundColor(t.help_desc))?;
        write!(out, ":{}", desc)?;
    }
    // Pad remaining width
    let content_len: usize = items.iter().map(|(k, d)| k.len() + 1 + d.len()).sum::<usize>() + (items.len().saturating_sub(1)) * 2;
    let remaining = (width as usize).saturating_sub(content_len + 1);
    write!(out, "{:remaining$}", "", remaining = remaining)?;
    queue!(out, ResetColor)?;
    Ok(())
}

pub fn render_swarm_overview(
    cluster_info: &Option<SwarmClusterInfo>,
    nodes: &[SwarmNodeInfo],
    stacks: &[SwarmStackInfo],
    services: &[SwarmServiceInfo],
    ui_state: &SwarmUIState,
    warnings: &[String],
    status_message: &Option<String>,
    service_tasks: &HashMap<String, Vec<SwarmTaskInfo>>,
) -> io::Result<()> {
    let t = theme();
    let mut out = stdout();
    queue!(out, cursor::MoveTo(0, 2))?;

    let size = crossterm::terminal::size()?;

    queue!(
        io::stdout(),
        SetForegroundColor(t.lavender),
        SetAttribute(Attribute::Bold)
    )?;
    writeln(&mut out, "  Swarm Cluster")?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;
    writeln(&mut out, "")?;

    if let Some(info) = cluster_info {
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        write!(out, "  Cluster: ")?;
        queue!(io::stdout(), SetForegroundColor(t.teal))?;
        write!(out, "{}", info.nodes_total)?;
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        write!(out, " nodes (")?;
        queue!(io::stdout(), SetForegroundColor(t.teal))?;
        write!(out, "{}", info.managers)?;
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        write!(out, " managers) | This node: ")?;
        queue!(io::stdout(), SetForegroundColor(t.teal))?;
        write!(
            out,
            "{}\r\n",
            if info.is_manager { "manager" } else { "worker" }
        )?;
        queue!(io::stdout(), ResetColor)?;
    }
    writeln(&mut out, "")?;

    if !warnings.is_empty() {
        for w in warnings {
            queue!(
                io::stdout(),
                SetForegroundColor(t.red),
                SetAttribute(Attribute::Bold)
            )?;
            writeln(&mut out, &format!("  ⚠ {}", w))?;
            queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
        }
        writeln(&mut out, "")?;
    }

    let mut row_idx: usize = 0;

    let nodes_expanded = ui_state.expanded_ids.contains("__nodes__");
    let node_indicator = if nodes_expanded { "▼" } else { "▶" };
    // Section header with teal indicator and lavender label
    let node_header = format!("  {} NODES ({})", node_indicator, nodes.len());
    // Render with themed selection
    if row_idx == ui_state.selected_index {
        queue!(io::stdout(), SetBackgroundColor(t.selected_bg), SetForegroundColor(t.selected_fg))?;
        write!(out, "{}\r\n", node_header)?;
        queue!(io::stdout(), ResetColor)?;
    } else {
        queue!(io::stdout(), SetForegroundColor(t.lavender), SetAttribute(Attribute::Bold))?;
        write!(out, "{}\r\n", node_header)?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;
    }
    row_idx += 1;

    if nodes_expanded {
        queue!(
            io::stdout(),
            SetForegroundColor(t.header_fg),
            SetAttribute(Attribute::Bold)
        )?;
        write!(
            out,
            "    {:<14} {:<20} {:<16} {:<10} {:<12} {:<14} {}\r\n",
            "ID", "HOSTNAME", "IP", "STATUS", "AVAIL", "ROLE", "ENGINE"
        )?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

        for node in nodes {
            let role = if !node.manager_status.is_empty() {
                &node.manager_status
            } else {
                "worker"
            };
            let self_marker = if node.is_self { " *" } else { "" };
            let ip_display = if node.ip_address.is_empty() {
                "—"
            } else {
                &node.ip_address
            };
            let line = format!(
                "    {:<14} {:<20} {:<16} {:<10} {:<12} {:<14} {}{}",
                truncate_str(&node.id, 12),
                truncate_str(&node.hostname, 18),
                truncate_str(ip_display, 15),
                &node.status,
                &node.availability,
                role,
                &node.engine_version,
                self_marker,
            );

            // Color based on node status
            let status_lower = node.status.to_lowercase();
            let avail_lower = node.availability.to_lowercase();
            if status_lower.contains("down") {
                queue!(io::stdout(), SetForegroundColor(t.red))?;
            } else if avail_lower.contains("drain") {
                queue!(io::stdout(), SetForegroundColor(t.yellow))?;
            } else if status_lower.contains("ready") {
                queue!(io::stdout(), SetForegroundColor(t.green))?;
            }

            // Manager status coloring is embedded in the line; apply general row color
            write_selectable(&mut out, &line, row_idx == ui_state.selected_index)?;
            queue!(io::stdout(), ResetColor)?;
            row_idx += 1;
        }
    }

    writeln(&mut out, "")?;

    for stack in stacks {
        let stack_expanded = ui_state.expanded_ids.contains(&stack.name);
        let indicator = if stack_expanded { "▼" } else { "▶" };
        let svc_count = stack.service_indices.len();
        let stack_header = format!(
            "  {} STACK: {} ({} services)",
            indicator, stack.name, svc_count
        );

        if row_idx == ui_state.selected_index {
            queue!(io::stdout(), SetBackgroundColor(t.selected_bg), SetForegroundColor(t.selected_fg))?;
            write!(out, "{}\r\n", stack_header)?;
            queue!(io::stdout(), ResetColor)?;
        } else {
            queue!(io::stdout(), SetForegroundColor(t.lavender), SetAttribute(Attribute::Bold))?;
            write!(out, "{}\r\n", stack_header)?;
            queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;
        }
        row_idx += 1;

        if stack_expanded {
            queue!(
                io::stdout(),
                SetForegroundColor(t.header_fg),
                SetAttribute(Attribute::Bold)
            )?;
            write!(
                out,
                "    {:<14} {:<28} {:<12} {:<10} {:<20} {}\r\n",
                "ID", "NAME", "MODE", "REPLICAS", "IMAGE", "PORTS"
            )?;
            queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

            // Build hostname -> IP lookup from nodes
            let node_ip_map: HashMap<&str, &str> = nodes
                .iter()
                .filter(|n| !n.ip_address.is_empty())
                .map(|n| (n.hostname.as_str(), n.ip_address.as_str()))
                .collect();

            for &idx in &stack.service_indices {
                let svc = &services[idx];
                let line = format!(
                    "    {:<14} {:<28} {:<12} {:<10} {:<20} {}",
                    truncate_str(&svc.id, 12),
                    truncate_str(&svc.name, 26),
                    &svc.mode,
                    &svc.replicas,
                    truncate_str(&svc.image, 18),
                    truncate_str(&svc.ports, 20),
                );

                if is_replica_failed(&svc.replicas) {
                    queue!(io::stdout(), SetForegroundColor(t.red))?;
                } else if is_replica_degraded(&svc.replicas) {
                    queue!(io::stdout(), SetForegroundColor(t.peach))?;
                }

                write_selectable(&mut out, &line, row_idx == ui_state.selected_index)?;
                queue!(io::stdout(), ResetColor)?;
                row_idx += 1;

                // Render inline replica sub-rows (non-selectable)
                if let Some(tasks) = service_tasks.get(&svc.id) {
                    for task in tasks {
                        // Extract short name: strip the service name prefix, keep ".N" suffix
                        let short_name = if let Some(dot_pos) = task.name.rfind('.') {
                            &task.name[dot_pos..]
                        } else {
                            &task.name
                        };
                        let node_ip = node_ip_map.get(task.node.as_str()).copied().unwrap_or("—");

                        let sub_line = format!(
                            "                       └ {:<20} {:<18} {:<16} {}",
                            short_name,
                            truncate_str(&task.node, 16),
                            truncate_str(node_ip, 14),
                            truncate_str(&task.current_state, 24),
                        );

                        // Color task sub-rows by state
                        let state_lower = task.current_state.to_lowercase();
                        if state_lower.contains("running") {
                            queue!(io::stdout(), SetForegroundColor(t.green))?;
                        } else if state_lower.contains("failed") || state_lower.contains("rejected") {
                            queue!(io::stdout(), SetForegroundColor(t.red))?;
                        } else if state_lower.contains("shutdown") || state_lower.contains("complete") {
                            queue!(io::stdout(), SetForegroundColor(t.subtext))?;
                        } else {
                            queue!(io::stdout(), SetForegroundColor(t.subtext))?;
                        }
                        writeln(&mut out, &sub_line)?;
                        queue!(io::stdout(), ResetColor)?;
                    }
                }
            }
        }
    }

    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(io::stdout(), SetForegroundColor(t.yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(io::stdout(), ResetColor)?;
    }

    let help_y = size.1.saturating_sub(1);
    render_help_footer(&mut out, &[
        ("q", "Quit"),
        ("↑↓", "Select"),
        ("Enter", "Expand"),
        ("L", "Logs"),
        ("S", "Scale"),
        ("R", "Restart"),
        ("Tab", "Next"),
    ], size.0, help_y)?;

    out.flush()?;
    Ok(())
}

pub fn render_swarm_tasks(
    service_name: &str,
    tasks: &[SwarmTaskInfo],
    nodes: &[SwarmNodeInfo],
    selected_index: usize,
    status_message: &Option<String>,
) -> io::Result<()> {
    let t = theme();
    let mut out = stdout();
    queue!(out, cursor::MoveTo(0, 2))?;

    let size = crossterm::terminal::size()?;

    // Build hostname -> IP lookup
    let node_ip_map: HashMap<&str, &str> = nodes
        .iter()
        .filter(|n| !n.ip_address.is_empty())
        .map(|n| (n.hostname.as_str(), n.ip_address.as_str()))
        .collect();

    queue!(
        io::stdout(),
        SetForegroundColor(t.lavender),
        SetAttribute(Attribute::Bold)
    )?;
    writeln(&mut out, &format!("  Swarm › Tasks: {}", service_name))?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;
    writeln(&mut out, "")?;

    if tasks.is_empty() {
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        writeln(&mut out, "  No tasks found for this service.")?;
        queue!(io::stdout(), ResetColor)?;
    } else {
        queue!(
            io::stdout(),
            SetForegroundColor(t.header_fg),
            SetAttribute(Attribute::Bold)
        )?;
        write!(
            out,
            "  {:<14} {:<28} {:<18} {:<16} {:<12} {:<24} {}\r\n",
            "ID", "NAME", "NODE", "IP", "DESIRED", "CURRENT STATE", "ERROR"
        )?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

        for (idx, task) in tasks.iter().enumerate() {
            let node_ip = node_ip_map.get(task.node.as_str()).copied().unwrap_or("—");
            let line = format!(
                "  {:<14} {:<28} {:<18} {:<16} {:<12} {:<24} {}",
                truncate_str(&task.id, 12),
                truncate_str(&task.name, 26),
                truncate_str(&task.node, 16),
                truncate_str(node_ip, 14),
                &task.desired_state,
                truncate_str(&task.current_state, 22),
                truncate_str(&task.error, 30),
            );

            let state_lower = task.current_state.to_lowercase();
            if state_lower.contains("failed") || state_lower.contains("rejected") {
                queue!(io::stdout(), SetForegroundColor(t.red))?;
            } else if state_lower.contains("shutdown") || state_lower.contains("complete") {
                queue!(io::stdout(), SetForegroundColor(t.subtext))?;
            } else if state_lower.contains("running") {
                queue!(io::stdout(), SetForegroundColor(t.green))?;
            }

            write_selectable(&mut out, &line, idx == selected_index)?;
            queue!(io::stdout(), ResetColor)?;
        }
    }

    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(io::stdout(), SetForegroundColor(t.yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(io::stdout(), ResetColor)?;
    }

    let help_y = size.1.saturating_sub(1);
    render_help_footer(&mut out, &[
        ("q/Esc/←", "Back"),
        ("↑↓", "Navigate"),
        ("→/L", "Service Logs"),
        ("R", "Rolling Restart"),
    ], size.0, help_y)?;

    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_replica_degraded_ok() {
        assert!(!is_replica_degraded("3/3"));
        assert!(!is_replica_degraded("1/1"));
    }

    #[test]
    fn is_replica_degraded_degraded() {
        assert!(is_replica_degraded("2/3"));
        assert!(is_replica_degraded("0/1"));
    }

    #[test]
    fn is_replica_degraded_invalid() {
        assert!(!is_replica_degraded(""));
        assert!(!is_replica_degraded("running"));
    }
}
