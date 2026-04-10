use crate::model::{
    FdInfo, SocketOverviewInfo, ContextSwitchInfo,
};
use sysinfo::Pid;
use std::collections::HashMap;

pub mod mac;
pub mod linux;
pub mod parsers;
pub mod remote;
#[cfg(test)]
mod tests;

/// A complete one-shot snapshot of all system metrics a collector can produce.
///
/// `collect_snapshot()` returns this. Local collectors get a default impl
/// that just calls the existing five per-metric methods. Remote collectors
/// (`RemoteLinuxCollector`) override `collect_snapshot()` with one batched
/// SSH command, avoiding the 5x round-trip cost of calling the per-metric
/// methods over the network.
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    pub disk_io_pct: f64,
    pub fd: FdInfo,
    pub sockets: SocketOverviewInfo,
    pub ctxt: ContextSwitchInfo,
    pub proc_net: HashMap<Pid, (u64, u64)>,
}

/// Trait for OS-specific system data collection.
/// Implementations (MacCollector, LinuxCollector, RemoteLinuxCollector)
/// handle the low-level details.
pub trait SystemCollector: Send {
    /// Collect disk I/O statistics (busy %).
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

    /// Collect everything in one shot. Default impl calls the five
    /// per-metric methods sequentially — fine for local collectors where
    /// each call is ~free. Remote collectors should override this with a
    /// single batched SSH command.
    fn collect_snapshot(&mut self) -> SystemSnapshot {
        SystemSnapshot {
            disk_io_pct: self.get_disk_io_pct(),
            fd: self.get_fd_stats(),
            sockets: self.get_socket_stats(),
            ctxt: self.get_context_switches(),
            proc_net: self.get_process_network_stats(),
        }
    }
}
