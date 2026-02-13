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
use crate::collectors::{SystemCollector, mac::MacCollector, linux::LinuxCollector};

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
    collector: Box<dyn SystemCollector>,
}

impl Monitor {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let core_count = sys.cpus().len() as f64;
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();

        let collector: Box<dyn SystemCollector> = if cfg!(target_os = "macos") {
            Box::new(MacCollector::new())
        } else {
            Box::new(LinuxCollector::new())
        };

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
            collector,
        }
    }

    pub fn update(&mut self) {
        self.sys.refresh_all();
        self.disks.refresh(true);
        self.networks.refresh(true);

        let now_chrono = Local::now();
        let now_instant = Instant::now();
        let load_avg_raw = System::load_average();

        // Collect OS-specific data
        let net_stats = self.collector.get_process_network_stats();
        let fd_info = self.collector.get_fd_stats();
        let socket_info = self.collector.get_socket_stats();
        let csw_info = self.collector.get_context_switches();
        let disk_busy = self.collector.get_disk_io_pct();

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
        self.history.push_back((now_instant, live_groups));
        if self.history.len() > 20 { // 60 seconds (3s interval) -> 20 snapshots
            self.history.pop_front();
        }

        let historical_top = self.compute_top_processes();

        // --- Other Metrics ---
        let memory = MemoryInfo {
            total: self.sys.total_memory(),
            used: self.sys.used_memory(),
            available: self.sys.available_memory(),
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
        };

        let mut disk_space = Vec::new();
        for disk in &self.disks {
            let total = disk.total_space() as f64 / 1_000_000_000.0;
            let available = disk.available_space() as f64 / 1_000_000_000.0;
            let percent_free = if total > 0.0 { (available / total) * 100.0 } else { 0.0 };
            
            disk_space.push(DiskSpaceInfo {
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                total_gb: total,
                available_gb: available,
                percent_free,
                is_warning: percent_free < 10.0,
            });
        }

        // Network Interfaces
        let mut interfaces = Vec::new();
        let mut current_interfaces = Vec::new();
        for (name, data) in &self.networks {
             current_interfaces.push((name.clone(), data.received(), data.transmitted()));
        }
        
        if let Some((prev_time, prev_data)) = &self.prev_net_snapshot {
             let duration = now_instant.duration_since(*prev_time).as_secs_f64();
             if duration > 0.0 {
                 let prev_map: HashMap<&str, (u64, u64)> = prev_data.iter().map(|(n, r, t)| (n.as_str(), (*r, *t))).collect();
                 for (name, curr_rx, curr_tx) in &current_interfaces {
                     if let Some((prev_rx, prev_tx)) = prev_map.get(name.as_str()) {
                         let rx_diff = curr_rx.saturating_sub(*prev_rx);
                         let tx_diff = curr_tx.saturating_sub(*prev_tx);
                         let rx_rate = (rx_diff as f64 / duration) as u64;
                         let tx_rate = (tx_diff as f64 / duration) as u64;
                         
                         if rx_rate > 0 || tx_rate > 0 {
                             interfaces.push(NetworkInterfaceInfo {
                                 name: name.clone(),
                                 rx_rate,
                                 tx_rate,
                             });
                         }
                     }
                 }
             }
        }
        self.prev_net_snapshot = Some((now_instant, current_interfaces));

        let network_info = NetworkInfo {
            interfaces,
            top_bandwidth_processes: Vec::new(), 
            established: socket_info.established,
            time_wait: socket_info.time_wait,
            close_wait: socket_info.close_wait,
        };
        
        self.last_data = Some(MonitorData {
            time: now_chrono.format("%H:%M:%S").to_string(),
            core_count: self.core_count,
            load_avg: (load_avg_raw.one, load_avg_raw.five, load_avg_raw.fifteen),
            historical_top,
            disk_space,
            disk_busy_pct: disk_busy,
            memory,
            network: network_info,
            fd_info,
            context_switches: csw_info,
            socket_overview: socket_info,
        });
    }

    fn compute_top_processes(&self) -> Vec<ProcessGroup> {
        let mut avg_groups: HashMap<Pid, ProcessGroup> = HashMap::new();
        let snapshots = self.history.len();
        if snapshots == 0 { return Vec::new(); }

        for (_, groups) in &self.history {
            for (pid, g) in groups {
                let entry = avg_groups.entry(*pid).or_insert_with(|| {
                    let mut clone = g.clone();
                    clone.cpu = 0.0;
                    clone.read_bytes = 0;
                    clone.written_bytes = 0;
                    clone.net_rx_bytes = 0;
                    clone.net_tx_bytes = 0;
                    clone
                });
                entry.cpu += g.cpu;
                entry.read_bytes += g.read_bytes;
                entry.written_bytes += g.written_bytes;
                entry.net_rx_bytes += g.net_rx_bytes;
                entry.net_tx_bytes += g.net_tx_bytes;
            }
        }

        let mut top: Vec<ProcessGroup> = avg_groups.into_values().map(|mut g| {
            g.cpu /= snapshots as f64;
            // Rates: sum / snapshots gives avg bytes per interval (3s).
            // Divide by 3.0 to get B/s.
            let interval_secs = 3.0; // Approximation
            g.read_bytes = (g.read_bytes as f64 / snapshots as f64 / interval_secs) as u64;
            g.written_bytes = (g.written_bytes as f64 / snapshots as f64 / interval_secs) as u64;
            
            // Network rates? `nettop` (from collector) might already be B/s?
            // collector uses `nettop -L 1 -J bytes_in,bytes_out`.
            // Does nettop return rate or total?
            // "bytes_in" usually implies total bytes? Or bytes in interval?
            // If it is bytes in interval (1s due to -L 1?), then it is B/s.
            // MacCollector implementation line 133: `Command::new("nettop").args(["-P", "-L", "1"])`.
            // logic returns `bytes_in`.
            // `nettop` in "polling mode" usually reports delta?
            // If so, `nettop -L 1` reports delta over 1s?
            // If it is B/s, then we just average it (divide by snapshots).
            // We don't divide by 3.0 unless it was accumulated over 3s.
            // Since nettop command runs instantaneously (wait, it blocks for 1s?), `update` takes at least 1s.
            // `Monitor::update` is called every 3s.
            // So we sample nettop every 3s.
            // If `nettop` returns B/s, then we just average.
            g.net_rx_bytes /= snapshots as u64;
            g.net_tx_bytes /= snapshots as u64;
            g
        }).collect();

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
}
