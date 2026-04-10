use crossterm::{
    cursor, queue,
    style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use std::io::{self, Write};

use super::theme::theme;
use crate::model::{AppView, RemoteTab};

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

    // Detect whether we're in remote drill-in mode. When yes, the tab
    // bar shows REMOTE tab labels (derived from the RemoteTab) and a
    // "◂ Fleet" prefix that reminds the user Esc goes back to overview.
    let remote_tab: Option<&RemoteTab> = match current_view {
        AppView::Remote { tab, .. } => Some(tab),
        _ => None,
    };

    write!(out, " ")?;

    if let Some(rtab) = remote_tab {
        // "◂ Fleet" back-hint.
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        write!(out, "◂ Fleet  ")?;
        queue!(io::stdout(), ResetColor)?;

        let is_system = matches!(rtab, RemoteTab::System);
        draw_tab(out, " System ", is_system, &t)?;

        write!(out, "  ")?;
        let is_containers = matches!(
            rtab,
            RemoteTab::Containers | RemoteTab::ContainerLogs(_)
        );
        draw_tab(
            out,
            &format!(" Containers({}) ", container_count),
            is_containers,
            &t,
        )?;

        if swarm_active {
            write!(out, "  ")?;
            let is_swarm = matches!(
                rtab,
                RemoteTab::Swarm
                    | RemoteTab::SwarmServiceTasks(_, _)
                    | RemoteTab::SwarmServiceLogs(_, _)
            );
            draw_tab(out, &format!(" Swarm({}) ", node_count), is_swarm, &t)?;
        }
    } else {
        // Local mode — System / Containers / Swarm.
        let system_active = matches!(current_view, AppView::System);
        draw_tab(out, " System ", system_active, &t)?;

        if docker_available {
            write!(out, "  ")?;
            let containers_active = matches!(
                current_view,
                AppView::Containers | AppView::ContainerLogs(_)
            );
            draw_tab(
                out,
                &format!(" Containers({}) ", container_count),
                containers_active,
                &t,
            )?;
        }

        if swarm_active {
            write!(out, "  ")?;
            let swarm_tab_active = matches!(
                current_view,
                AppView::Swarm
                    | AppView::SwarmServiceTasks(_, _)
                    | AppView::SwarmServiceLogs(_, _)
            );
            draw_tab(out, &format!(" Swarm({}) ", node_count), swarm_tab_active, &t)?;
        }
    }

    // --- Right-aligned: flotop - HH:MM:SS ---
    let size = terminal::size()?;
    let time_str = format!("flotop - {} ", time);
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

fn draw_tab(
    out: &mut impl Write,
    label: &str,
    active: bool,
    t: &super::theme::Theme,
) -> io::Result<()> {
    if active {
        queue!(
            io::stdout(),
            SetBackgroundColor(t.tab_active_bg),
            SetForegroundColor(t.tab_active_fg)
        )?;
    } else {
        queue!(io::stdout(), SetForegroundColor(t.tab_inactive_fg))?;
    }
    write!(out, "{}", label)?;
    queue!(io::stdout(), ResetColor)?;
    Ok(())
}
