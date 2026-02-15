use std::io::{self, Write, stdout};
use crossterm::{cursor::MoveTo, queue, style::{Color, SetForegroundColor, ResetColor, SetAttribute, Attribute, Print}};
use sysinfo::Pid;

use crate::layout::{Layout, SectionId};
use crate::model::{MonitorData, SortColumn, UIState};
use super::shared::{writeln, write_section_header, write_selectable, format_bytes_rate, progress_bar, format_number};
use super::RowKind;

pub fn render(
    data: &MonitorData,
    ui_state: &mut UIState,
    layout: &Layout,
) -> io::Result<Vec<(Pid, RowKind)>> {
    let mut out = stdout();
    let mut rows: Vec<(Pid, RowKind)> = Vec::new();
    let mut current_row: usize = 0;

    queue!(out, SetAttribute(Attribute::Bold))?;
    writeln(&mut out, "  System")?;
    queue!(out, SetAttribute(Attribute::Reset))?;
    writeln(&mut out, &format!("  {}  |  Cores: {}", data.time, data.core_count))?;
    writeln(&mut out, "")?;

    for section in &layout.sections {
        if section.id == SectionId::Summary {
            rows.push((Pid::from(0), RowKind::SectionHeader(section.id)));
            current_row += 1;
            render_summary(&mut out, data)?;
            continue;
        }

        let indicator = if section.collapsed { "▶" } else { "▼" };
        let header = format!("{} --- {} ---", indicator, section.title);

        write_section_header(&mut out, &header, current_row == ui_state.selected_index)?;
        rows.push((Pid::from(0), RowKind::SectionHeader(section.id)));
        current_row += 1;

        if section.collapsed {
            continue;
        }

        match section.id {
            SectionId::Summary => {}
            SectionId::Processes => {
                render_processes(&mut out, data, ui_state, &mut rows, &mut current_row)?;
            }
            SectionId::Network => render_network(&mut out, data)?,
            SectionId::FileDescriptors => render_fd_info(&mut out, data)?,
            SectionId::SocketOverview => render_socket_overview(&mut out, data)?,
        }
        writeln(&mut out, "")?;
    }

    ui_state.total_rows = current_row;

    let size = crossterm::terminal::size()?;
    let help = "q: Quit | Ctrl+C: Force Quit | Tab: Switch | ↑/↓: Navigate | →/←: Expand/Collapse | Sort: (c)pu (m)em (r)ead (w)rite (d)ownload (u)pload";
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
        writeln(&mut out, "(Expanded section data frozen)")?;
        queue!(out, ResetColor)?;
    }

    out.flush()?;
    Ok(rows)
}

