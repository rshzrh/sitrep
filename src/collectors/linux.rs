use super::SystemCollector;
use crate::model::{ContextSwitchInfo, FdInfo, SocketOverviewInfo};
use sysinfo::Pid;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

pub struct LinuxCollector {
    /// Previous per-device I/O tick counts (ms) from /proc/diskstats.
    prev_disk_ticks: HashMap<String, u64>,
    prev_disk_time: Option<Instant>,

    /// Previous per-interface (rx_bytes, tx_bytes) from /proc/net/dev.
    prev_net_bytes: HashMap<String, (u64, u64)>,
    prev_net_time: Option<Instant>,

    /// Running cumulative estimate of per-PID network bytes (rx, tx).
    /// Grows over time so the returned values behave like macOS `nettop`
    /// (which reports lifetime bytes per process).
    cumulative_net: HashMap<u32, (u64, u64)>,
}

impl LinuxCollector {
    pub fn new() -> Self {
        Self {
            prev_disk_ticks: HashMap::new(),
            prev_disk_time: None,
            prev_net_bytes: HashMap::new(),
            prev_net_time: None,
            cumulative_net: HashMap::new(),
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Read /proc/diskstats and return device_name → io_ticks (ms) for real
    /// block devices only (partitions are excluded).
    fn read_diskstats() -> HashMap<String, u64> {
        let mut result = HashMap::new();
        let content = match fs::read_to_string("/proc/diskstats") {
            Ok(c) => c,
            Err(_) => return result,
        };
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // Fields (0-indexed):
            //  0  major
            //  1  minor
            //  2  name
            //  3  reads completed
            //  …
            // 12  io_ticks (ms spent doing I/Os)
            if parts.len() >= 13 {
                let name = parts[2];
                if is_block_device(name) {
                    if let Ok(ticks) = parts[12].parse::<u64>() {
                        result.insert(name.to_string(), ticks);
                    }
                }
            }
        }
        result
    }

    /// Read /proc/net/dev and return interface → (rx_bytes, tx_bytes),
    /// excluding the loopback adapter.
    fn read_net_dev() -> HashMap<String, (u64, u64)> {
        let mut result = HashMap::new();
        let content = match fs::read_to_string("/proc/net/dev") {
            Ok(c) => c,
            Err(_) => return result,
        };
        // First two lines are headers.
        for line in content.lines().skip(2) {
            let line = line.trim();
            if let Some((iface, rest)) = line.split_once(':') {
                let iface = iface.trim();
                if iface == "lo" {
                    continue;
                }
                let cols: Vec<&str> = rest.split_whitespace().collect();
                // rx_bytes is col 0, tx_bytes is col 8.
                if cols.len() >= 10 {
                    let rx = cols[0].parse::<u64>().unwrap_or(0);
                    let tx = cols[8].parse::<u64>().unwrap_or(0);
                    result.insert(iface.to_string(), (rx, tx));
                }
            }
        }
        result
    }

    /// Scan /proc/[pid]/fd/ to build a mapping of socket inode → (pid, comm).
    fn build_socket_pid_map() -> HashMap<u64, (u32, String)> {
        let mut map: HashMap<u64, (u32, String)> = HashMap::new();
        let entries = match fs::read_dir("/proc") {
            Ok(e) => e,
            Err(_) => return map,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let fname = match path.file_name() {
                Some(f) => f.to_string_lossy().to_string(),
                None => continue,
            };
            let pid: u32 = match fname.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            let comm = fs::read_to_string(path.join("comm"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| fname.clone());

            let fd_dir = path.join("fd");
            let fds = match fs::read_dir(&fd_dir) {
                Ok(f) => f,
                Err(_) => continue, // permission denied or process gone
            };
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path()) {
                    let t = target.to_string_lossy();
                    if let Some(inode_str) =
                        t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']'))
                    {
                        if let Ok(inode) = inode_str.parse::<u64>() {
                            map.insert(inode, (pid, comm.clone()));
                        }
                    }
                }
            }
        }
        map
    }

    /// Parse /proc/net/tcp and /proc/net/tcp6.
    /// Returns Vec<(inode, tcp_state)>.
    fn read_tcp_entries() -> Vec<(u64, u8)> {
        let mut entries = Vec::new();
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(content) = fs::read_to_string(path) {
                for line in content.lines().skip(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    // col 3 = state (hex), col 9 = inode
                    if parts.len() >= 10 {
                        let state = u8::from_str_radix(parts[3], 16).unwrap_or(0);
                        let inode = parts[9].parse::<u64>().unwrap_or(0);
                        if inode > 0 {
                            entries.push((inode, state));
                        }
                    }
                }
            }
        }
        entries
    }
}

