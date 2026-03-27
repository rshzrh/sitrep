use crossterm::{
    cursor, queue,
    style::{Attribute, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use std::io::{self, stdout, Write};

use super::shared::{render_help_footer, safe_truncate, writeln};
use super::theme::theme;
use crate::model::{ContainerUIState, DockerContainerInfo};

/// Build a 6-char inline CPU bar using `|` for filled and space for empty.
fn cpu_bar(percent: f64) -> String {
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * 6.0).round() as usize;
    let empty = 6_usize.saturating_sub(filled);
    format!("[{}{}]", "|".repeat(filled), " ".repeat(empty))
}

pub fn render_containers(
    containers: &[DockerContainerInfo],
    ui_state: &ContainerUIState,
    status_message: &Option<String>,
) -> io::Result<()> {
    let t = theme();
    let mut out = stdout();
    let (width, _height) = crossterm::terminal::size()?;
    let w = width as usize;

    queue!(out, cursor::MoveTo(0, 2))?;

    if containers.is_empty() {
        queue!(out, SetForegroundColor(t.subtext))?;
        writeln(&mut out, "")?;
        writeln(&mut out, "  No running containers found.")?;
        writeln(&mut out, "")?;
        writeln(
            &mut out,
            "  Make sure Docker is running and you have containers up.",
        )?;
        queue!(out, ResetColor)?;
    } else {
        // Column header
        let header = format!(
            "  {:<4}{:<15}{:<20}{:<11}{:<9}{:<16}{:<28}{}",
            "##", "CONTAINER ID", "NAME", "STATE", "UPTIME", "CPU", "PORTS", "IP"
        );
        queue!(
            out,
            SetForegroundColor(t.header_fg),
            SetAttribute(Attribute::Bold),
        )?;
        write!(out, "{:<w$}\r\n", header, w = w)?;
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;

        for (idx, c) in containers.iter().enumerate() {
            let selected = idx == ui_state.selected_index;
            let is_multi_selected = ui_state.selected_containers.contains(&c.id);

            // Multi-select marker
            let marker = if is_multi_selected { "[*]" } else { "[ ]" };

            let name_trunc = safe_truncate(&c.name, 18);
            let ports_trunc = safe_truncate(&c.ports, 26);
            let bar = cpu_bar(c.cpu_percent);
            let cpu_str = format!("{} {:>5.1}%", bar, c.cpu_percent);

            // Set background for selected row
            if selected {
                queue!(
                    out,
                    SetBackgroundColor(t.selected_bg),
                    SetForegroundColor(t.selected_fg),
                )?;
            }

            // Write the marker column
            write!(out, "  ")?;
            if is_multi_selected {
                // Mauve asterisk
                if !selected {
                    queue!(out, SetForegroundColor(t.mauve))?;
                }
                write!(out, "{:<4}", marker)?;
                if !selected {
                    queue!(out, ResetColor)?;
                }
            } else {
                if selected {
                    // Keep selected colors
                } else {
                    queue!(out, SetForegroundColor(t.text))?;
                }
                write!(out, "{:<4}", marker)?;
                if !selected {
                    queue!(out, ResetColor)?;
                }
            }

            // Container ID
            if selected {
                // already have selected colors
            } else {
                queue!(out, SetForegroundColor(t.text))?;
            }
            write!(out, "{:<15}", safe_truncate(&c.id, 14))?;

            // Name
            write!(out, "{:<20}", name_trunc)?;

            // State with color
            let state_lower = c.state.to_lowercase();
            if !selected {
                let state_color = if state_lower == "running" {
                    t.green
                } else if state_lower == "exited" || state_lower == "stopped" {
                    t.red
                } else if state_lower == "paused" {
                    t.yellow
                } else {
                    t.subtext
                };
                queue!(out, SetForegroundColor(state_color))?;
            }
            write!(out, "{:<11}", safe_truncate(&c.state, 10))?;

            // Uptime
            if !selected {
                queue!(out, SetForegroundColor(t.text))?;
            }
            write!(out, "{:<9}", safe_truncate(&c.uptime, 8))?;

            // CPU bar + percent
            if !selected {
                // bar in teal, percent in text
                queue!(out, SetForegroundColor(t.bar_filled))?;
                write!(out, "{}", bar)?;
                queue!(out, SetForegroundColor(t.text))?;
                write!(out, " {:>5.1}%  ", c.cpu_percent)?;
            } else {
                write!(out, "{}  ", cpu_str)?;
            }

            // Ports
            write!(out, "{:<28}", ports_trunc)?;

            // IP
            write!(out, "{}", c.ip_address)?;

            // Pad to full width if selected (for background highlight)
            if selected {
                // Calculate how much we've written roughly and pad
                let written_approx = 2 + 4 + 15 + 20 + 11 + 9 + 16 + 28 + c.ip_address.len();
                if written_approx < w {
                    write!(out, "{}", " ".repeat(w - written_approx))?;
                }
            }

            queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
            write!(out, "\r\n")?;

            // Expanded details
            if ui_state.expanded_ids.contains(&c.id) {
                queue!(out, SetForegroundColor(t.subtext))?;
                writeln(&mut out, &format!("       Image: {}", c.image))?;
                writeln(&mut out, &format!("       Status: {}", c.status))?;
                queue!(out, ResetColor)?;
            }
        }
    }

    // Status message
    if let Some(msg) = status_message {
        writeln(&mut out, "")?;
        queue!(out, SetForegroundColor(t.yellow))?;
        writeln(&mut out, &format!("  {}", msg))?;
        queue!(out, ResetColor)?;
    }

    // Help footer
    let (_width, height) = crossterm::terminal::size()?;
    let help_y = height.saturating_sub(1);
    render_help_footer(
        &mut out,
        &[
            ("q", "Quit"),
            ("\u{2191}\u{2193}", "Select"),
            ("Enter", "Expand"),
            ("Space", "Select"),
            ("L", "Logs"),
            ("M", "Multi-Log"),
            ("S", "Start"),
            ("T", "Stop"),
            ("R", "Restart"),
            ("Tab", "Next"),
        ],
        w,
        help_y,
    )?;

    out.flush()?;
    Ok(())
}
