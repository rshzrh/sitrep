use sysinfo::Pid;
use std::collections::{HashSet, VecDeque};
use serde::Deserialize;

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
    Swarm,                 // Swarm cluster view
    SwarmServiceTasks(String, String), // (service_id, service_name)
    SwarmServiceLogs(String, String),  // (service_id, service_name)
}

// --- Log viewer state ---

pub struct LogViewState {
    pub container_id: String,
    pub container_name: String,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize,  // 0 = at bottom (following)
    pub auto_follow: bool,
    pub search_mode: bool,      // true when typing a search query
    pub search_query: String,   // current search text
}

impl LogViewState {
    pub fn new(container_id: String, container_name: String) -> Self {
        Self {
            container_id,
            container_name,
            lines: VecDeque::with_capacity(5000),
            scroll_offset: 0,
            auto_follow: true,
            search_mode: false,
            search_query: String::new(),
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

// --- Swarm data ---

/// Whether we're running standalone Docker or in a Swarm cluster
#[derive(Clone, Debug, PartialEq)]
pub enum SwarmMode {
    Standalone,
    Swarm,
}

/// Cluster-level overview
#[derive(Clone, Debug, Default)]
pub struct SwarmClusterInfo {
    pub node_id: String,
    pub node_addr: String,
    pub is_manager: bool,
    pub managers: u32,
    pub nodes_total: u32,
}

/// A single Swarm node
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmNodeInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "Status")]
    pub status: String,         // "Ready", "Down"
    #[serde(rename = "Availability")]
    pub availability: String,   // "Active", "Pause", "Drain"
    #[serde(rename = "ManagerStatus")]
    #[serde(default)]
    pub manager_status: String, // "Leader", "Reachable", ""
    #[serde(rename = "EngineVersion")]
    #[serde(default)]
    pub engine_version: String,
    #[serde(rename = "Self")]
    #[serde(default)]
    pub is_self: bool,
}

/// A Swarm service
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmServiceInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Mode")]
    #[serde(default)]
    pub mode: String,           // "replicated", "global"
    #[serde(rename = "Replicas")]
    #[serde(default)]
    pub replicas: String,       // "3/3"
    #[serde(rename = "Image")]
    #[serde(default)]
    pub image: String,
    #[serde(rename = "Ports")]
    #[serde(default)]
    pub ports: String,
    // Derived: stack name from label com.docker.stack.namespace
    #[serde(skip)]
    pub stack: String,
}

/// A Swarm task (replica of a service)
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmTaskInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Image")]
    #[serde(default)]
    pub image: String,
    #[serde(rename = "Node")]
    #[serde(default)]
    pub node: String,
    #[serde(rename = "DesiredState")]
    #[serde(default)]
    pub desired_state: String,
    #[serde(rename = "CurrentState")]
    #[serde(default)]
    pub current_state: String,
    #[serde(rename = "Error")]
    #[serde(default)]
    pub error: String,
    #[serde(rename = "Ports")]
    #[serde(default)]
    pub ports: String,
}

/// Stack: a group of services deployed together (uses indices into SwarmMonitor.services)
#[derive(Clone, Debug)]
pub struct SwarmStackInfo {
    pub name: String,
    pub service_indices: Vec<usize>,
}

/// What level of the Swarm drill-down we're viewing
#[derive(Clone, Debug, PartialEq)]
pub enum SwarmViewLevel {
    Overview,                        // nodes + stacks/services
    ServiceTasks(String, String),    // (service_id, service_name) -> task list
    ServiceLogs(String, String),     // (service_id, service_name) -> log viewer
}

/// UI state for the Swarm tab
pub struct SwarmUIState {
    pub view_level: SwarmViewLevel,
    pub selected_index: usize,
    pub expanded_ids: HashSet<String>,
}

impl Default for SwarmUIState {
    fn default() -> Self {
        Self {
            view_level: SwarmViewLevel::Overview,
            selected_index: 0,
            expanded_ids: HashSet::new(),
        }
    }
}

/// Log viewer state for service logs
pub struct ServiceLogState {
    pub service_id: String,
    pub service_name: String,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize,
    pub auto_follow: bool,
    pub filter_errors: bool,
    pub search_mode: bool,
    pub search_query: String,
}

impl ServiceLogState {
    pub fn new(service_id: String, service_name: String) -> Self {
        Self {
            service_id,
            service_name,
            lines: VecDeque::with_capacity(10000),
            scroll_offset: 0,
            auto_follow: true,
            filter_errors: false,
            search_mode: false,
            search_query: String::new(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.lines.len() >= 10000 {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
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
