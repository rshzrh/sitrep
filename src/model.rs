use sysinfo::Pid;
use std::collections::{HashSet, VecDeque};

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

// --- Docker data ---

#[derive(Clone, Debug)]
pub struct DockerContainerInfo {
    pub id: String,           // short ID (first 12 chars)
    pub name: String,         // container name
    pub image: String,        // image name (shown when expanded)
    pub status: String,       // "running", "paused", etc.
    pub state: String,        // raw state string from Docker
    pub uptime: String,       // human-readable (e.g. "2h 34m")
    pub cpu_percent: f64,     // from stats
    pub ports: String,        // e.g. "0.0.0.0:8080->80/tcp"
    pub ip_address: String,   // internal IP from NetworkSettings
}

// --- App-level view state ---

#[derive(Clone, Debug, PartialEq)]
pub enum AppView {
    System,
    Containers,
    ContainerLogs(String), // container ID
}

// --- Log viewer state ---

pub struct LogViewState {
    pub container_id: String,
    pub container_name: String,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize,  // 0 = at bottom (following)
    pub auto_follow: bool,
}

impl LogViewState {
    pub fn new(container_id: String, container_name: String) -> Self {
        Self {
            container_id,
            container_name,
            lines: VecDeque::with_capacity(5000),
            scroll_offset: 0,
            auto_follow: true,
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.lines.len() >= 5000 {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

// --- Container UI state ---

pub struct ContainerUIState {
    pub selected_index: usize,
    pub total_rows: usize,
    pub expanded_ids: HashSet<String>,
}

impl Default for ContainerUIState {
    fn default() -> Self {
        Self {
            selected_index: 0,
            total_rows: 0,
            expanded_ids: HashSet::new(),
        }
    }
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
