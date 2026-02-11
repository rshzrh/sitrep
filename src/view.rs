use crate::model::{MonitorData, UIState};
use crate::layout::{Layout, SectionId};
use std::io::{self, Write, stdout};
use crossterm::{
    cursor, execute, queue,
    style::{Color, SetForegroundColor, SetBackgroundColor, ResetColor, Attribute, SetAttribute},
    terminal::{Clear, ClearType},
};
use sysinfo::Pid;

/// What kind of row this is in the row mapping
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    SectionHeader(SectionId),
    CpuParent,
    CpuChild,
    DiskParent,
    DiskChild,
}

pub struct Presenter;

impl Presenter {
    pub fn render(
        data: &MonitorData,
        ui_state: &mut UIState,
        layout: &Layout,
    ) -> io::Result<Vec<(Pid, RowKind)>> {
        let mut out = stdout();
        execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

        let mut rows: Vec<(Pid, RowKind)> = Vec::new();
        let mut current_row: usize = 0;

        // Time header
        Self::writeln(&mut out, &format!("  adtop — {}  |  Cores: {}", data.time, data.core_count))?;
        Self::writeln(&mut out, "")?;

        for section in &layout.sections {
            let indicator = if section.collapsed { "▶" } else { "▼" };
            let header = format!("{} --- {} ---", indicator, section.title);

            Self::write_section_header(&mut out, &header, current_row == ui_state.selected_index)?;
            rows.push((Pid::from(0), RowKind::SectionHeader(section.id)));
            current_row += 1;

            if section.collapsed {
                continue;
            }

            match section.id {
                SectionId::DiskSpace => Self::render_disk_space(&mut out, data)?,
                SectionId::Memory => Self::render_memory(&mut out, data)?,
                SectionId::LoadAverage => Self::render_load_average(&mut out, data)?,
                SectionId::CpuProcesses => {
                    Self::render_cpu_processes(&mut out, data, ui_state, &mut rows, &mut current_row)?;
                }
                SectionId::DiskIo => {
                    Self::render_disk_io(&mut out, data, ui_state, &mut rows, &mut current_row)?;
                }
                SectionId::Network => Self::render_network(&mut out, data)?,
                SectionId::FileDescriptors => Self::render_fd_info(&mut out, data)?,
                SectionId::ContextSwitches => Self::render_context_switches(&mut out, data)?,
                SectionId::SocketOverview => Self::render_socket_overview(&mut out, data)?,
            }
            Self::writeln(&mut out, "")?;
        }

        ui_state.total_rows = current_row;

        Self::writeln(&mut out, "↑↓ Navigate  →/← Expand/Collapse Section  Q Quit")?;
        if ui_state.has_expansions() {
            queue!(out, SetForegroundColor(Color::Yellow))?;
            Self::writeln(&mut out, "(Expanded section data frozen)")?;
            queue!(out, ResetColor)?;
        }

        out.flush()?;
        Ok(rows)
    }

    // --- Section renderers ---

    fn render_disk_space(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        if data.disk_space.is_empty() {
            Self::writeln(out, "  ✓ All disks have > 10% free space")?;
        } else {
            for d in &data.disk_space {
                let color = if d.is_warning { Color::Red } else { Color::Yellow };
                queue!(io::stdout(), SetForegroundColor(color))?;
                Self::writeln(out, &format!(
                    "  ⚠ {}  {:.1} GB free / {:.1} GB total ({:.1}% free)",
                    d.mount_point, d.available_gb, d.total_gb, d.percent_free
                ))?;
                queue!(io::stdout(), ResetColor)?;
            }
        }
        Self::writeln(out, &format!("  Disk busy: {:.1} MB/s throughput", data.disk_busy_pct))?;
        Ok(())
    }

