use crate::model::{
    DiskSpaceInfo, FdInfo, SocketOverviewInfo, ContextSwitchInfo, 
    NetworkProcessInfo, NetworkInterfaceInfo
};
use sysinfo::Pid;
use std::collections::HashMap;

pub mod mac;
pub mod linux;

/// Trait for OS-specific system data collection.
/// Implementations (MacCollector, LinuxCollector) handle the low-level details.
pub trait SystemCollector {
    /// Collect disk I/O statistics (e.g. busy %, read/write rates).
    /// Returns a tuple of (busy_percent, additional_stats if any).
    /// Note: Currently we only really use busy_percent broadly, but structure allows expansion.
    /// For macOS, we used `iostat`.
    fn get_disk_io_pct(&mut self) -> f64;

    /// Collect file descriptor statistics.
    fn get_fd_stats(&self) -> FdInfo;

    /// Collect socket statistics (ESTABLISHED, TIME_WAIT, etc.).
    fn get_socket_stats(&self) -> SocketOverviewInfo;

    /// Collect context switch statistics.
    fn get_context_switches(&self) -> ContextSwitchInfo;

    /// Collect network bandwidth stats per process.
    /// Returns a map of Pid -> (rx_bytes, tx_bytes).
    fn get_process_network_stats(&mut self) -> HashMap<Pid, (u64, u64)>;
}
