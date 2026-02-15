use std::io::{self, Write};
use crossterm::{
    cursor, queue,
    style::{Color, SetForegroundColor, SetBackgroundColor, ResetColor},
    terminal,
};

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
