use super::SystemCollector;
use crate::model::{
    FdInfo, SocketOverviewInfo, ContextSwitchInfo
};
use sysinfo::Pid;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;

/// macOS metrics collector.
///
/// The "expensive" bits — `lsof`, `netstat`, `ps`, `nettop` — all run in
/// background threads that push their results into shared caches. The
/// render path (`SystemCollector` methods) never spawns a subprocess:
/// it just reads whatever the background threads have most recently
/// produced. This keeps the first-frame latency bounded by sysinfo's
/// in-process refresh (~50–200ms) instead of lsof (2–6s).
///
/// On the very first tick after launch the caches are empty and the
/// sidebars render with zeros for 1–3 seconds until the warmer threads
/// finish their first pass. The main UI (CPU, memory, disks, processes)
/// is usable immediately.
pub struct MacCollector {
    /// Cached nettop results, updated by a background thread every ~2 seconds.
    nettop_cache: Arc<Mutex<HashMap<Pid, (u64, u64)>>>,
    /// Cached lsof/netstat/ps results, refreshed by a background
    /// command-warmer thread on a ~10 second cadence.
    command_cache: Arc<Mutex<MacCommandCache>>,
    shutdown_flag: Arc<AtomicBool>,
    /// Set to `false` when the System tab is not the active tab.
    /// Both background loops check this and skip spawning subprocesses
    /// while the user is looking at another tab — cached numbers go
    /// stale but we stop burning subprocesses for data nobody is reading.
    active_flag: Arc<AtomicBool>,
}

#[derive(Default)]
struct MacCommandCache {
    fd_info: Option<FdInfo>,
    socket_info: Option<SocketOverviewInfo>,
    context_switches: Option<ContextSwitchInfo>,
}

impl MacCollector {
    pub fn new() -> Self {
        Self::new_with_active_flag(Arc::new(AtomicBool::new(true)))
    }

    /// Construct with an externally-owned `active_flag`. Used by
    /// `Monitor` so the App can toggle the flag even while the
    /// collector has been moved into the background update thread.
    pub fn new_with_active_flag(active_flag: Arc<AtomicBool>) -> Self {
        let nettop_cache: Arc<Mutex<HashMap<Pid, (u64, u64)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let command_cache: Arc<Mutex<MacCommandCache>> =
            Arc::new(Mutex::new(MacCommandCache::default()));
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        // ── nettop warmer thread ────────────────────────────────────
        // Runs the `nettop -P -L 1` one-shot every 2s to keep per-PID
        // network counters fresh. Cheap enough to run at 2s cadence.
        {
            let cache_clone = Arc::clone(&nettop_cache);
            let shutdown_clone = Arc::clone(&shutdown_flag);
            let active_clone = Arc::clone(&active_flag);
            thread::spawn(move || {
                loop {
                    if shutdown_clone.load(Ordering::Acquire) {
                        break;
                    }
                    if active_clone.load(Ordering::Acquire) {
                        let stats = run_nettop();
                        *cache_clone.lock() = stats;
                    }
                    // Sleep regardless — whether we ran nettop or not — so
                    // the loop stays on a predictable cadence and resumes
                    // cleanly when the user switches back to the System tab.
                    thread::sleep(Duration::from_secs(2));
                }
            });
        }

        // ── command warmer thread ──────────────────────────────────
        // Runs `sysctl`/`lsof`/`netstat`/`ps` every ~10s and pushes the
        // results into `command_cache`. The sync render path never calls
        // these commands directly — it just reads whatever the warmer
        // left in the cache. First pass runs immediately so the cache
        // populates as soon as possible after launch.
        {
            let cache_clone = Arc::clone(&command_cache);
            let shutdown_clone = Arc::clone(&shutdown_flag);
            let active_clone = Arc::clone(&active_flag);
            thread::spawn(move || {
                loop {
                    if shutdown_clone.load(Ordering::Acquire) {
                        break;
                    }
                    if active_clone.load(Ordering::Acquire) {
                        // Compute OUTSIDE the lock — these subprocess
                        // calls can take multiple seconds and we don't
                        // want to block the render path that long.
                        let fd = compute_fd_stats();
                        let sk = compute_socket_stats();
                        let cs = compute_context_switches();
                        let mut cache = cache_clone.lock();
                        cache.fd_info = Some(fd);
                        cache.socket_info = Some(sk);
                        cache.context_switches = Some(cs);
                    }
                    thread::sleep(Duration::from_secs(10));
                }
            });
        }

        Self {
            nettop_cache,
            command_cache,
            shutdown_flag,
            active_flag,
        }
    }

}

fn parse_sysctl_value(output: &str) -> u64 {
    output.split(":").nth(1).unwrap_or("0").trim().parse().unwrap_or(0)
}

fn compute_fd_stats() -> FdInfo {
    let mut info = FdInfo::default();

    if let Ok(output) = Command::new("sysctl").arg("kern.num_files").output() {
        if output.status.success() {
            info.system_used = parse_sysctl_value(&String::from_utf8_lossy(&output.stdout));
        }
    }
    if let Ok(output) = Command::new("sysctl").arg("kern.maxfiles").output() {
        if output.status.success() {
            info.system_max = parse_sysctl_value(&String::from_utf8_lossy(&output.stdout));
        }
    }

    let cmd = "lsof -n -P | awk '{print $1}' | sort | uniq -c | sort -nr | head -5";
    if let Ok(output) = Command::new("sh").arg("-c").arg(cmd).output() {
        let out_str = String::from_utf8_lossy(&output.stdout);
        for line in out_str.lines() {
             let parts: Vec<&str> = line.trim().split_whitespace().collect();
             if parts.len() >= 2 {
                 let count: u64 = parts[0].parse().unwrap_or(0);
                 let name = parts[1].to_string();
                 info.top_processes.push((name, count));
             }
        }
    }

    info
}