// ── block-device detection ──────────────────────────────────────────────

/// Return `true` if `name` looks like a whole block device rather than a
/// partition.  Uses /sys/block/<name> when available, otherwise falls back
/// to name-pattern heuristics.
fn is_block_device(name: &str) -> bool {
    // Fast path: the kernel exposes every real block device here.
    if Path::new(&format!("/sys/block/{}", name)).exists() {
        return true;
    }
    // Fallback heuristics when /sys is unavailable (e.g. some containers).
    // sda, sdb  (SCSI/SATA — not sda1)
    if name.starts_with("sd") && name.len() == 3 && name.as_bytes()[2].is_ascii_alphabetic() {
        return true;
    }
    // nvme0n1 (NVMe — not nvme0n1p1)
    if name.starts_with("nvme") && name.contains('n') && !name.contains('p') {
        return true;
    }
    // vda, vdb (virtio — not vda1)
    if name.starts_with("vd") && name.len() == 3 && name.as_bytes()[2].is_ascii_alphabetic() {
        return true;
    }
    // xvda (Xen — not xvda1)
    if name.starts_with("xvd") && name.len() == 4 && name.as_bytes()[3].is_ascii_alphabetic() {
        return true;
    }
    // mmcblk0 (SD cards — not mmcblk0p1)
    if name.starts_with("mmcblk") && !name.contains('p') {
        return true;
    }
    // dm-0, dm-1 (device-mapper / LVM)
    if name.starts_with("dm-") {
        return true;
    }
    false
}

// ── trait implementation ────────────────────────────────────────────────

impl SystemCollector for LinuxCollector {
    /// Disk I/O busy percentage derived from /proc/diskstats io_ticks.
    ///
    /// The field `io_ticks` counts the number of milliseconds during which
    /// the device had I/O in progress.  By comparing two snapshots we get:
    ///
    ///     busy% = delta_io_ticks / elapsed_ms × 100
    ///
    /// We report the *maximum* busy% across all block devices so that a
    /// single saturated disk is immediately visible.
    fn get_disk_io_pct(&mut self) -> f64 {
        let now = Instant::now();
        let current = Self::read_diskstats();

        let result = match self.prev_disk_time {
            Some(prev_time) => {
                let elapsed_ms = prev_time.elapsed().as_millis() as f64;
                if elapsed_ms <= 0.0 || current.is_empty() {
                    0.0
                } else {
                    let mut max_busy = 0.0_f64;
                    for (dev, &cur_ticks) in &current {
                        if let Some(&prev_ticks) = self.prev_disk_ticks.get(dev) {
                            let delta = cur_ticks.saturating_sub(prev_ticks) as f64;
                            let busy = (delta / elapsed_ms) * 100.0;
                            max_busy = max_busy.max(busy.min(100.0));
                        }
                    }
                    max_busy
                }
            }
            None => 0.0, // first call — no previous snapshot yet
        };

        self.prev_disk_ticks = current;
        self.prev_disk_time = Some(now);
        result
    }