fn render_summary(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
    writeln(out, "────────────────────────────────────────────────────────────────────────────────")?;

    let (l1, l5, l15) = data.load_avg;
    let cores = data.core_count;
    let mut load_line = String::from("LOAD   ");

    for (label, val) in [("1m:", l1), ("5m:", l5), ("15m:", l15)] {
        load_line.push_str(&format!("{} ", label));
        if val > cores {
            load_line.push_str(&format!("\x1b[31m{:.2}\x1b[0m  ", val));
        } else {
            load_line.push_str(&format!("{:.2}  ", val));
        }
    }

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
    writeln(out, &load_line)?;

    let m = &data.memory;
    let total_gb = m.total as f64 / 1_073_741_824.0;
    let used_gb = m.used as f64 / 1_073_741_824.0;
    let avail_gb = m.available as f64 / 1_073_741_824.0;
    let pct_used = if m.total > 0 { (m.used as f64 / m.total as f64) * 100.0 } else { 0.0 };

    let mem_bar = progress_bar(pct_used, 20);
    let mem_color = if pct_used > 85.0 {
        "\x1b[31m"
    } else if pct_used > 70.0 {
        "\x1b[33m"
    } else {
        ""
    };
    writeln(out, &format!(
        "MEM    {}{}\x1b[0m {:.1}/{:.1} GB ({:.0}%)  │ avail: {:.1} GB",
        mem_color, mem_bar, used_gb, total_gb, pct_used, avail_gb
    ))?;

    if m.swap_total > 0 {
        let swap_total_gb = m.swap_total as f64 / 1_073_741_824.0;
        let swap_used_gb = m.swap_used as f64 / 1_073_741_824.0;
        let swap_pct = (m.swap_used as f64 / m.swap_total as f64) * 100.0;
        let swap_bar = progress_bar(swap_pct, 20);

        let swap_color = if swap_pct > 50.0 {
            "\x1b[31m"
        } else if swap_pct > 20.0 {
            "\x1b[33m"
        } else {
            ""
        };
        writeln(out, &format!(
            "SWAP   {}{}\x1b[0m {:.1}/{:.1} GB ({:.0}%)",
            swap_color, swap_bar, swap_used_gb, swap_total_gb, swap_pct
        ))?;
    } else {
        writeln(out, "SWAP   none")?;
    }

    if data.disk_space.is_empty() {
        writeln(out, "DISK   all disks > 10% free")?;
    } else {
        for (idx, d) in data.disk_space.iter().enumerate() {
            let label = if idx == 0 { "DISK   " } else { "       " };
            let used_gb = d.total_gb - d.available_gb;
            let pct_used = 100.0 - d.percent_free;
            let disk_bar = progress_bar(pct_used, 20);

            let warning = if d.percent_free < 10.0 {
                " \x1b[31m⚠ LOW\x1b[0m"
            } else if d.percent_free < 20.0 {
                " \x1b[33m⚠ LOW\x1b[0m"
            } else {
                ""
            };

            writeln(out, &format!(
                "{}{:<10} {} {:.1}/{:.1} GB ({:.0}% free){}",
                label, d.mount_point, disk_bar, used_gb, d.total_gb, d.percent_free, warning
            ))?;
        }
    }

    writeln(out, &format!("IO     Busy: {:.1} MB/s", data.disk_busy_pct))?;

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

    writeln(out, &format!(
        "FD     {}{} / {} ({:.0}%)\x1b[0m  │ top: {}",
        fd_color, format_number(f.system_used), format_number(f.system_max), fd_pct, top_fd
    ))?;

    let s = &data.socket_overview;
    let time_wait_color = if s.time_wait > 100 { "\x1b[33m" } else { "" };
    let close_wait_color = if s.close_wait > 10 { "\x1b[31m" } else { "" };

    writeln(out, &format!(
        "SOCK   EST: {}  LISTEN: {}  {}TIME_WAIT: {}\x1b[0m  {}CLOSE_WAIT: {}\x1b[0m  FIN_WAIT: {}",
        s.established, s.listen, time_wait_color, s.time_wait,
        close_wait_color, s.close_wait, s.fin_wait
    ))?;

    let n = &data.network;
    if n.interfaces.is_empty() {
        writeln(out, "NET    no active traffic")?;
    } else {
        for (idx, iface) in n.interfaces.iter().enumerate() {
            let label = if idx == 0 { "NET    " } else { "       " };
            writeln(out, &format!(
                "{}{:<10} ↓ {}  ↑ {}",
                label, iface.name,
                format_bytes_rate(iface.rx_rate),
                format_bytes_rate(iface.tx_rate)
            ))?;
        }
    }

    writeln(out, "────────────────────────────────────────────────────────────────────────────────")?;

    Ok(())
}

