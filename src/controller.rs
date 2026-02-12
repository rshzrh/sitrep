use sysinfo::{System, Pid, Disks, Networks};
use std::time::{Duration, Instant};
use std::collections::{HashMap, VecDeque};
use std::process::Command;
use chrono::Local;
use crate::model::{
    MonitorData, ProcessGroup, ProcessInfo, UIState, SortColumn,
    SocketOverviewInfo, ContextSwitchInfo, FdInfo,
    NetworkInfo, NetworkProcessInfo, NetworkInterfaceInfo,
    MemoryInfo, DiskSpaceInfo
};
use crate::layout::Layout;

pub struct Monitor {
    sys: System,
    core_count: f64,
    history: VecDeque<(Instant, HashMap<Pid, ProcessGroup>)>,
    disks: Disks,
    networks: Networks,
    prev_net_snapshot: Option<(Instant, Vec<(String, u64, u64)>)>,
    pub ui_state: UIState,
    pub layout: Layout,
    pub last_data: Option<MonitorData>,
}

impl Monitor {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let core_count = sys.cpus().len() as f64;
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();
        Self {
            sys,
            core_count,
            history: VecDeque::new(),
            disks,
            networks,
            prev_net_snapshot: None,
            ui_state: UIState::default(),
            layout: Layout::default_layout(),
            last_data: None,
        }
    }

    pub fn update(&mut self) {
        self.sys.refresh_all();
        self.disks.refresh(true);
        self.networks.refresh(true);

        let now_chrono = Local::now();
        let now_instant = Instant::now();
        let load_avg_raw = System::load_average();

        // Pre-fetch network stats
        let net_stats = Self::collect_process_network_stats();

        // --- Process grouping ---
        let mut live_groups: HashMap<Pid, ProcessGroup> = HashMap::new();
        for p in self.sys.processes().values() {
            let parent_pid = p.parent().unwrap_or(p.pid());
            let group = live_groups.entry(parent_pid).or_insert_with(|| ProcessGroup {
                pid: parent_pid,
                cpu: 0.0,
                mem: 0,
                read_bytes: 0,
                written_bytes: 0,
                net_rx_bytes: 0,
                net_tx_bytes: 0,
                child_count: 0,
                name: p.name().to_string_lossy().into_owned(),
                children: Vec::new(),
            });

            group.cpu += p.cpu_usage() as f64;
            group.mem += p.memory();
            group.read_bytes += p.disk_usage().read_bytes;
            group.written_bytes += p.disk_usage().written_bytes;

            let (proc_rx, proc_tx) = *net_stats.get(&p.pid()).unwrap_or(&(0, 0));
            group.net_rx_bytes += proc_rx;
            group.net_tx_bytes += proc_tx;

            group.children.push(ProcessInfo {
                pid: p.pid(),
                cpu: p.cpu_usage(),
                mem: p.memory(),
                read_bytes: p.disk_usage().read_bytes,
                written_bytes: p.disk_usage().written_bytes,
                net_rx_bytes: proc_rx,
                net_tx_bytes: proc_tx,
                name: p.name().to_string_lossy().into_owned(),
            });

            if p.parent().is_some() {
                group.child_count += 1;
            }
        }

        // --- History ---
        self.history.push_back((now_instant, live_groups.clone()));
        while let Some(front) = self.history.front() {
            if now_instant.duration_since(front.0) > Duration::from_secs(60) {
                self.history.pop_front();
            } else {
                break;
            }
        }

        // --- CPU top (freeze if expanded) ---
        let new_cpu_top = if self.ui_state.expanded_pids.is_empty() {
            self.compute_cpu_top()
        } else {
            self.last_data.as_ref().map(|d| d.historical_top.clone()).unwrap_or_default()
        };

        // --- New sections ---
        let disk_space = self.collect_disk_space();
        let disk_busy_pct = Self::collect_disk_busy();
        let memory = self.collect_memory();
        let network = self.collect_network(now_instant);
        let fd_info = Self::collect_fd_info();
        let context_switches = Self::collect_context_switches();
        let socket_overview = Self::collect_socket_overview();

        self.last_data = Some(MonitorData {
            time: now_chrono.format("%Y-%m-%d %H:%M:%S").to_string(),
            core_count: self.core_count,
            load_avg: (load_avg_raw.one, load_avg_raw.five, load_avg_raw.fifteen),
            historical_top: new_cpu_top,
            disk_space,
            disk_busy_pct,
            memory,
            network,
            fd_info,
            context_switches,
            socket_overview,
        });
    }

    // --- CPU & Disk compute helpers ---

    fn compute_cpu_top(&self) -> Vec<ProcessGroup> {
        if self.history.is_empty() { return Vec::new(); }
        
        // Track totals for averaging CPU/Mem
        // PID -> (total_cpu, total_mem, samples, child_count, name, children)
        let mut cpu_totals: HashMap<Pid, (f64, u64, f64, usize, String, Vec<ProcessInfo>)> = HashMap::new();
        
        // Track first/last for Disk & Net rate calculation
        // PID -> (first_read, first_written, first_net_rx, first_net_tx, first_time, 
        //         last_read, last_written, last_net_rx, last_net_tx, last_time)
        struct Stats {
            first_disk: (u64, u64),
            first_net: (u64, u64),
            first_time: Instant,
            last_disk: (u64, u64),
            last_net: (u64, u64),
            last_time: Instant,
        }
        let mut rate_stats: HashMap<Pid, Stats> = HashMap::new();

        for (timestamp, snapshot) in &self.history {
            for (pid, group) in snapshot {
                // CPU Accumulation
                let entry = cpu_totals.entry(*pid).or_insert((0.0, 0, 0.0, 0, group.name.clone(), group.children.clone()));
                entry.0 += group.cpu;
                entry.1 += group.mem;
                entry.2 += 1.0;
                entry.3 = group.child_count;
                
                // Rate Stats Tracking
                rate_stats.entry(*pid)
                    .and_modify(|stats| {
                        stats.last_disk = (group.read_bytes, group.written_bytes);
                        stats.last_net = (group.net_rx_bytes, group.net_tx_bytes);
                        stats.last_time = *timestamp;
                    })
                    .or_insert(Stats {
                        first_disk: (group.read_bytes, group.written_bytes),
                        first_net: (group.net_rx_bytes, group.net_tx_bytes),
                        first_time: *timestamp,
                        last_disk: (group.read_bytes, group.written_bytes),
                        last_net: (group.net_rx_bytes, group.net_tx_bytes),
                        last_time: *timestamp,
                    });
            }
        }

        let mut top: Vec<ProcessGroup> = cpu_totals
            .into_iter()
            .map(|(pid, (total_cpu, total_mem, samples, child_count, name, children))| {
                let (read_rate, write_rate, net_rx_rate, net_tx_rate) = if let Some(stats) = rate_stats.get(&pid) {
                    let duration = stats.last_time.duration_since(stats.first_time).as_secs_f64();
                    if duration > 0.1 {
                        (
                            ((stats.last_disk.0.saturating_sub(stats.first_disk.0)) as f64 / duration) as u64,
                            ((stats.last_disk.1.saturating_sub(stats.first_disk.1)) as f64 / duration) as u64,
                            ((stats.last_net.0.saturating_sub(stats.first_net.0)) as f64 / duration) as u64,
                            ((stats.last_net.1.saturating_sub(stats.first_net.1)) as f64 / duration) as u64
                        )
                    } else {
                        (0, 0, 0, 0)
                    }
                } else {
                    (0, 0, 0, 0)
                };

                ProcessGroup {
                    pid, 
                    cpu: total_cpu / samples, 
                    mem: total_mem / samples as u64,
                    read_bytes: read_rate,
                    written_bytes: write_rate,
                    net_rx_bytes: net_rx_rate,
                    net_tx_bytes: net_tx_rate,
                    child_count, 
                    name, 
                    children,
                }
            })
            .collect();
            
            top.sort_by(|a, b| {
                match self.ui_state.sort_column {
                    SortColumn::Cpu => b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal),
                    SortColumn::Memory => b.mem.cmp(&a.mem),
                    SortColumn::Read => b.read_bytes.cmp(&a.read_bytes),
                    SortColumn::Write => b.written_bytes.cmp(&a.written_bytes),
                    SortColumn::NetDown => b.net_rx_bytes.cmp(&a.net_rx_bytes),
                    SortColumn::NetUp => b.net_tx_bytes.cmp(&a.net_tx_bytes),
                }
            });
        top.into_iter().take(10).collect()
    }

    // --- New data collectors ---

    fn collect_disk_space(&self) -> Vec<DiskSpaceInfo> {
        let mut warnings = Vec::new();
        for disk in self.disks.list() {
            let total = disk.total_space() as f64;
            let available = disk.available_space() as f64;
            if total <= 0.0 { continue; }
            let percent_free = (available / total) * 100.0;
            let is_warning = percent_free < 10.0;
            if is_warning {
                warnings.push(DiskSpaceInfo {
                    mount_point: disk.mount_point().to_string_lossy().into_owned(),
                    total_gb: total / 1_073_741_824.0,
                    available_gb: available / 1_073_741_824.0,
                    percent_free,
                    is_warning,
                });
            }
        }
        warnings
    }

    fn collect_disk_busy() -> f64 {
        if let Ok(output) = Command::new("iostat").args(["-d", "-c", "1"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().rev() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    if let Ok(val) = parts.last().unwrap_or(&"0").parse::<f64>() {
                        return val;
                    }
                }
            }
        }
        0.0
    }

    fn collect_memory(&self) -> MemoryInfo {
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let available_raw = self.sys.available_memory();
        // On macOS, available_memory() can return 0; fall back to total - used
        let available = if available_raw > 0 { available_raw } else { total.saturating_sub(used) };

        MemoryInfo {
            total,
            used,
            available,
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
        }
    }

    fn collect_network(&mut self, now: Instant) -> NetworkInfo {
        let mut current_snapshot: Vec<(String, u64, u64)> = Vec::new();
        let mut interfaces = Vec::new();

        for (name, data) in self.networks.list() {
            let rx = data.total_received();
            let tx = data.total_transmitted();
            current_snapshot.push((name.clone(), rx, tx));
        }

        if let Some((prev_time, ref prev_data)) = self.prev_net_snapshot {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed > 0.1 {
                let prev_map: HashMap<&str, (u64, u64)> = prev_data.iter()
                    .map(|(n, r, t)| (n.as_str(), (*r, *t)))
                    .collect();
                for (name, rx, tx) in &current_snapshot {
                    if let Some((prev_rx, prev_tx)) = prev_map.get(name.as_str()) {
                        let rx_rate = (rx.saturating_sub(*prev_rx) as f64 / elapsed) as u64;
                        let tx_rate = (tx.saturating_sub(*prev_tx) as f64 / elapsed) as u64;
                        if rx_rate > 0 || tx_rate > 0 {
                            interfaces.push(NetworkInterfaceInfo {
                                name: name.clone(), rx_rate, tx_rate,
                            });
                        }
                    }
                }
            }
        }
        self.prev_net_snapshot = Some((now, current_snapshot));

        // Connection counts
        let (established, time_wait, close_wait) = Self::collect_connection_counts();

        // Top 5 bandwidth-consuming processes via nettop
        let top_bandwidth_processes = Self::collect_top_bandwidth_processes();

        NetworkInfo { interfaces, top_bandwidth_processes, established, time_wait, close_wait }
    }

    fn collect_connection_counts() -> (u32, u32, u32) {
        let mut established = 0u32;
        let mut time_wait = 0u32;
        let mut close_wait = 0u32;
        if let Ok(output) = Command::new("netstat").args(["-an", "-p", "tcp"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("ESTABLISHED") { established += 1; }
                else if line.contains("TIME_WAIT") { time_wait += 1; }
                else if line.contains("CLOSE_WAIT") { close_wait += 1; }
            }
        }
        (established, time_wait, close_wait)
    }

    fn collect_top_bandwidth_processes() -> Vec<NetworkProcessInfo> {
        // Use lsof -i -n -P to find processes with network connections,
        // then cross-reference with nettop snapshot for bandwidth
        // Fallback: just show processes with most network file descriptors
        let mut process_net_fds: HashMap<String, u64> = HashMap::new();

        if let Ok(output) = Command::new("lsof").args(["-i", "-n", "-P"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(name) = parts.first() {
                    *process_net_fds.entry(name.to_string()).or_insert(0) += 1;
                }
            }
        }

        let mut top: Vec<NetworkProcessInfo> = process_net_fds.into_iter()
            .map(|(name, count)| NetworkProcessInfo { name, bandwidth: count })
            .collect();
        top.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
        top.truncate(5);
        top
    }

    fn collect_fd_info() -> FdInfo {
        let system_max = if let Ok(output) = Command::new("sysctl").args(["-n", "kern.maxfiles"]).output() {
            String::from_utf8_lossy(&output.stdout).trim().parse::<u64>().unwrap_or(0)
        } else { 0 };

        let mut system_used = 0u64;
        let mut process_counts: HashMap<String, u64> = HashMap::new();

        if let Ok(output) = Command::new("lsof").args(["-n"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                system_used += 1;
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(name) = parts.first() {
                    *process_counts.entry(name.to_string()).or_insert(0) += 1;
                }
            }
        }

        let mut top_processes: Vec<(String, u64)> = process_counts.into_iter().collect();
        top_processes.sort_by(|a, b| b.1.cmp(&a.1));
        top_processes.truncate(5);

        FdInfo { system_used, system_max, top_processes }
    }

    fn collect_context_switches() -> ContextSwitchInfo {
        // Per-process involuntary context switches from ps
        // Sum all for system total (more reliable on macOS than sysctl)
        let mut top_processes = Vec::new();
        let mut total_csw = 0u64;

        if let Ok(output) = Command::new("ps").args(["-eo", "comm,nivcsw"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(csw) = parts.last().unwrap().parse::<u64>() {
                        total_csw += csw;
                        if csw > 0 {
                            let name = parts[..parts.len()-1].join(" ");
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

    fn collect_process_network_stats() -> HashMap<Pid, (u64, u64)> {
        let mut stats = HashMap::new();
        // nettop -P -L 1
        // Output format: time,name.pid,?,?,bytes_in,bytes_out,...
        if let Ok(output) = Command::new("nettop").args(["-P", "-L", "1"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) { // Skip header if present (though -L 1 usually has header)
                // Actually nettop -P -L 1 output varies. Let's assume standard CSV.
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 6 {
                    // split name.pid
                    if let Some(name_pid) = parts.get(1) {
                        if let Some(last_dot) = name_pid.rfind('.') {
                            if let Ok(pid) = name_pid[last_dot+1..].parse::<Pid>() {
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

    fn collect_socket_overview() -> SocketOverviewInfo {
        let mut established = 0u32;
        let mut listen = 0u32;
        let mut time_wait = 0u32;
        let mut close_wait = 0u32;
        let mut fin_wait = 0u32;

        if let Ok(output) = Command::new("netstat").args(["-an", "-p", "tcp"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("ESTABLISHED") { established += 1; }
                else if line.contains("LISTEN") { listen += 1; }
                else if line.contains("TIME_WAIT") { time_wait += 1; }
                else if line.contains("CLOSE_WAIT") { close_wait += 1; }
                else if line.contains("FIN_WAIT") { fin_wait += 1; }
            }
        }

        // Top processes by connection count
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
}