    /// File-descriptor statistics from /proc/sys/fs/file-nr (system-wide)
    /// and /proc/[pid]/fd (per-process top consumers).
    fn get_fd_stats(&self) -> FdInfo {
        let mut info = FdInfo::default();

        // ── system-wide ──
        // /proc/sys/fs/file-nr: "allocated  free  max"
        if let Ok(content) = fs::read_to_string("/proc/sys/fs/file-nr") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() >= 3 {
                let allocated: u64 = parts[0].parse().unwrap_or(0);
                let free: u64 = parts[1].parse().unwrap_or(0);
                info.system_used = allocated.saturating_sub(free);
                info.system_max = parts[2].parse().unwrap_or(0);
            }
        }

        // ── per-process: top 5 FD consumers ──
        let mut counts: Vec<(String, u64)> = Vec::new();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let fname = match path.file_name() {
                    Some(f) => f.to_string_lossy().to_string(),
                    None => continue,
                };
                if !fname.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }

                let fd_path = path.join("fd");
                if let Ok(fd_entries) = fs::read_dir(&fd_path) {
                    let count = fd_entries.count() as u64;
                    if count > 0 {
                        let name = fs::read_to_string(path.join("comm"))
                            .map(|s| s.trim().to_string())
                            .unwrap_or(fname);
                        counts.push((name, count));
                    }
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(5);
        info.top_processes = counts;
        info
    }

    /// Socket overview from /proc/net/tcp{,6} with per-process connection
    /// counts derived by mapping socket inodes back to owning PIDs.
    fn get_socket_stats(&self) -> SocketOverviewInfo {
        let mut info = SocketOverviewInfo::default();

        let tcp_entries = Self::read_tcp_entries();

        // ── aggregate state counts ──
        for &(_inode, st) in &tcp_entries {
            match st {
                0x01 => info.established += 1,
                0x04 | 0x05 => info.fin_wait += 1,
                0x06 => info.time_wait += 1,
                0x08 => info.close_wait += 1,
                0x0A => info.listen += 1,
                _ => {}
            }
        }

        // ── top processes by active connection count ──
        let socket_pid_map = Self::build_socket_pid_map();
        let mut proc_counts: HashMap<String, u32> = HashMap::new();

        for &(inode, st) in &tcp_entries {
            // Count ESTABLISHED, CLOSE_WAIT, LISTEN — the states most
            // relevant for triage (same filter the macOS lsof path uses).
            if matches!(st, 0x01 | 0x08 | 0x0A) {
                if let Some((_pid, name)) = socket_pid_map.get(&inode) {
                    *proc_counts.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }

        let mut top: Vec<(String, u32)> = proc_counts.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(5);
        info.top_processes = top;

        info
    }

    /// Context-switch statistics.
    ///
    /// * System-wide total from `/proc/stat` (`ctxt` line).
    /// * Per-process voluntary + involuntary from `/proc/[pid]/status`.
    fn get_context_switches(&self) -> ContextSwitchInfo {
        let mut info = ContextSwitchInfo::default();

        // ── system-wide total (lifetime since boot) ──
        if let Ok(stat) = fs::read_to_string("/proc/stat") {
            for line in stat.lines() {
                if let Some(rest) = line.strip_prefix("ctxt ") {
                    info.total_csw = rest.trim().parse().unwrap_or(0);
                    break;
                }
            }
        }

        // ── per-process ──
        let mut counts: Vec<(String, u64)> = Vec::new();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let fname = match path.file_name() {
                    Some(f) => f.to_string_lossy().to_string(),
                    None => continue,
                };
                if !fname.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }

                if let Ok(status) = fs::read_to_string(path.join("status")) {
                    let mut name = fname.clone();
                    let mut vol: u64 = 0;
                    let mut nonvol: u64 = 0;

                    for line in status.lines() {
                        if let Some(rest) = line.strip_prefix("Name:") {
                            name = rest.trim().to_string();
                        } else if let Some(rest) = line.strip_prefix("voluntary_ctxt_switches:") {
                            vol = rest.trim().parse().unwrap_or(0);
                        } else if let Some(rest) = line.strip_prefix("nonvoluntary_ctxt_switches:")
                        {
                            nonvol = rest.trim().parse().unwrap_or(0);
                        }
                    }
                    let total = vol + nonvol;
                    if total > 0 {
                        counts.push((name, total));
                    }
                }
            }
        }

        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(5);
        info.top_processes = counts;
        info
    }

    /// Per-process network bandwidth estimation.
    ///
    /// Linux doesn't expose per-process byte counters the way macOS `nettop`
    /// does.  We approximate by:
    ///
    /// 1. Reading system-wide interface counters from `/proc/net/dev`.
    /// 2. Computing the delta since the previous snapshot.
    /// 3. Mapping ESTABLISHED TCP sockets to PIDs via
    ///    `/proc/net/tcp{,6}` inodes → `/proc/[pid]/fd/` symlinks.
    /// 4. Distributing the total delta proportionally by each PID's share
    ///    of active connections.
    /// 5. Accumulating into a running per-PID total so that the returned
    ///    values behave like cumulative byte counts (matching the macOS
    ///    `nettop` semantics the controller expects).
    fn get_process_network_stats(&mut self) -> HashMap<Pid, (u64, u64)> {
        let now = Instant::now();
        let current_net = Self::read_net_dev();

        if let Some(prev_time) = self.prev_net_time {
            let elapsed = prev_time.elapsed().as_secs_f64();
            if elapsed > 0.0 && !current_net.is_empty() {
                // ── system-wide delta ──
                let mut total_delta_rx: u64 = 0;
                let mut total_delta_tx: u64 = 0;
                for (iface, &(cur_rx, cur_tx)) in &current_net {
                    if let Some(&(prev_rx, prev_tx)) = self.prev_net_bytes.get(iface) {
                        total_delta_rx += cur_rx.saturating_sub(prev_rx);
                        total_delta_tx += cur_tx.saturating_sub(prev_tx);
                    }
                }

                if total_delta_rx > 0 || total_delta_tx > 0 {
                    // ── map ESTABLISHED sockets to PIDs ──
                    let socket_pid_map = Self::build_socket_pid_map();
                    let tcp_entries = Self::read_tcp_entries();

                    let mut pid_conn_count: HashMap<u32, u64> = HashMap::new();
                    let mut total_conns: u64 = 0;

                    for &(inode, state) in &tcp_entries {
                        if state == 0x01 {
                            // ESTABLISHED
                            if let Some(&(pid, _)) = socket_pid_map.get(&inode) {
                                *pid_conn_count.entry(pid).or_insert(0) += 1;
                                total_conns += 1;
                            }
                        }
                    }

                    // ── distribute proportionally and accumulate ──
                    if total_conns > 0 {
                        for (&pid, &conn_count) in &pid_conn_count {
                            let fraction = conn_count as f64 / total_conns as f64;
                            let delta_rx = (total_delta_rx as f64 * fraction) as u64;
                            let delta_tx = (total_delta_tx as f64 * fraction) as u64;

                            let entry = self.cumulative_net.entry(pid).or_insert((0, 0));
                            entry.0 += delta_rx;
                            entry.1 += delta_tx;
                        }
                    }
                }
            }
        }

        self.prev_net_bytes = current_net;
        self.prev_net_time = Some(now);

        // Prune PIDs that no longer exist so the map doesn't grow unboundedly.
        self.cumulative_net
            .retain(|pid, _| Path::new(&format!("/proc/{}", pid)).exists());

        // Convert to the Pid type the controller expects.
        self.cumulative_net
            .iter()
            .map(|(&pid, &(rx, tx))| (Pid::from(pid as usize), (rx, tx)))
            .collect()
    }
}