    fn render_memory(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let m = &data.memory;
        let total_gb = m.total as f64 / 1_073_741_824.0;
        let used_gb = m.used as f64 / 1_073_741_824.0;
        let avail_gb = m.available as f64 / 1_073_741_824.0;
        let pct_used = if m.total > 0 { (m.used as f64 / m.total as f64) * 100.0 } else { 0.0 };

        let bar = Self::progress_bar(pct_used, 30);
        Self::writeln(out, &format!("  RAM:  {} {:.1}/{:.1} GB ({:.0}% used)  Free: {:.1} GB",
            bar, used_gb, total_gb, pct_used, avail_gb))?;

        if m.swap_total > 0 {
            let swap_total_gb = m.swap_total as f64 / 1_073_741_824.0;
            let swap_used_gb = m.swap_used as f64 / 1_073_741_824.0;
            let swap_pct = (m.swap_used as f64 / m.swap_total as f64) * 100.0;
            let swap_bar = Self::progress_bar(swap_pct, 30);

            if swap_pct > 80.0 {
                queue!(io::stdout(), SetForegroundColor(Color::Red))?;
            }
            Self::writeln(out, &format!("  Swap: {} {:.1}/{:.1} GB ({:.0}% used)",
                swap_bar, swap_used_gb, swap_total_gb, swap_pct))?;
            if swap_pct > 80.0 {
                queue!(io::stdout(), ResetColor)?;
            }
        } else {
            Self::writeln(out, "  Swap: None")?;
        }
        Ok(())
    }

    fn render_load_average(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let (l1, l5, l15) = data.load_avg;
        let cores = data.core_count;
        // Build each value with color if it exceeds core count
        let mut line = String::from("  ");
        for (label, val) in [("1m:", l1), ("5m:", l5), ("15m:", l15)] {
            line.push_str(&format!("{} ", label));
            if val > cores {
                line.push_str(&format!("\x1b[31m{:.2}\x1b[0m  ", val)); // red
            } else {
                line.push_str(&format!("{:.2}  ", val));
            }
        }
        line.push_str(&format!("(cores: {})", cores as u32));
        Self::writeln(out, &line)?;
        Ok(())
    }

    fn render_cpu_processes(
        out: &mut impl Write, data: &MonitorData, ui_state: &UIState,
        rows: &mut Vec<(Pid, RowKind)>, current_row: &mut usize,
    ) -> io::Result<()> {
        Self::writeln(out, &format!("  {:<18} {:<10} {:<12} {}",
            "PID[CHILDREN]", "CPU %", "MEM (MB)", "Name"))?;
        for g in &data.historical_top {
            let pid_label = format!("{}[{}]", g.pid, g.child_count);
            let mem_mb = g.mem as f64 / 1_048_576.0;
            let line = format!("  {:<18} {:<10.2} {:<12.2} {}", pid_label, g.cpu, mem_mb, g.name);
            Self::write_selectable(out, &line, *current_row == ui_state.selected_index)?;
            rows.push((g.pid, RowKind::CpuParent));
            *current_row += 1;

            if ui_state.cpu_expanded_pids.contains(&g.pid) {
                for child in &g.children {
                    let child_line = format!("    {:<16} {:<10.2} {:<12.2} {}",
                        child.pid, child.cpu, child.mem as f64 / 1_048_576.0, child.name);
                    Self::write_selectable(out, &child_line, *current_row == ui_state.selected_index)?;
                    rows.push((child.pid, RowKind::CpuChild));
                    *current_row += 1;
                }
            }
        }
        Ok(())
    }

    fn render_disk_io(
        out: &mut impl Write, data: &MonitorData, ui_state: &UIState,
        rows: &mut Vec<(Pid, RowKind)>, current_row: &mut usize,
    ) -> io::Result<()> {
        Self::writeln(out, &format!("  {:<18} {:<12} {:<12} {}",
            "PID[CHILDREN]", "READ/s", "WRITE/s", "Name"))?;
        for g in &data.historical_disk_top {
            let pid_label = format!("{}[{}]", g.pid, g.child_count);
            let read_fmt = Self::format_bytes_rate(g.read_bytes);
            let write_fmt = Self::format_bytes_rate(g.written_bytes);
            let line = format!("  {:<18} {:<12} {:<12} {}", pid_label, read_fmt, write_fmt, g.name);
            Self::write_selectable(out, &line, *current_row == ui_state.selected_index)?;
            rows.push((g.pid, RowKind::DiskParent));
            *current_row += 1;

            if ui_state.disk_expanded_pids.contains(&g.pid) {
                for child in &g.children {
                    let child_line = format!("    {:<16} {:<12} {:<12} {}",
                        child.pid, "N/A", "N/A", child.name);
                    Self::write_selectable(out, &child_line, *current_row == ui_state.selected_index)?;
                    rows.push((child.pid, RowKind::DiskChild));
                    *current_row += 1;
                }
            }
        }
        Ok(())
    }

