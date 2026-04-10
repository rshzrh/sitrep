//! Fleet overview rendering.
//!
//! The fleet overview is a single screen showing N hosts as rows. Each
//! row has: status indicator | hostname | load avg | mem % | disk % |
//! top CPU process | container count | last refresh / error.
//!
//! ┌────────────────────────────────────────────────────────────────────┐
//! │ FLEET OVERVIEW                                  flotop - 12:34:56  │
//! ├────────────────────────────────────────────────────────────────────┤
//! │ ● UP    box1.example.com   0.42  18%  42%  postgres   12  3s ago  │
//! │ ● UP    box2.example.com   1.21  65%  78%  nginx      4   2s ago  │
//! │ ✕ DEG   box3.example.com   ---   ---  ---  Auth fail  -   6s ago  │
//! └────────────────────────────────────────────────────────────────────┘
//!
//! Architecture: there's a pure `format_fleet_overview()` function that
//! returns Vec<String> (one line per row), and a `Presenter` method that
//! writes those lines to stdout via crossterm. This split makes snapshot
//! testing trivial — `insta` snapshots the formatted lines, not the
//! crossterm escape sequences.

use crate::model::{FleetState, HostEntry, HostStatus};
use crate::view::shared::truncate_str;

/// Pure formatter: produce one display line per host. Tests snapshot
/// these strings directly.
pub fn format_fleet_overview(state: &FleetState, cols: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let title_line = format!(
        "FLEET OVERVIEW — {} host{}",
        state.hosts.len(),
        if state.hosts.len() == 1 { "" } else { "s" }
    );
    lines.push(title_line);
    lines.push(header_row(cols));

    if state.hosts.is_empty() {
        lines.push("(no hosts — invoke flotop with one or more host arguments)".into());
        return lines;
    }

    for (i, host) in state.hosts.iter().enumerate() {
        let selected = i == state.selected;
        lines.push(format_host_row(host, selected, cols));
    }
    lines
}

fn header_row(cols: u16) -> String {
    // Columns: status(8) | host(20) | load(10) | mem(7) | disk(7) | top(20) | conns(7) | age(10)
    let raw = format!(
        "{:<8} {:<20} {:<10} {:<7} {:<7} {:<20} {:<7} {:<10}",
        "STATUS", "HOST", "LOAD", "MEM%", "DISK%", "TOP CPU", "ESTAB", "AGE",
    );
    truncate_str(&raw, cols as usize)
}

/// Format a single host row. The selected row is marked with a `▶` prefix
/// in column 0 (replaces the leading space). The pure formatter doesn't
/// emit color codes — that's the renderer's job.
pub fn format_host_row(
    host: &crate::model::HostEntry,
    selected: bool,
    cols: u16,
) -> String {
    let status_label = format!("{} {}", status_glyph(host.status), host.status.label());
    let host_name = truncate_str(&host.name, 20);

    let load = match (host.vitals.load_1m, host.vitals.load_5m, host.vitals.load_15m) {
        (Some(a), Some(b), Some(c)) => format!("{:.2} {:.2} {:.2}", a, b, c),
        _ => "—".into(),
    };
    let load = truncate_str(&load, 10);

    let mem_pct = if host.vitals.mem_total_bytes > 0 {
        format!("{:.0}%", host.vitals.mem_pct())
    } else {
        "—".into()
    };

    let disk_pct = match host.vitals.disk_busiest_pct {
        Some(p) => format!("{:.0}%", p),
        None => "—".into(),
    };

    let top = match &host.vitals.top_proc {
        Some((name, pct)) => format!("{} {:.0}%", truncate_str(name, 14), pct),
        None => host
            .last_error
            .as_deref()
            .map(|e| truncate_str(e, 20))
            .unwrap_or_else(|| "—".into()),
    };
    let top = truncate_str(&top, 20);

    let estab = match host.vitals.established_conns {
        Some(n) => format!("{}", n),
        None => "—".into(),
    };

    let age = match host.last_refresh {
        Some(t) => format_duration_since(t),
        None => "never".into(),
    };

    let prefix = if selected { "▶" } else { " " };

    let raw = format!(
        "{}{:<8} {:<20} {:<10} {:<7} {:<7} {:<20} {:<7} {:<10}",
        prefix, status_label, host_name, load, mem_pct, disk_pct, top, estab, age,
    );
    truncate_str(&raw, cols as usize)
}