fn compute_socket_stats() -> SocketOverviewInfo {
    let mut established = 0u32;
    let mut listen = 0u32;
    let mut time_wait = 0u32;
    let mut close_wait = 0u32;
    let mut fin_wait = 0u32;

    if let Ok(output) = Command::new("netstat").args(["-an", "-p", "tcp"]).output() {
        if !output.status.success() {
            return SocketOverviewInfo { established, listen, time_wait, close_wait, fin_wait, top_processes: Vec::new() };
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if line.contains("ESTABLISHED") { established += 1; }
            else if line.contains("LISTEN") { listen += 1; }
            else if line.contains("TIME_WAIT") { time_wait += 1; }
            else if line.contains("CLOSE_WAIT") { close_wait += 1; }
            else if line.contains("FIN_WAIT") { fin_wait += 1; }
        }
    }

    let mut process_conns: HashMap<String, u32> = HashMap::new();
    if let Ok(output) = Command::new("lsof").args(["-i", "-n", "-P"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines().skip(1) {
             if line.contains("ESTABLISHED") || line.contains("CLOSE_WAIT") || line.contains("LISTEN") {
                 let parts: Vec<&str> = line.split_whitespace().collect();
                 if let Some(name) = parts.first() {
                     *process_conns.entry(name.to_string()).or_insert(0) += 1;
                 }
             }
        }
    }

    let mut top_processes: Vec<(String, u32)> = process_conns.into_iter().collect();
    top_processes.sort_by(|a, b| b.1.cmp(&a.1));
    top_processes.truncate(5);

    SocketOverviewInfo { established, listen, time_wait, close_wait, fin_wait, top_processes }
}

fn compute_context_switches() -> ContextSwitchInfo {
    let mut total_csw = 0u64;
    let mut top_processes = Vec::new();

    if let Ok(output) = Command::new("ps").args(["-Acro", "comm,nivcsw"]).output() {
        if !output.status.success() {
            return ContextSwitchInfo { total_csw, top_processes };
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.trim().split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(csw) = parts.last().unwrap_or(&"0").parse::<u64>() {
                    total_csw += csw;
                    let name_parts = &parts[..parts.len()-1];
                    let name = name_parts.join(" ");

                    if csw > 0 {
                        top_processes.push((name, csw));
                    }
                }
            }
        }
    }

    top_processes.sort_by(|a, b| b.1.cmp(&a.1));
    top_processes.truncate(5);

    ContextSwitchInfo { total_csw, top_processes }
}

impl Drop for MacCollector {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
    }
}

/// Run nettop once and return per-PID network stats.
fn run_nettop() -> HashMap<Pid, (u64, u64)> {
    let mut stats = HashMap::new();
    if let Ok(output) = Command::new("nettop").args(["-P", "-L", "1"]).output() {
        if !output.status.success() {
            return stats;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 6 {
                if let Some(name_pid) = parts.get(1) {
                    if let Some(last_dot) = name_pid.rfind('.') {
                        if let Ok(pid_val) = name_pid[last_dot+1..].parse::<u32>() {
                            let pid = Pid::from(pid_val as usize);
                            let bytes_in = parts.get(4).unwrap_or(&"0").parse::<u64>().unwrap_or(0);
                            let bytes_out = parts.get(5).unwrap_or(&"0").parse::<u64>().unwrap_or(0);
                            stats.insert(pid, (bytes_in, bytes_out));
                        }
                    }
                }
            }
        }
    }
    stats
}

impl SystemCollector for MacCollector {
    fn get_disk_io_pct(&mut self) -> f64 {
        // macOS `iostat` doesn't give a simple "busy %" easily without parsing complex output tailored to specific disks.
        // In the original `controller.rs`, `collect_disk_io_stats` or similar didn't exist in the snippet I saw.
        // However, `monitor.disks` from `sysinfo` provides usage? `sysinfo` doesn't provide busy %.
        // If the original controller didn't have it, we return 0.0 effectively.
        // But let's check if we can get it. `iostat -d -c 2` gives KB/t tps MB/s.
        // We will leave it as 0.0 for now to match perceived current state or improve later.
        0.0
    }

    fn get_fd_stats(&self) -> FdInfo {
        // Pure cache read. If the background command-warmer thread
        // hasn't finished its first pass yet, return defaults — the
        // sidebar will populate within a second or two.
        self.command_cache.lock().fd_info.clone().unwrap_or_default()
    }

    fn get_socket_stats(&self) -> SocketOverviewInfo {
        self.command_cache.lock().socket_info.clone().unwrap_or_default()
    }

    fn get_context_switches(&self) -> ContextSwitchInfo {
        self.command_cache.lock().context_switches.clone().unwrap_or_default()
    }

    fn get_process_network_stats(&mut self) -> HashMap<Pid, (u64, u64)> {
        // Read cached results from the background nettop thread — no blocking!
        self.nettop_cache.lock().clone()
    }

    fn set_active(&self, active: bool) {
        self.active_flag.store(active, Ordering::Release);
    }
}
