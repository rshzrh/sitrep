//! Fleet overview state — the lowest-common-denominator view of N hosts.
//!
//! ┌──────────────────────────────────────────────────────────────────┐
//! │  FleetState                                                       │
//! │   ┌──────────────┐ ┌──────────────┐ ┌──────────────┐              │
//! │   │  HostEntry   │ │  HostEntry   │ │  HostEntry   │   …          │
//! │   │  ─────────   │ │  ─────────   │ │  ─────────   │              │
//! │   │ name "box1"  │ │ name "box2"  │ │ name "box3"  │              │
//! │   │ status UP    │ │ status DEG.  │ │ status DOWN  │              │
//! │   │ vitals { ... }│ │ vitals { ...}│ │ last_error  │              │
//! │   └──────────────┘ └──────────────┘ └──────────────┘              │
//! │                                                                   │
//! │   selected_index, scroll_offset, drilled_in (Option<usize>)       │
//! └──────────────────────────────────────────────────────────────────┘
//!
//! `FleetVitals` is the LCD struct: every field is something both a
//! local Mac/Linux box AND a remote Linux box can produce. Drill-in
//! views stay platform-specific; the fleet overview shows only what
//! all platforms can deliver.
//!
//! Status transitions:
//!     DISCONNECTED ──connect()──▶ UP
//!     UP ──refresh OK──▶ UP
//!     UP ──refresh fails──▶ DEGRADED  (last good vitals retained)
//!     DEGRADED ──refresh OK──▶ UP
//!     DEGRADED ──N consecutive failures──▶ DISCONNECTED

use std::time::Instant;

/// Connection / freshness state for a host. Mirrors
/// `crate::collectors::remote::ConnState` but lives in `model/` so the
/// fleet UI doesn't depend on the collector's internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    /// Last refresh succeeded recently.
    Up,
    /// Last refresh failed but we still have stale vitals to show.
    Degraded,
    /// Multiple consecutive refresh failures — connection is gone.
    Disconnected,
}

impl HostStatus {
    pub fn label(&self) -> &'static str {
        match self {
            HostStatus::Up => "UP",
            HostStatus::Degraded => "DEGRADED",
            HostStatus::Disconnected => "DOWN",
        }
    }
}

/// One process row shown in the fleet detail view. Plain data struct
/// that lives in model/ so view/ doesn't have to import from collectors/.
#[derive(Debug, Clone)]
pub struct FleetProcessRow {
    pub user: String,
    pub pid: u32,
    pub cpu_pct: f32,
    pub mem_pct: f32,
    pub command: String,
}

/// The display data for a single host. Used BOTH by the fleet overview
/// row (which shows only the LCD fields) and by the per-host detail
/// view (which shows everything here).
///
/// Optional fields default to None/0 when a host can't provide them
/// (e.g. SwapTotal=0 on a swap-disabled box, container_count=None for
/// v0.1 remote hosts that don't query Docker).
#[derive(Debug, Clone, Default)]
pub struct FleetVitals {
    pub load_1m: Option<f64>,
    pub load_5m: Option<f64>,
    pub load_15m: Option<f64>,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    /// Busiest mounted disk's % used. None means unknown.
    pub disk_busiest_pct: Option<f64>,
    /// Top CPU process name + cpu% — the one shown in the fleet row.
    pub top_proc: Option<(String, f32)>,
    /// Full top-N process list shown in the drill-in detail view.
    pub top_processes: Vec<FleetProcessRow>,
    /// Network interfaces: (name, rx_bytes, tx_bytes).
    pub interfaces: Vec<(String, u64, u64)>,
    /// Number of containers (Docker) running on this host. None if
    /// Docker isn't reachable on this remote (v0.1: always None for
    /// remote hosts).
    pub container_count: Option<u32>,
    /// Number of established TCP connections.
    pub established_conns: Option<u32>,
    pub time_wait_conns: Option<u32>,
    /// Total context switches since boot (from /proc/stat).
    pub ctxt_total: Option<u64>,
}

impl FleetVitals {
    /// Memory % used (0..=100). 0.0 if total is 0.
    pub fn mem_pct(&self) -> f64 {
        if self.mem_total_bytes == 0 {
            0.0
        } else {
            (self.mem_used_bytes as f64 / self.mem_total_bytes as f64) * 100.0
        }
    }

    /// Swap % used (0..=100). 0.0 if no swap configured.
    pub fn swap_pct(&self) -> f64 {
        if self.swap_total_bytes == 0 {
            0.0
        } else {
            (self.swap_used_bytes as f64 / self.swap_total_bytes as f64) * 100.0
        }
    }
}

/// One host entry in the fleet.
#[derive(Debug, Clone)]
pub struct HostEntry {
    /// Display name (typically the hostname or user@host:port).
    pub name: String,
    pub status: HostStatus,
    /// Last successful vitals. Retained across refresh failures so
    /// DEGRADED rows still show useful data.
    pub vitals: FleetVitals,
    /// Most recent error message, if any. Cleared on next success.
    pub last_error: Option<String>,
    /// Wall-clock instant of the last successful refresh. None means
    /// no successful refresh yet.
    pub last_refresh: Option<Instant>,
    /// Consecutive failures since the last success. Used by the surrounding
    /// task to drive backoff.
    pub consecutive_failures: u32,
}

