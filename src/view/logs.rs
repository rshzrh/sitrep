use crossterm::{
    cursor::MoveTo,
    execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{self, stdout, Write};

use super::shared::safe_truncate;
use crate::model::{LogViewState, MultiLogViewState, ServiceLogState};
use crate::view::shared::writeln;

pub fn render_logs(log_state: &LogViewState) -> io::Result<()> {
    let mut out = stdout();
    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;

    let size = terminal::size()?;
    let width = size.0 as usize;
    let height = size.1 as usize;

    // Header
    let follow_indicator = if log_state.auto_follow {
        "FOLLOWING"
    } else {
        "PAUSED"
    };
    let search_indicator = if !log_state.search_query.is_empty() {
        format!(" | SEARCH: \"{}\"", log_state.search_query)
    } else {
        String::new()
    };
    let header = format!(
        "  Containers › Logs: {} ({}) - {}{}",
        log_state.container_name, log_state.container_id, follow_indicator, search_indicator
    );

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
        let sep: String = "─".repeat(width);
        queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        write!(out, "{}\r\n", sep)?;
        queue!(io::stdout(), ResetColor)?;
    }

    // Log content area
    let log_area_height = height.saturating_sub(4);
    let has_search = !log_state.search_query.is_empty();
    let query_lower = log_state.search_query.to_lowercase();

    let display_lines: Vec<&String> = if has_search {
        log_state
            .lines
            .iter()
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
            let prefix = format!("{}: ", log_state.container_name);
            let full_line = format!("{}{}", prefix, line);
            let display_line = safe_truncate(&full_line, width);
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
        crossterm::style::Print(format!("{:<width$}", help, width = width)),
        ResetColor
    )?;

    out.flush()?;
    Ok(())
}

pub fn render_multi_container_logs(
    log_state: &MultiLogViewState,
    active_container_names: &[String],
) -> io::Result<()> {
    let mut out = stdout();
    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;

    let size = terminal::size()?;
    let width = size.0 as usize;
    let height = size.1 as usize;

    let container_count = active_container_names.len();
    let follow_indicator = if log_state.auto_follow {
        "FOLLOWING"
    } else {
        "PAUSED"
    };
    let search_indicator = if !log_state.search_query.is_empty() {
        format!(" | SEARCH: \"{}\"", log_state.search_query)
    } else {
        String::new()
    };
    let header = format!(
        "  Containers › Multi-Log: {} containers - {}{}",
        container_count, follow_indicator, search_indicator
    );

    queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
    if !log_state.auto_follow {
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
    }
    write!(out, "{}\r\n", header)?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

    // On-screen indicator for active multi-container streams
    if !active_container_names.is_empty() {
        let names = active_container_names.join(", ");
        queue!(io::stdout(), SetForegroundColor(Color::Cyan))?;
        writeln!(&mut out, "  Streaming: {}", names)?;
        queue!(io::stdout(), ResetColor)?;
    }

    if log_state.search_mode {
        queue!(io::stdout(), SetForegroundColor(Color::Cyan))?;
        write!(out, "  Search: {}_\r\n", log_state.search_query)?;
        queue!(io::stdout(), ResetColor)?;
    } else {
        let sep: String = "─".repeat(width);
        queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        write!(out, "{}\r\n", sep)?;
        queue!(io::stdout(), ResetColor)?;
    }

    let log_area_height = height.saturating_sub(4);
    let has_search = !log_state.search_query.is_empty();
    let query_lower = log_state.search_query.to_lowercase();

    let display_lines: Vec<_> = if has_search {
        log_state
            .lines
            .iter()
            .filter(|entry| entry.line.to_lowercase().contains(&query_lower))
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
        if let Some(entry) = display_lines.get(i) {
            let full_line = format!("{}: {}", entry.container_name, entry.line);
            let display_line = safe_truncate(&full_line, width);
            if has_search && entry.line.to_lowercase().contains(&query_lower) {
                queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
                write!(out, "{}\r\n", display_line)?;
                queue!(io::stdout(), ResetColor)?;
            } else {
                write!(out, "{}\r\n", display_line)?;
            }
            lines_printed += 1;
        }
    }

    for _ in lines_printed..log_area_height {
        write!(out, "\r\n")?;
    }

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
        crossterm::style::Print(format!("{:<width$}", help, width = width)),
        ResetColor
    )?;

    out.flush()?;
    Ok(())
}

pub fn render_service_logs(log_state: &ServiceLogState) -> io::Result<()> {
    let mut out = stdout();
    execute!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;

    let size = terminal::size()?;
    let width = size.0 as usize;
    let height = size.1 as usize;

    // Header
    let follow_indicator = if log_state.auto_follow {
        "FOLLOWING"
    } else {
        "PAUSED"
    };
    let filter_indicator = if log_state.filter_errors {
        " | ERRORS ONLY"
    } else {
        ""
    };
    let search_indicator = if !log_state.search_query.is_empty() {
        format!(" | SEARCH: \"{}\"", log_state.search_query)
    } else {
        String::new()
    };
    let header = format!(
        "  Swarm › Service Logs: {} ({}) - {}{}{}",
        log_state.service_name,
        log_state.service_id,
        follow_indicator,
        filter_indicator,
        search_indicator
    );

    queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
    if !log_state.auto_follow {
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
    }
    write!(out, "{}\r\n", header)?;
    queue!(io::stdout(), SetAttribute(Attribute::Reset), ResetColor)?;

    if log_state.search_mode {
        queue!(io::stdout(), SetForegroundColor(Color::Cyan))?;
        write!(out, "  Search: {}_\r\n", log_state.search_query)?;
        queue!(io::stdout(), ResetColor)?;
    } else {
        let sep: String = "─".repeat(width);
        queue!(io::stdout(), SetForegroundColor(Color::DarkGrey))?;
        write!(out, "{}\r\n", sep)?;
        queue!(io::stdout(), ResetColor)?;
    }

    let has_search = !log_state.search_query.is_empty();
    let query_lower = log_state.search_query.to_lowercase();

    let display_lines: Vec<&String> = log_state
        .lines
        .iter()
        .filter(|l| {
            if log_state.filter_errors {
                let lower = l.to_lowercase();
                if !(lower.contains("error")
                    || lower.contains("err")
                    || lower.contains("panic")
                    || lower.contains("fatal")
                    || lower.contains("exception")
                    || lower.contains("fail"))
                {
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
            let is_error = {
                let lower = line.to_lowercase();
                lower.contains("error")
                    || lower.contains("panic")
                    || lower.contains("fatal")
                    || lower.contains("exception")
            };
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
        crossterm::style::Print(format!("{:<width$}", help, width = width)),
        ResetColor
    )?;

    out.flush()?;
    Ok(())
}