    fn render_network(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let n = &data.network;
        Self::writeln(out, &format!("  Connections:  ESTABLISHED: {}  TIME_WAIT: {}  CLOSE_WAIT: {}",
            n.established, n.time_wait, n.close_wait))?;

        // Interface bandwidth
        if n.interfaces.is_empty() {
            Self::writeln(out, "  Bandwidth: No active traffic")?;
        } else {
            for iface in &n.interfaces {
                Self::writeln(out, &format!("  {:<10}  ↓ {}  ↑ {}",
                    iface.name,
                    Self::format_bytes_rate(iface.rx_rate),
                    Self::format_bytes_rate(iface.tx_rate)))?;
            }
        }

        // Top bandwidth processes
        if !n.top_bandwidth_processes.is_empty() {
            Self::writeln(out, "  Top processes by network connections:")?;
            for p in &n.top_bandwidth_processes {
                Self::writeln(out, &format!("    {:<25} {} connections", p.name, p.bandwidth))?;
            }
        }
        Ok(())
    }

    fn render_fd_info(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let f = &data.fd_info;
        let pct = if f.system_max > 0 { (f.system_used as f64 / f.system_max as f64) * 100.0 } else { 0.0 };
        let color = if pct > 80.0 { Color::Red } else if pct > 50.0 { Color::Yellow } else { Color::Green };
        queue!(io::stdout(), SetForegroundColor(color))?;
        Self::writeln(out, &format!("  System: {} / {} ({:.1}%)", f.system_used, f.system_max, pct))?;
        queue!(io::stdout(), ResetColor)?;
        if !f.top_processes.is_empty() {
            Self::writeln(out, "  Top processes by open files:")?;
            for (name, count) in &f.top_processes {
                Self::writeln(out, &format!("    {:<25} {}", name, count))?;
            }
        }
        Ok(())
    }

    fn render_context_switches(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let c = &data.context_switches;
        Self::writeln(out, &format!("  Total involuntary context switches: {}", c.total_csw))?;
        if !c.top_processes.is_empty() {
            Self::writeln(out, "  Top processes (involuntary ctx switches — high = CPU contention):")?;
            for (name, count) in &c.top_processes {
                Self::writeln(out, &format!("    {:<25} {}", name, count))?;
            }
        }
        Ok(())
    }

    fn render_socket_overview(out: &mut impl Write, data: &MonitorData) -> io::Result<()> {
        let s = &data.socket_overview;
        Self::writeln(out, &format!(
            "  ESTABLISHED: {}  LISTEN: {}  TIME_WAIT: {}  CLOSE_WAIT: {}  FIN_WAIT: {}",
            s.established, s.listen, s.time_wait, s.close_wait, s.fin_wait
        ))?;

        if s.close_wait > 10 {
            queue!(io::stdout(), SetForegroundColor(Color::Red))?;
            Self::writeln(out, &format!("  ⚠ High CLOSE_WAIT count ({}) — possible connection leak", s.close_wait))?;
            queue!(io::stdout(), ResetColor)?;
        }
        if s.time_wait > 100 {
            queue!(io::stdout(), SetForegroundColor(Color::Yellow))?;
            Self::writeln(out, &format!("  ⚠ High TIME_WAIT count ({}) — high connection churn", s.time_wait))?;
            queue!(io::stdout(), ResetColor)?;
        }

        if !s.top_processes.is_empty() {
            Self::writeln(out, "  Top processes by open connections:")?;
            for (name, count) in &s.top_processes {
                Self::writeln(out, &format!("    {:<25} {}", name, count))?;
            }
        }
        Ok(())
    }

    // --- Helpers ---

    fn writeln(out: &mut impl Write, text: &str) -> io::Result<()> {
        write!(out, "{}\r\n", text)
    }

    fn write_section_header(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
        if selected {
            queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
        } else {
            queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
        }
        write!(out, "{}\r\n", text)?;
        queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
        Ok(())
    }

    fn write_selectable(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
        if selected {
            queue!(io::stdout(), SetBackgroundColor(Color::DarkGrey), SetForegroundColor(Color::White))?;
        }
        write!(out, "{}\r\n", text)?;
        if selected {
            queue!(io::stdout(), ResetColor)?;
        }
        Ok(())
    }

    fn format_bytes_rate(bytes: u64) -> String {
        if bytes > 1_048_576 {
            format!("{:.2} MB/s", bytes as f64 / 1_048_576.0)
        } else if bytes > 1024 {
            format!("{:.2} KB/s", bytes as f64 / 1024.0)
        } else {
            format!("{} B/s", bytes)
        }
    }

    fn progress_bar(percent: f64, width: usize) -> String {
        let filled = ((percent / 100.0) * width as f64).round() as usize;
        let empty = width.saturating_sub(filled);
        format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
    }
}