impl HostEntry {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HostStatus::Disconnected,
            vitals: FleetVitals::default(),
            last_error: None,
            last_refresh: None,
            consecutive_failures: 0,
        }
    }

    /// Mark a successful refresh: bump status to UP, clear error,
    /// reset failure counter, store new vitals.
    pub fn record_success(&mut self, vitals: FleetVitals) {
        self.status = HostStatus::Up;
        self.vitals = vitals;
        self.last_error = None;
        self.last_refresh = Some(Instant::now());
        self.consecutive_failures = 0;
    }

    /// Mark a refresh failure. Keeps last good vitals. After
    /// `max_before_disconnect` consecutive failures, transitions to
    /// DISCONNECTED.
    pub fn record_failure(&mut self, error: String, max_before_disconnect: u32) {
        self.last_error = Some(error);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = if self.consecutive_failures >= max_before_disconnect {
            HostStatus::Disconnected
        } else {
            HostStatus::Degraded
        };
    }
}

/// Top-level fleet state. Owned by the App; rendered by `view/fleet.rs`.
#[derive(Debug, Clone)]
pub struct FleetState {
    pub hosts: Vec<HostEntry>,
    pub selected: usize,
    /// When `Some(i)`, the user is drilled into the per-host view for
    /// hosts[i] and the fleet overview is hidden. None means showing
    /// the fleet overview.
    pub drilled_in: Option<usize>,
}

impl FleetState {
    pub fn new(host_names: Vec<String>) -> Self {
        Self {
            hosts: host_names.into_iter().map(HostEntry::new).collect(),
            selected: 0,
            drilled_in: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    pub fn move_selection_down(&mut self) {
        if !self.hosts.is_empty() {
            self.selected = (self.selected + 1).min(self.hosts.len() - 1);
        }
    }

    pub fn move_selection_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn drill_in(&mut self) {
        if !self.hosts.is_empty() {
            self.drilled_in = Some(self.selected);
        }
    }

    pub fn drill_out(&mut self) {
        self.drilled_in = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1: FleetVitals::mem_pct math + zero guard ──
    #[test]
    fn fleet_vitals_mem_pct_handles_zero_total() {
        let v = FleetVitals::default();
        assert_eq!(v.mem_pct(), 0.0);

        let v = FleetVitals {
            mem_used_bytes: 256,
            mem_total_bytes: 1024,
            ..Default::default()
        };
        assert_eq!(v.mem_pct(), 25.0);
    }

    // ── 2: status label strings ──
    #[test]
    fn host_status_labels() {
        assert_eq!(HostStatus::Up.label(), "UP");
        assert_eq!(HostStatus::Degraded.label(), "DEGRADED");
        assert_eq!(HostStatus::Disconnected.label(), "DOWN");
    }

    // ── 3: record_success → UP, clears error, resets failure count ──
    #[test]
    fn host_entry_record_success_resets_state() {
        let mut h = HostEntry::new("box1");
        h.last_error = Some("old".into());
        h.consecutive_failures = 3;
        h.record_success(FleetVitals {
            mem_used_bytes: 100,
            mem_total_bytes: 200,
            ..Default::default()
        });
        assert_eq!(h.status, HostStatus::Up);
        assert!(h.last_error.is_none());
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_refresh.is_some());
        assert_eq!(h.vitals.mem_used_bytes, 100);
    }

    // ── 4: record_failure transitions DEGRADED → DISCONNECTED after N ──
    #[test]
    fn host_entry_record_failure_transitions_to_disconnected() {
        let mut h = HostEntry::new("box1");
        // Start UP with some vitals
        h.record_success(FleetVitals {
            mem_total_bytes: 1024,
            ..Default::default()
        });
        // First failure → DEGRADED
        h.record_failure("net err".into(), 3);
        assert_eq!(h.status, HostStatus::Degraded);
        // Last good vitals retained
        assert_eq!(h.vitals.mem_total_bytes, 1024);
        // Two more → still DEGRADED on second, DISCONNECTED on third
        h.record_failure("net err".into(), 3);
        assert_eq!(h.status, HostStatus::Degraded);
        h.record_failure("net err".into(), 3);
        assert_eq!(h.status, HostStatus::Disconnected);
        assert_eq!(h.consecutive_failures, 3);
    }

    // ── 5: FleetState selection navigation respects bounds ──
    #[test]
    fn fleet_state_selection_navigation_respects_bounds() {
        let mut s = FleetState::new(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(s.selected, 0);
        s.move_selection_up(); // already at top
        assert_eq!(s.selected, 0);
        s.move_selection_down();
        s.move_selection_down();
        s.move_selection_down(); // would overshoot
        assert_eq!(s.selected, 2);
    }

    // ── 6: drill_in / drill_out ──
    #[test]
    fn fleet_state_drill_in_and_out() {
        let mut s = FleetState::new(vec!["a".into(), "b".into()]);
        assert!(s.drilled_in.is_none());
        s.selected = 1;
        s.drill_in();
        assert_eq!(s.drilled_in, Some(1));
        s.drill_out();
        assert!(s.drilled_in.is_none());
    }

    // ── 7: empty fleet (local-only mode, no host args) is valid ──
    #[test]
    fn fleet_state_empty_is_valid_and_doesnt_panic() {
        let mut s = FleetState::new(vec![]);
        assert!(s.is_empty());
        s.move_selection_down();
        s.move_selection_up();
        s.drill_in();
        assert!(s.drilled_in.is_none());
    }
}
