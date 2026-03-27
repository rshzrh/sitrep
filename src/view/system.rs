use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use std::io::{self, stdout, Write};
use sysinfo::Pid;

use super::shared::{
    format_bytes_rate, format_mem_human, load_avg_color, render_bar, render_help_footer,
};
use super::theme::theme;
use super::RowKind;
use crate::layout::Layout;
use crate::model::{MonitorData, SortColumn, UIState};

pub fn render(
    data: &MonitorData,
    ui_state: &mut UIState,
    _layout: &Layout,
) -> io::Result<Vec<(Pid, RowKind)>> {
    let mut out = stdout();
    let mut rows: Vec<(Pid, RowKind)> = Vec::new();
    let mut current_row: usize = 0;
    let t = theme();
    let size = crossterm::terminal::size()?;
    let term_width = size.0 as usize;

    // ── Top panel: CPU / Mem / Swap bars with right-side stats ──

    let bar_width: usize = 40;

    // CPU bar
    let cpu_total: f64 = data
        .historical_top
        .iter()
        .map(|p| p.cpu)
        .sum::<f64>();
    let cpu_pct = (cpu_total / (data.core_count * 100.0) * 100.0).min(100.0);
    render_bar(&mut out, "CPU", cpu_pct, "", bar_width)?;

    // Right side: Tasks + Load average (on the same line as CPU bar)
    let task_count = data.historical_top.len();
    let right_col = bar_width + 3 + 5 + 8; // label(5) + bar + bracket(2) + percent(7) + spacing
    let right_start = right_col + 2;
    if right_start < term_width {
        queue!(out, SetForegroundColor(t.subtext))?;
        write!(out, "    Tasks: ", )?;
        queue!(out, SetForegroundColor(t.text))?;
        write!(out, "{:<6}", task_count)?;

        queue!(out, SetForegroundColor(t.subtext))?;
        write!(out, "Load average: ")?;

        let (l1, l5, l15) = data.load_avg;
        let cores = data.core_count;
        for (i, val) in [l1, l5, l15].iter().enumerate() {
            let color = load_avg_color(*val, cores);
            queue!(out, SetForegroundColor(color))?;
            write!(out, "{:.2}", val)?;
            queue!(out, ResetColor)?;
            if i < 2 {
                write!(out, " ")?;
            }
        }
    }
    queue!(out, ResetColor)?;
    write!(out, "\r\n")?;

    // Mem bar
    let m = &data.memory;
    let mem_pct = if m.total > 0 {
        (m.used as f64 / m.total as f64) * 100.0
    } else {
        0.0
    };
    let mem_used_gb = m.used as f64 / 1_073_741_824.0;
    let mem_total_gb = m.total as f64 / 1_073_741_824.0;
    let mem_detail = format!("{:.1}G/{:.1}G", mem_used_gb, mem_total_gb);
    render_bar(&mut out, "Mem", mem_pct, &mem_detail, bar_width)?;

    // Right side: Uptime
    if !data.time.is_empty() {
        queue!(out, SetForegroundColor(t.subtext))?;
        write!(out, "              Uptime: ")?;
        queue!(out, SetForegroundColor(t.text))?;
        write!(out, "{}", data.time)?;
    }
    queue!(out, ResetColor)?;
    write!(out, "\r\n")?;

    // Swap bar
    if m.swap_total > 0 {
        let swap_pct = (m.swap_used as f64 / m.swap_total as f64) * 100.0;
        let swap_used_gb = m.swap_used as f64 / 1_073_741_824.0;
        let swap_total_gb = m.swap_total as f64 / 1_073_741_824.0;
        let swap_detail = format!("{:.1}G/{:.1}G", swap_used_gb, swap_total_gb);
        render_bar(&mut out, "Swp", swap_pct, &swap_detail, bar_width)?;
    } else {
        queue!(out, SetForegroundColor(t.subtext))?;
        write!(out, " Swp [no swap]")?;
    }
    queue!(out, ResetColor)?;
    write!(out, "\r\n")?;

    // ── Disk usage ──
    if !data.disk_space.is_empty() {
        for disk in &data.disk_space {
            let used_gb = disk.total_gb - disk.available_gb;
            let used_pct = if disk.total_gb > 0.0 {
                (used_gb / disk.total_gb) * 100.0
            } else {
                0.0
            };
            let label = if disk.mount_point.len() <= 10 {
                format!(" {} ", disk.mount_point)
            } else {
                let short: String = disk.mount_point.chars().take(8).collect();
                format!(" {}.. ", short)
            };
            let detail = format!("{:.1}G/{:.1}G", used_gb, disk.total_gb);

            // Color the bar based on usage
            let bar_color = if used_pct > 90.0 {
                t.red
            } else if used_pct > 75.0 {
                t.peach
            } else {
                t.bar_filled
            };

            // Render disk bar manually with custom color
            let filled = ((used_pct / 100.0) * bar_width as f64).round() as usize;
            let empty = bar_width.saturating_sub(filled);

            queue!(out, SetForegroundColor(t.subtext))?;
            write!(out, "{:<4}", label.chars().take(4).collect::<String>())?;
            queue!(out, SetForegroundColor(t.subtext))?;
            write!(out, "[")?;
            queue!(out, SetForegroundColor(bar_color))?;
            write!(out, "{}", "|".repeat(filled))?;
            queue!(out, SetBackgroundColor(t.bar_empty), SetForegroundColor(t.bar_empty))?;
            write!(out, "{}", " ".repeat(empty))?;
            queue!(out, ResetColor, SetForegroundColor(t.subtext))?;
            write!(out, "]")?;
            queue!(out, SetForegroundColor(t.text))?;
            write!(out, " {:>5.1}%  ", used_pct)?;
            queue!(out, SetForegroundColor(t.subtext))?;
            write!(out, "{}", detail)?;

            // Warning indicator
            if used_pct > 90.0 {
                queue!(out, SetForegroundColor(t.red))?;
                write!(out, "  LOW")?;
            } else if used_pct > 80.0 {
                queue!(out, SetForegroundColor(t.peach))?;
                write!(out, "  LOW")?;
            }

            // Disk I/O on the first disk line
            if std::ptr::eq(disk, &data.disk_space[0]) && data.disk_busy_pct > 0.0 {
                queue!(out, SetForegroundColor(t.subtext))?;
                write!(out, "    I/O: ")?;
                let io_color = if data.disk_busy_pct > 80.0 {
                    t.red
                } else if data.disk_busy_pct > 50.0 {
                    t.peach
                } else {
                    t.text
                };
                queue!(out, SetForegroundColor(io_color))?;
                write!(out, "{:.1}%", data.disk_busy_pct)?;
            }

            queue!(out, ResetColor)?;
            write!(out, "\r\n")?;
        }
    }

    // ── Network interfaces ──
    if !data.network.interfaces.is_empty() {
        for iface in &data.network.interfaces {
            let rx_str = format_bytes_rate(iface.rx_rate);
            let tx_str = format_bytes_rate(iface.tx_rate);

            queue!(out, SetForegroundColor(t.subtext))?;
            write!(out, " Net ")?;
            queue!(out, SetForegroundColor(t.text))?;
            write!(out, "{:<10}", iface.name)?;
            queue!(out, SetForegroundColor(t.teal))?;
            write!(out, " \u{2193}{:<10}", rx_str)?;
            queue!(out, SetForegroundColor(t.peach))?;
            write!(out, " \u{2191}{:<10}", tx_str)?;
            queue!(out, ResetColor)?;
            write!(out, "\r\n")?;
        }
    }

    // ── Socket summary (compact, single line) ──
    let sock = &data.socket_overview;
    if sock.established > 0 || sock.listen > 0 || sock.time_wait > 0 || sock.close_wait > 0 {
        queue!(out, SetForegroundColor(t.subtext))?;
        write!(out, " Sock ")?;
        queue!(out, SetForegroundColor(t.text))?;
        write!(out, "EST:{} ", sock.established)?;
        write!(out, "LISTEN:{} ", sock.listen)?;

        if sock.time_wait > 100 {
            queue!(out, SetForegroundColor(t.yellow))?;
        }
        write!(out, "TW:{} ", sock.time_wait)?;
        queue!(out, SetForegroundColor(t.text))?;

        if sock.close_wait > 10 {
            queue!(out, SetForegroundColor(t.red))?;
        }
        write!(out, "CW:{}", sock.close_wait)?;

        queue!(out, ResetColor)?;
        write!(out, "\r\n")?;
    }

    // ── Separator line ──
    queue!(out, SetForegroundColor(t.separator))?;
    let sep: String = "\u{2500}".repeat(term_width);
    write!(out, "{}\r\n", sep)?;
    queue!(out, ResetColor)?;

    // ── Process table header ──
    let headers: &[(&str, usize, Option<SortColumn>)] = &[
        ("PID", 7, None),
        ("USER", 10, None),
        ("CPU%", 6, Some(SortColumn::Cpu)),
        ("MEM", 6, Some(SortColumn::Memory)),
        ("NET I/O", 10, Some(SortColumn::NetDown)),
        ("TIME+", 10, None),
        ("Command", 0, None),
    ];

    queue!(
        out,
        SetForegroundColor(t.header_fg),
        SetAttribute(Attribute::Bold)
    )?;

    write!(out, "  ")?;
    for (text, width, col) in headers {
        let is_sorted = col.map_or(false, |c| c == ui_state.sort_column);
        if is_sorted {
            queue!(
                out,
                SetBackgroundColor(t.surface),
                SetForegroundColor(t.text)
            )?;
        }
        if *width > 0 {
            write!(out, "{:<width$} ", text, width = width)?;
        } else {
            write!(out, "{}", text)?;
        }
        if is_sorted {
            queue!(
                out,
                ResetColor,
                SetForegroundColor(t.header_fg)
            )?;
        }
    }
    queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
    write!(out, "\r\n")?;

    // ── Process rows ──
    for g in &data.historical_top {
        let mem_str = format_mem_human(g.mem);
        let net_total = g.net_rx_bytes.saturating_add(g.net_tx_bytes);
        let net_str = format_bytes_rate(net_total);
        let time_str = "-".to_string(); // CPU time not directly available in model

        // Determine CPU color
        let cpu_color = if g.cpu > 80.0 {
            t.red
        } else if g.cpu > 50.0 {
            t.peach
        } else {
            t.text
        };

        // Build the line: we need to color CPU specially, but write_selectable takes a string.
        // For the selected row highlight we use write_selectable for the background,
        // but we also want per-column colors. We'll do manual rendering.
        let is_selected = current_row == ui_state.selected_index;

        if is_selected {
            queue!(out, SetBackgroundColor(t.selected_bg))?;
        }

        // PID
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.text }))?;
        write!(out, "  {:<7}", g.pid)?;

        // USER
        let user_display = if g.user.len() > 9 {
            &g.user[..9]
        } else {
            &g.user
        };
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.subtext }))?;
        write!(out, "{:<10} ", user_display)?;

        // CPU%
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { cpu_color }))?;
        write!(out, "{:<6.1}", g.cpu)?;

        // MEM
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.text }))?;
        write!(out, "{:<6} ", mem_str)?;

        // NET I/O
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.subtext }))?;
        write!(out, "{:<10} ", net_str)?;

        // TIME+
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.subtext }))?;
        write!(out, "{:<10} ", time_str)?;

        // Command (fill remaining width)
        let used_cols = 2 + 7 + 10 + 1 + 6 + 6 + 1 + 10 + 1 + 10 + 1;
        let remaining = term_width.saturating_sub(used_cols);
        let name = if g.name.len() > remaining {
            &g.name[..remaining]
        } else {
            &g.name
        };
        queue!(out, SetForegroundColor(if is_selected { t.selected_fg } else { t.text }))?;
        write!(out, "{}", name)?;

        queue!(out, ResetColor)?;
        write!(out, "\r\n")?;

        rows.push((g.pid, RowKind::ProcessParent));
        current_row += 1;

        // Expanded children
        if ui_state.expanded_pids.contains(&g.pid) {
            for child in &g.children {
                let child_is_selected = current_row == ui_state.selected_index;
                let child_mem = format_mem_human(child.mem);
                let child_net = format_bytes_rate(
                    child.net_rx_bytes.saturating_add(child.net_tx_bytes),
                );
                let child_cpu_color = if child.cpu as f64 > 80.0 {
                    t.red
                } else if child.cpu as f64 > 50.0 {
                    t.peach
                } else {
                    t.text
                };

                if child_is_selected {
                    queue!(out, SetBackgroundColor(t.selected_bg))?;
                }

                // Indented PID
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.text }))?;
                write!(out, "      {:<5}", child.pid)?;

                // USER
                let child_user = if child.user.len() > 9 {
                    &child.user[..9]
                } else {
                    &child.user
                };
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.subtext }))?;
                write!(out, "{:<10} ", child_user)?;

                // CPU%
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { child_cpu_color }))?;
                write!(out, "{:<6.1}", child.cpu)?;

                // MEM
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.text }))?;
                write!(out, "{:<6} ", child_mem)?;

                // NET I/O
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.subtext }))?;
                write!(out, "{:<10} ", child_net)?;

                // TIME+
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.subtext }))?;
                write!(out, "{:<10} ", "-")?;

                // Command
                let child_remaining = term_width.saturating_sub(used_cols);
                let child_name = if child.name.len() > child_remaining {
                    &child.name[..child_remaining]
                } else {
                    &child.name
                };
                queue!(out, SetForegroundColor(if child_is_selected { t.selected_fg } else { t.text }))?;
                write!(out, "{}", child_name)?;

                queue!(out, ResetColor)?;
                write!(out, "\r\n")?;

                rows.push((child.pid, RowKind::ProcessChild));
                current_row += 1;
            }
        }
    }

    ui_state.total_rows = current_row;

    // ── Help footer (last row) ──
    let help_y = size.1.saturating_sub(1);
    render_help_footer(
        &mut out,
        &[
            ("q", "Quit"),
            ("\u{2191}\u{2193}", "Select"),
            ("Enter", "Expand"),
            ("Tab", "Next"),
            ("s", "Sort"),
            ("/", "Search"),
        ],
        term_width,
        help_y,
    )?;

    if ui_state.has_expansions() {
        // Position just above help footer
        let note_y = help_y.saturating_sub(1);
        queue!(out, MoveTo(1, note_y), SetForegroundColor(t.yellow))?;
        write!(out, "(Expanded section data frozen)")?;
        queue!(out, ResetColor)?;
    }

    out.flush()?;
    Ok(rows)
}
