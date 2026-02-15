use std::io::{self, Write, stdout};
use crossterm::{cursor, queue, style::{Color, SetForegroundColor, ResetColor, SetAttribute, Attribute}};

use crate::model::{DockerContainerInfo, ContainerUIState};
use super::shared::{truncate_str, writeln, write_selectable};

pub fn render_containers(
    containers: &[DockerContainerInfo],
    ui_state: &ContainerUIState,
    status_message: &Option<String>,
) -> io::Result<()> {
    let mut out = stdout();
    queue!(out, cursor::MoveTo(0, 2))?;

    let size = crossterm::terminal::size()?;

    if containers.is_empty() {
        writeln(&mut out, "")?;
        writeln(&mut out, "  No running containers found.")?;
        writeln(&mut out, "")?;
        writeln(&mut out, "  Make sure Docker is running and you have containers up.")?;
    } else {
        writeln(&mut out, "")?;

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

            write_selectable(&mut out, &line, selected)?;

            // Expanded detail
            if ui_state.expanded_ids.contains(&c.id) {
                queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
                writeln(&mut out, &format!("    Image:  {}", c.image))?;
                writeln(&mut out, &format!("    Status: {}", c.status))?;
                writeln(&mut out, &format!("    Ports:  {}", c.ports))?;
                writeln(&mut out, &format!("    IP:     {}", c.ip_address))?;
                queue!(io::stdout(), ResetColor)?;
            }
        }
    }

    // Status message (action feedback)
    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(io::stdout(), ResetColor)?;
    }

    // Footer
    let help = "q/Esc: Back | Tab: Switch | ↑/↓: Navigate | →: Logs | S: Start | T: Stop | R: Restart (confirm with y)";
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
