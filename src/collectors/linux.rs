use super::SystemCollector;
use super::parsers::{
    parse_diskstats, parse_file_nr, parse_net_dev, parse_proc_stat_ctxt,
    parse_proc_status_ctxt, parse_tcp_table,
};
use crate::model::{ContextSwitchInfo, FdInfo, SocketOverviewInfo};
use sysinfo::Pid;
use std::cell::RefCell;
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

    socket_scan_cache: RefCell<Option<SocketScanCache>>,
}

struct SocketScanCache {
    scanned_at: Instant,
    socket_pid_map: HashMap<u64, (u32, String)>,
    tcp_entries: Vec<(u64, u8)>,
}

impl LinuxCollector {
    pub fn new() -> Self {
        Self {
            prev_disk_ticks: HashMap::new(),
            prev_disk_time: None,
            prev_net_bytes: HashMap::new(),
            prev_net_time: None,
            cumulative_net: HashMap::new(),
            socket_scan_cache: RefCell::new(None),
        }
    }

    // ── thin I/O wrappers around pure parsers ───────────────────────────
    //
    // These read /proc files from disk and delegate parsing to
    // `super::parsers`. Same parsers are used by RemoteLinuxCollector to
    // parse SSH-fetched output, so they only exist in one place.

    fn read_diskstats() -> HashMap<String, u64> {
        fs::read_to_string("/proc/diskstats")
            .map(|c| parse_diskstats(&c))
            .unwrap_or_default()
    }

    fn read_net_dev() -> HashMap<String, (u64, u64)> {
        fs::read_to_string("/proc/net/dev")
            .map(|c| parse_net_dev(&c))
            .unwrap_or_default()
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

    /// Read /proc/net/tcp and /proc/net/tcp6, parse via the shared parser.
    fn read_tcp_entries() -> Vec<(u64, u8)> {
        let mut entries = Vec::new();
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(content) = fs::read_to_string(path) {
                entries.extend(parse_tcp_table(&content));
            }
        }
        entries
    }

    fn socket_scan_ttl() -> std::time::Duration {
        std::time::Duration::from_secs(1)
    }

    fn get_socket_scan(&self) -> (HashMap<u64, (u32, String)>, Vec<(u64, u8)>) {
        let now = Instant::now();
        if let Some(cache) = self.socket_scan_cache.borrow().as_ref() {
            if now.duration_since(cache.scanned_at) < Self::socket_scan_ttl() {
                return (cache.socket_pid_map.clone(), cache.tcp_entries.clone());
            }
        }

        let socket_pid_map = Self::build_socket_pid_map();
        let tcp_entries = Self::read_tcp_entries();
        *self.socket_scan_cache.borrow_mut() = Some(SocketScanCache {
            scanned_at: now,
            socket_pid_map: socket_pid_map.clone(),
            tcp_entries: tcp_entries.clone(),
        });
        (socket_pid_map, tcp_entries)
    }
}

// (`is_block_device` lives in `super::parsers` so it can be used by both
// the local file reader and the SSH-fetched-output parser.)

// ── trait implementation ────────────────────────────────────────────────

impl SystemCollector for LinuxCollector {
    /// Disk I/O busy percentage derived from /proc/diskstats io_ticks.
    ///
    /// The field `io_ticks` counts the number of milliseconds during which
    /// the device had I/O in progress.  By comparing two snapshots we get:
    ///
    /// ```text
    /// busy% = delta_io_ticks / elapsed_ms * 100
    /// ```
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
        if let Ok(content) = fs::read_to_string("/proc/sys/fs/file-nr") {
            if let Some((used, max)) = parse_file_nr(&content) {
                info.system_used = used;
                info.system_max = max;
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

        let (socket_pid_map, tcp_entries) = self.get_socket_scan();

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
            info.total_csw = parse_proc_stat_ctxt(&stat).unwrap_or(0);
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
                    let parsed = parse_proc_status_ctxt(&status);
                    let total = parsed.total();
                    if total > 0 {
                        let name = if parsed.name.is_empty() {
                            fname
                        } else {
                            parsed.name
                        };
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
                    let (socket_pid_map, tcp_entries) = self.get_socket_scan();

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