fn status_glyph(s: HostStatus) -> &'static str {
    match s {
        HostStatus::Up => "●",
        HostStatus::Degraded => "◐",
        HostStatus::Disconnected => "✕",
    }
}

fn format_duration_since(t: std::time::Instant) -> String {
    let secs = t.elapsed().as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

// ─── drill-in detail view ───────────────────────────────────────────────
//
// ┌─ HOST: root@143.198.144.194:22 [UP] ────────────── 2s ago ─────────┐
// │                                                                    │
// │ LOAD   0.42 / 0.31 / 0.28   (1m / 5m / 15m)                        │
// │ MEM    3.6 GB / 8.3 GB used   [████████░░░░░░░░░░░]  43%           │
// │ SWAP   0 B / 0 B                                                   │
// │ DISK   busiest mount: 42%                                          │
// │ CONNS  8 established / 52 time-wait                                │
// │ CTXT   34,587,209,748 since boot                                   │
// │                                                                    │
// │ TOP PROCESSES (by CPU)                                             │
// │   USER       PID  %CPU  %MEM  COMMAND                              │
// │   root   618054   7.7   2.5   prefect                              │
// │   root  3695679   3.3   2.0   dockerd                              │
// │   ...                                                              │
// │                                                                    │
// │ INTERFACES                                                         │
// │   eth0   rx 9.4 GB   tx 1.2 GB                                     │
// │                                                                    │
// │ [Esc / ←] back to fleet                                            │
// └────────────────────────────────────────────────────────────────────┘

/// Pure formatter for the per-host drill-in detail view. Returns one
/// line per display row. Tests snapshot these strings directly.
pub fn format_remote_host_detail(host: &HostEntry, cols: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let cols_usize = cols as usize;

    let age = match host.last_refresh {
        Some(t) => format_duration_since(t),
        None => "never".into(),
    };

    // ── Header ──
    lines.push(truncate_str(
        &format!(
            "HOST: {}   [{}]   {}",
            host.name,
            host.status.label(),
            age
        ),
        cols_usize,
    ));
    if let Some(err) = &host.last_error {
        lines.push(truncate_str(&format!("  ⚠  {}", err), cols_usize));
    }
    lines.push(String::new());

    let v = &host.vitals;

    // ── Load avg ──
    let load_str = match (v.load_1m, v.load_5m, v.load_15m) {
        (Some(a), Some(b), Some(c)) => {
            format!("LOAD    {:.2} / {:.2} / {:.2}   (1m / 5m / 15m)", a, b, c)
        }
        _ => "LOAD    —".into(),
    };
    lines.push(truncate_str(&load_str, cols_usize));

    // ── Memory ──
    let mem_line = if v.mem_total_bytes > 0 {
        format!(
            "MEM     {} / {} used   {}  {:.0}%",
            format_bytes(v.mem_used_bytes),
            format_bytes(v.mem_total_bytes),
            render_bar(v.mem_pct() / 100.0, 20),
            v.mem_pct(),
        )
    } else {
        "MEM     —".into()
    };
    lines.push(truncate_str(&mem_line, cols_usize));

    // ── Swap ──
    let swap_line = if v.swap_total_bytes > 0 {
        format!(
            "SWAP    {} / {} used   {}  {:.0}%",
            format_bytes(v.swap_used_bytes),
            format_bytes(v.swap_total_bytes),
            render_bar(v.swap_pct() / 100.0, 20),
            v.swap_pct(),
        )
    } else {
        "SWAP    (none configured)".into()
    };
    lines.push(truncate_str(&swap_line, cols_usize));

    // ── Disk busy ──
    let disk_line = match v.disk_busiest_pct {
        Some(p) => format!("DISK    busiest mount: {:.0}% busy", p),
        None => "DISK    —".into(),
    };
    lines.push(truncate_str(&disk_line, cols_usize));

    // ── TCP connections ──
    let conns_line = match (v.established_conns, v.time_wait_conns) {
        (Some(e), Some(t)) => format!("CONNS   {} established / {} time-wait", e, t),
        (Some(e), None) => format!("CONNS   {} established", e),
        _ => "CONNS   —".into(),
    };
    lines.push(truncate_str(&conns_line, cols_usize));

    // ── Context switches ──
    if let Some(c) = v.ctxt_total {
        lines.push(truncate_str(
            &format!("CTXT    {} switches since boot", format_number(c)),
            cols_usize,
        ));
    }

    lines.push(String::new());

    // ── Top processes ──
    lines.push(truncate_str("TOP PROCESSES (by CPU)", cols_usize));
    lines.push(truncate_str(
        &format!("  {:<10} {:>7} {:>5} {:>5}  {}", "USER", "PID", "%CPU", "%MEM", "COMMAND"),
        cols_usize,
    ));
    if v.top_processes.is_empty() {
        lines.push(truncate_str("  (no process data)", cols_usize));
    } else {
        for p in v.top_processes.iter().take(12) {
            let row = format!(
                "  {:<10} {:>7} {:>5.1} {:>5.1}  {}",
                truncate_str(&p.user, 10),
                p.pid,
                p.cpu_pct,
                p.mem_pct,
                p.command,
            );
            lines.push(truncate_str(&row, cols_usize));
        }
    }

    lines.push(String::new());

    // ── Interfaces ──
    if !v.interfaces.is_empty() {
        lines.push(truncate_str("INTERFACES", cols_usize));
        for (name, rx, tx) in &v.interfaces {
            lines.push(truncate_str(
                &format!("  {:<10} rx {}   tx {}", name, format_bytes(*rx), format_bytes(*tx)),
                cols_usize,
            ));
        }
        lines.push(String::new());
    }

    lines.push(truncate_str(
        "[Esc / ←] back to fleet    [q] quit",
        cols_usize,
    ));

    lines
}

fn format_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    // Promote the unit early (at 1000, not 1024) so we never show
    // "1000.0 KB" — reads as "didn't finish the conversion".
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

fn format_number(n: u64) -> String {
    // Thousands separators — manual to avoid the num-format crate.
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.insert(0, ',');
        }
        out.insert(0, c);
    }
    out
}

