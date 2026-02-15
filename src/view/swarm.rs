use std::io::{self, Write, stdout};
use crossterm::{cursor, queue, style::{Color, SetForegroundColor, ResetColor, SetAttribute, Attribute}};

use crate::model::{SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo, SwarmTaskInfo, SwarmUIState};
use super::shared::{truncate_str, writeln, write_selectable};

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

    let size = crossterm::terminal::size()?;

    queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
    writeln(&mut out, "  Swarm Cluster")?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
    writeln(&mut out, "")?;

    if let Some(info) = cluster_info {
        queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        write!(out, "  Cluster: {} nodes ({} managers) | This node: {}\r\n",
            info.nodes_total, info.managers,
            if info.is_manager { "manager" } else { "worker" })?;
        queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
    }
    writeln(&mut out, "")?;

    if !warnings.is_empty() {
        for w in warnings {
            queue!(io::stdout(), SetForegroundColor(Color::Red), SetAttribute(Attribute::Bold))?;
            writeln(&mut out, &format!("  ⚠ {}", w))?;
            queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
        }
        writeln(&mut out, "")?;
    }

    let mut row_idx: usize = 0;

    let nodes_expanded = ui_state.expanded_ids.contains("__nodes__");
    let node_indicator = if nodes_expanded { "▼" } else { "▶" };
    let node_header = format!("  {} NODES ({})", node_indicator, nodes.len());
    write_selectable(&mut out, &node_header, row_idx == ui_state.selected_index)?;
    row_idx += 1;

    if nodes_expanded {
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

            if node.status.to_lowercase().contains("down") {
                queue!(io::stdout(), SetForegroundColor(Color::Red))?;
            } else if node.availability.to_lowercase().contains("drain") {
                queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            }

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
        let stack_header = format!("  {} STACK: {} ({} services)", indicator, stack.name, svc_count);

        write_selectable(&mut out, &stack_header, row_idx == ui_state.selected_index)?;
        row_idx += 1;

        if stack_expanded {
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

                if is_replica_degraded(&svc.replicas) {
                    queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                }

                write_selectable(&mut out, &line, row_idx == ui_state.selected_index)?;
                queue!(io::stdout(), ResetColor)?;
                row_idx += 1;
            }
        }
    }

    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(io::stdout(), ResetColor)?;
    }

    let help = "q/Esc: Back | Tab: Switch | ↑/↓: Navigate | →: Expand/Drill | ←: Collapse/Back | R: Rolling Restart (confirm with y)";
    let help_y = size.1.saturating_sub(1);
    queue!(
        out,
        cursor::MoveTo(1, help_y),
        SetForegroundColor(Color::DarkGrey),
        crossterm::style::Print(format!("{:<width$}", help, width = size.0 as usize)),
        ResetColor
    )?;

    out.flush()?;
    Ok(())
}

pub fn render_swarm_tasks(
    service_name: &str,
    tasks: &[SwarmTaskInfo],
    selected_index: usize,
    status_message: &Option<String>,
) -> io::Result<()> {
    let mut out = stdout();
    queue!(out, cursor::MoveTo(0, 2))?;

    let size = crossterm::terminal::size()?;

    queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
    writeln(&mut out, &format!("  Swarm › Tasks: {}", service_name))?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset))?;
    writeln(&mut out, "")?;

    if tasks.is_empty() {
        writeln(&mut out, "  No tasks found for this service.")?;
    } else {
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

            let state_lower = task.current_state.to_lowercase();
            if state_lower.contains("failed") || state_lower.contains("rejected") {
                queue!(io::stdout(), SetForegroundColor(Color::Red))?;
            } else if state_lower.contains("shutdown") || state_lower.contains("complete") {
                queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
            } else if state_lower.contains("running") {
                queue!(io::stdout(), SetForegroundColor(Color::Green))?;
            }

            write_selectable(&mut out, &line, idx == selected_index)?;
            queue!(io::stdout(), ResetColor)?;
        }
    }

    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(io::stdout(), ResetColor)?;
    }

    let help = "q/Esc/←: Back | ↑/↓: Navigate | →/L: Service Logs | R: Rolling Restart (confirm with y)";
    let help_y = size.1.saturating_sub(1);
    queue!(
        out,
        cursor::MoveTo(1, help_y),
        SetForegroundColor(Color::DarkGrey),
        crossterm::style::Print(format!("{:<width$}", help, width = size.0 as usize)),
        ResetColor
    )?;

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
