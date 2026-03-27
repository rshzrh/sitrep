use crossterm::{
    cursor, queue,
    style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use std::io::{self, Write};

use super::theme::theme;
use crate::model::AppView;

pub fn render_tab_bar(
    out: &mut impl Write,
    current_view: &AppView,
    docker_available: bool,
    container_count: usize,
    swarm_active: bool,
    node_count: u32,
    time: &str,
) -> io::Result<()> {
    let t = theme();

    write!(out, " ")?;

    // --- System tab ---
    let system_active = matches!(current_view, AppView::System);
    if system_active {
        queue!(
            io::stdout(),
            SetBackgroundColor(t.tab_active_bg),
            SetForegroundColor(t.tab_active_fg)
        )?;
    } else {
        queue!(io::stdout(), SetForegroundColor(t.tab_inactive_fg))?;
    }
    write!(out, " System ")?;
    queue!(io::stdout(), ResetColor)?;

    // --- Containers tab ---
    if docker_available {
        write!(out, "  ")?;
        let containers_active = matches!(
            current_view,
            AppView::Containers | AppView::ContainerLogs(_)
        );
        if containers_active {
            queue!(
                io::stdout(),
                SetBackgroundColor(t.tab_active_bg),
                SetForegroundColor(t.tab_active_fg)
            )?;
        } else {
            queue!(io::stdout(), SetForegroundColor(t.tab_inactive_fg))?;
        }
        write!(out, " Containers({}) ", container_count)?;
        queue!(io::stdout(), ResetColor)?;
    }

    // --- Swarm tab ---
    if swarm_active {
        write!(out, "  ")?;
        let swarm_tab_active = matches!(
            current_view,
            AppView::Swarm | AppView::SwarmServiceTasks(_, _) | AppView::SwarmServiceLogs(_, _)
        );
        if swarm_tab_active {
            queue!(
                io::stdout(),
                SetBackgroundColor(t.tab_active_bg),
                SetForegroundColor(t.tab_active_fg)
            )?;
        } else {
            queue!(io::stdout(), SetForegroundColor(t.tab_inactive_fg))?;
        }
        write!(out, " Swarm({}) ", node_count)?;
        queue!(io::stdout(), ResetColor)?;
    }

    // --- Right-aligned: sitrep - HH:MM:SS ---
    let size = terminal::size()?;
    let time_str = format!("sitrep - {} ", time);
    let col = (size.0 as usize).saturating_sub(time_str.len());
    queue!(
        io::stdout(),
        cursor::MoveTo(col as u16, 0),
        SetForegroundColor(t.subtext),
        Print(&time_str),
        ResetColor
    )?;

    write!(out, "\r\n")?;

    // --- Separator line: thin horizontal rule in surface color ---
    let sep: String = "\u{2500}".repeat(size.0 as usize);
    queue!(io::stdout(), SetForegroundColor(t.separator))?;
    write!(out, "{}\r\n", sep)?;
    queue!(io::stdout(), ResetColor)?;

    Ok(())
}