fn render_bar(frac: f64, width: usize) -> String {
    let filled = (frac.clamp(0.0, 1.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FleetState, FleetVitals, HostEntry, HostStatus};

    fn host_with_vitals(name: &str, status: HostStatus, vitals: FleetVitals) -> HostEntry {
        let mut h = HostEntry::new(name);
        h.status = status;
        h.vitals = vitals;
        h
    }

    // ── 1: 1 host UP — happy path snapshot ──
    #[test]
    fn snapshot_single_host_up() {
        let mut state = FleetState::new(vec![]);
        state.hosts.push(host_with_vitals(
            "box1.example.com",
            HostStatus::Up,
            FleetVitals {
                load_1m: Some(0.42),
                load_5m: Some(0.31),
                load_15m: Some(0.28),
                mem_used_bytes: 4_000_000_000,
                mem_total_bytes: 16_000_000_000,
                disk_busiest_pct: Some(42.0),
                top_proc: Some(("postgres".into(), 87.5)),
                container_count: Some(12),
                established_conns: Some(87),
                ..Default::default()
            },
        ));
        let lines = format_fleet_overview(&state, 120);
        insta::assert_snapshot!(lines.join("\n"));
    }

    // ── 2: 5 hosts in mixed states ──
    #[test]
    fn snapshot_five_hosts_mixed_states() {
        let mut state = FleetState::new(vec![]);
        state.hosts.push(host_with_vitals(
            "web-01",
            HostStatus::Up,
            FleetVitals {
                load_1m: Some(0.5),
                load_5m: Some(0.4),
                load_15m: Some(0.3),
                mem_used_bytes: 800_000_000,
                mem_total_bytes: 2_000_000_000,
                disk_busiest_pct: Some(15.0),
                top_proc: Some(("nginx".into(), 12.0)),
                container_count: Some(3),
                established_conns: Some(42),
                ..Default::default()
            },
        ));
        state.hosts.push(host_with_vitals(
            "db-01",
            HostStatus::Up,
            FleetVitals {
                load_1m: Some(2.5),
                load_5m: Some(2.1),
                load_15m: Some(1.8),
                mem_used_bytes: 12_000_000_000,
                mem_total_bytes: 16_000_000_000,
                disk_busiest_pct: Some(78.0),
                top_proc: Some(("postgres".into(), 92.0)),
                container_count: Some(2),
                established_conns: Some(190),
                ..Default::default()
            },
        ));
        let mut degraded = host_with_vitals(
            "worker-01",
            HostStatus::Degraded,
            FleetVitals {
                load_1m: Some(0.1),
                load_5m: Some(0.2),
                load_15m: Some(0.3),
                mem_used_bytes: 500_000_000,
                mem_total_bytes: 1_000_000_000,
                disk_busiest_pct: Some(50.0),
                top_proc: Some(("python".into(), 5.0)),
                container_count: Some(1),
                established_conns: Some(8),
                ..Default::default()
            },
        );
        degraded.last_error = Some("connection reset".into());
        state.hosts.push(degraded);
        let mut down = HostEntry::new("worker-02");
        down.status = HostStatus::Disconnected;
        down.last_error = Some("Auth failed: bad key".into());
        state.hosts.push(down);
        let mut never = HostEntry::new("box-new.example.com");
        never.status = HostStatus::Disconnected;
        state.hosts.push(never);
        state.selected = 1;
        let lines = format_fleet_overview(&state, 120);
        insta::assert_snapshot!(lines.join("\n"));
    }

    // ── 3: 0 hosts (local-only invocation, no host args) ──
    #[test]
    fn snapshot_zero_hosts() {
        let state = FleetState::new(vec![]);
        let lines = format_fleet_overview(&state, 120);
        insta::assert_snapshot!(lines.join("\n"));
    }

    // ── detail view: host UP with rich vitals ──
    #[test]
    fn snapshot_remote_host_detail_up() {
        use crate::model::FleetProcessRow;
        let mut host = HostEntry::new("root@143.198.144.194:22");
        host.status = HostStatus::Up;
        host.last_refresh = None; // keep AGE deterministic for snapshot
        host.vitals = FleetVitals {
            load_1m: Some(0.42),
            load_5m: Some(0.31),
            load_15m: Some(0.28),
            mem_used_bytes: 3_900_000_000,
            mem_total_bytes: 8_300_000_000,
            swap_used_bytes: 0,
            swap_total_bytes: 0,
            disk_busiest_pct: Some(42.0),
            top_proc: Some(("prefect".into(), 7.7)),
            top_processes: vec![
                FleetProcessRow {
                    user: "root".into(),
                    pid: 618054,
                    cpu_pct: 7.7,
                    mem_pct: 2.5,
                    command: "prefect".into(),
                },
                FleetProcessRow {
                    user: "root".into(),
                    pid: 3695679,
                    cpu_pct: 3.3,
                    mem_pct: 2.0,
                    command: "dockerd".into(),
                },
                FleetProcessRow {
                    user: "root".into(),
                    pid: 99116,
                    cpu_pct: 2.4,
                    mem_pct: 0.6,
                    command: "containerd".into(),
                },
            ],
            interfaces: vec![
                ("eth0".into(), 9_400_000_000, 1_200_000_000),
                ("docker0".into(), 1_024_000, 2_048_000),
            ],
            container_count: None,
            established_conns: Some(8),
            time_wait_conns: Some(52),
            ctxt_total: Some(34_587_209_748),
        };
        let lines = format_remote_host_detail(&host, 100);
        insta::assert_snapshot!(lines.join("\n"));
    }

    // ── detail view: disconnected host shows error ──
    #[test]
    fn snapshot_remote_host_detail_disconnected() {
        let mut host = HostEntry::new("down.example.com");
        host.status = HostStatus::Disconnected;
        host.last_error = Some("Auth failed: no key accepted".into());
        let lines = format_remote_host_detail(&host, 100);
        insta::assert_snapshot!(lines.join("\n"));
    }

    // ── 4: long hostname truncation ──
    #[test]
    fn snapshot_long_hostname_truncation() {
        let mut state = FleetState::new(vec![]);
        state.hosts.push(host_with_vitals(
            "ridiculously-long-hostname-that-overflows-the-column.production.us-east-1.example.com",
            HostStatus::Up,
            FleetVitals {
                load_1m: Some(1.0),
                load_5m: Some(1.0),
                load_15m: Some(1.0),
                mem_used_bytes: 500,
                mem_total_bytes: 1000,
                disk_busiest_pct: Some(50.0),
                top_proc: Some(("an-equally-long-process-name".into(), 50.0)),
                container_count: Some(1),
                established_conns: Some(1),
                ..Default::default()
            },
        ));
        let lines = format_fleet_overview(&state, 120);
        // Hostname column is 20 chars wide — verify the host_name field
        // doesn't blow out the row width.
        let row = &lines[2];
        assert!(
            row.len() <= 120,
            "row exceeds terminal width: {} chars",
            row.len()
        );
        insta::assert_snapshot!(lines.join("\n"));
    }
}
