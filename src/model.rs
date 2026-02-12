use sysinfo::Pid;
use std::collections::HashSet;

// --- Process-level data ---

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: Pid,
    pub cpu: f32,
    pub mem: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct ProcessGroup {
    pub pid: Pid,
    pub cpu: f64,
    pub mem: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub child_count: usize,
    pub name: String,
    pub children: Vec<ProcessInfo>,
}

// --- New diagnostic data structs ---

#[derive(Clone, Debug)]
pub struct DiskSpaceInfo {
    pub mount_point: String,
    pub total_gb: f64,
    pub available_gb: f64,
    pub percent_free: f64,
    #[allow(dead_code)]
    pub is_warning: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Clone, Debug)]
pub struct NetworkInterfaceInfo {
    pub name: String,
    pub rx_rate: u64,
    pub tx_rate: u64,
}

#[derive(Clone, Debug)]
pub struct NetworkProcessInfo {
    pub name: String,
    pub bandwidth: u64, // bytes/sec total (rx+tx)
}

#[derive(Clone, Debug, Default)]
pub struct NetworkInfo {
    pub interfaces: Vec<NetworkInterfaceInfo>,
    pub top_bandwidth_processes: Vec<NetworkProcessInfo>,
    pub established: u32,
    pub time_wait: u32,
    pub close_wait: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FdInfo {
    pub system_used: u64,
    pub system_max: u64,
    pub top_processes: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct ContextSwitchInfo {
    pub total_csw: u64,
    pub top_processes: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct SocketOverviewInfo {
    pub established: u32,
    pub listen: u32,
    pub time_wait: u32,
    pub close_wait: u32,
    pub fin_wait: u32,
    pub top_processes: Vec<(String, u32)>, // (name, connection count)
}

// --- Aggregated monitor data ---

pub struct MonitorData {
    pub time: String,
    pub core_count: f64,
    pub load_avg: (f64, f64, f64),
    pub historical_top: Vec<ProcessGroup>,
    pub disk_space: Vec<DiskSpaceInfo>,
    pub disk_busy_pct: f64,
    pub memory: MemoryInfo,
    pub network: NetworkInfo,
    pub fd_info: FdInfo,
    #[allow(dead_code)]
    pub context_switches: ContextSwitchInfo,
    pub socket_overview: SocketOverviewInfo,
}

// --- UI State ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortColumn {
    Cpu,
    Memory,
    Read,
    Write,
    NetDown,
    NetUp,
}

pub struct UIState {
    pub selected_index: usize,
    pub expanded_pids: HashSet<Pid>,
    pub total_rows: usize,
    pub sort_column: SortColumn,
}

impl Default for UIState {
    fn default() -> Self {
        Self {
            selected_index: 0,
            expanded_pids: HashSet::new(),
            total_rows: 0,
            sort_column: SortColumn::Cpu,
        }
    }
}
impl UIState {
    pub fn has_expansions(&self) -> bool {
        !self.expanded_pids.is_empty()
    }
}