fn render_processes(
    out: &mut impl Write,
    data: &MonitorData,
    ui_state: &UIState,
    rows: &mut Vec<(Pid, RowKind)>,
    current_row: &mut usize,
) -> io::Result<()> {
    write!(out, "  {:<18} ", "PID[CHILDREN]")?;

    let headers = [
        ("CPU %", 8, Some(SortColumn::Cpu)),
        ("MEM (MB)", 10, Some(SortColumn::Memory)),
        ("READ/s", 10, Some(SortColumn::Read)),
        ("WRITE/s", 10, Some(SortColumn::Write)),
        ("NET ↓", 10, Some(SortColumn::NetDown)),
        ("NET ↑", 10, Some(SortColumn::NetUp)),
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
        let read_fmt = format_bytes_rate(g.read_bytes);
        let write_fmt = format_bytes_rate(g.written_bytes);
        let net_rx_fmt = format_bytes_rate(g.net_rx_bytes);
        let net_tx_fmt = format_bytes_rate(g.net_tx_bytes);

        let line = format!("  {:<18} {:<8.1} {:<10.1} {:<10} {:<10} {:<10} {:<10} {}",
            pid_label, g.cpu, mem_mb, read_fmt, write_fmt, net_rx_fmt, net_tx_fmt, g.name);

        write_selectable(out, &line, *current_row == ui_state.selected_index)?;
        rows.push((g.pid, RowKind::ProcessParent));
        *current_row += 1;

        if ui_state.expanded_pids.contains(&g.pid) {
            for child in &g.children {
                let child_read = format_bytes_rate(child.read_bytes);
                let child_write = format_bytes_rate(child.written_bytes);
                let child_net_rx = format_bytes_rate(child.net_rx_bytes);
                let child_net_tx = format_bytes_rate(child.net_tx_bytes);

                let child_line = format!("    {:<16} {:<8.1} {:<10.1} {:<10} {:<10} {:<10} {:<10} {}",
                    child.pid, child.cpu, child.mem as f64 / 1_048_576.0,
                    child_read, child_write, child_net_rx, child_net_tx, child.name);

                write_selectable(out, &child_line, *current_row == ui_state.selected_index)?;
                rows.push((child.pid, RowKind::ProcessChild));
                *current_row += 1;
            }
        }
    }
    Ok(())
}

fn render_network(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
    let n = &data.network;
    writeln(out, &format!("  Connections:  ESTABLISHED: {}  TIME_WAIT: {}  CLOSE_WAIT: {}",
        n.established, n.time_wait, n.close_wait))?;

    if n.interfaces.is_empty() {
        writeln(out, "  Bandwidth: No active traffic")?;
    } else {
        for iface in &n.interfaces {
            writeln(out, &format!("  {:<10}  ↓ {}  ↑ {}",
                iface.name,
                format_bytes_rate(iface.rx_rate),
                format_bytes_rate(iface.tx_rate)))?;
        }
    }

    if !n.top_bandwidth_processes.is_empty() {
        writeln(out, "  Top processes by network connections:")?;
        for p in &n.top_bandwidth_processes {
            writeln(out, &format!("    {:<25} {} connections", p.name, p.bandwidth))?;
        }
    }
    Ok(())
}

fn render_fd_info(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
    let f = &data.fd_info;
    let pct = if f.system_max > 0 { (f.system_used as f64 / f.system_max as f64) * 100.0 } else { 0.0 };
    let color = if pct > 80.0 { Color::Red } else if pct > 50.0 { Color::Yellow } else { Color::Green };
    queue!(io::stdout(), SetForegroundColor(color))?;
    writeln(out, &format!("  System: {} / {} ({:.1}%)", f.system_used, f.system_max, pct))?;
    queue!(io::stdout(), ResetColor)?;
    if !f.top_processes.is_empty() {
        writeln(out, "  Top processes by open files:")?;
        for (name, count) in &f.top_processes {
            writeln(out, &format!("    {:<25} {}", name, count))?;
        }
    }
    Ok(())
}

fn render_socket_overview(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
    let s = &data.socket_overview;
    writeln(out, &format!(
        "  ESTABLISHED: {}  LISTEN: {}  TIME_WAIT: {}  CLOSE_WAIT: {}  FIN_WAIT: {}",
        s.established, s.listen, s.time_wait, s.close_wait, s.fin_wait
    ))?;

    if s.close_wait > 10 {
        queue!(io::stdout(), SetForegroundColor(Color::Red))?;
        writeln(out, &format!("  ⚠ High CLOSE_WAIT count ({}) — possible connection leak", s.close_wait))?;
        queue!(io::stdout(), ResetColor)?;
    }
    if s.time_wait > 100 {
        queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
        writeln(out, &format!("  ⚠ High TIME_WAIT count ({}) — high connection churn", s.time_wait))?;
        queue!(io::stdout(), ResetColor)?;
    }

    if !s.top_processes.is_empty() {
        writeln(out, "  Top processes by open connections:")?;
        for (name, count) in &s.top_processes {
            writeln(out, &format!("    {:<25} {}", name, count))?;
        }
    }
    Ok(())
}
