//! System data collection and processing.

mod process;

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use chrono::Local;
use sysinfo::{Pid, System, Disks, Networks};

use crate::collectors::{mac::MacCollector, linux::LinuxCollector, SystemCollector};
use crate::layout::Layout;
use crate::model::{
    DiskSpaceInfo, MemoryInfo, MonitorData, NetworkInfo, NetworkInterfaceInfo,
    ProcessGroup, UIState,
};

pub struct Monitor {
    pub ui_state: UIState,
    pub layout: Layout,
    pub last_data: Option<MonitorData>,
    worker_state: Option<MonitorWorkerState>,
    update_receiver: Option<mpsc::Receiver<MonitorUpdateResult>>,
}

struct MonitorWorkerState {
    sys: System,
    core_count: f64,
    history: VecDeque<(Instant, HashMap<Pid, ProcessGroup>)>,
    disks: Disks,
    networks: Networks,
    prev_net_snapshot: Option<(Instant, Vec<(String, u64, u64)>)>,
    collector: Box<dyn SystemCollector>,
}

struct MonitorUpdateResult {
    worker_state: MonitorWorkerState,
    data: MonitorData,
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
            ui_state: UIState::default(),
            layout: Layout::default_layout(),
            last_data: None,
            worker_state: Some(MonitorWorkerState {
                sys,
                core_count,
                history: VecDeque::new(),
                disks,
                networks,
                prev_net_snapshot: None,
                collector,
            }),
            update_receiver: None,
        }
    }

    pub fn update(&mut self) {
        if self.update_receiver.is_some() {
            return;
        }

        let Some(mut worker_state) = self.worker_state.take() else {
            return;
        };
        let sort_column = self.ui_state.sort_column;
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let data = worker_state.collect_snapshot(sort_column);
            let _ = tx.send(MonitorUpdateResult { worker_state, data });
        });

        self.update_receiver = Some(rx);
    }

    pub fn poll_update(&mut self) -> bool {
        let Some(ref rx) = self.update_receiver else {
            return false;
        };

        match rx.try_recv() {
            Ok(result) => {
                self.worker_state = Some(result.worker_state);
                self.last_data = Some(result.data);
                self.update_receiver = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.update_receiver = None;
                false
            }
        }
    }
}

impl MonitorWorkerState {
    fn collect_snapshot(&mut self, sort_column: crate::model::SortColumn) -> MonitorData {
        self.sys.refresh_all();
        self.disks.refresh(true);
        self.networks.refresh(true);

        let now_chrono = Local::now();
        let now_instant = Instant::now();
        let load_avg_raw = System::load_average();

        let net_stats = self.collector.get_process_network_stats();
        let fd_info = self.collector.get_fd_stats();
        let socket_info = self.collector.get_socket_stats();
        let csw_info = self.collector.get_context_switches();
        let disk_busy = self.collector.get_disk_io_pct();

        let live_groups = process::build_live_groups(&self.sys, &net_stats);

        self.history.push_back((now_instant, live_groups));
        if self.history.len() > 20 {
            self.history.pop_front();
        }

        let historical_top = process::compute_top_processes(&self.history, sort_column);

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
            let percent_free = if total > 0.0 {
                (available / total) * 100.0
            } else {
                0.0
            };

            disk_space.push(DiskSpaceInfo {
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                total_gb: total,
                available_gb: available,
                percent_free,
                is_warning: percent_free < 10.0,
            });
        }

        let mut interfaces = Vec::new();
        let mut current_interfaces = Vec::new();
        for (name, data) in &self.networks {
            current_interfaces.push((name.clone(), data.received(), data.transmitted()));
        }

        if let Some((prev_time, prev_data)) = &self.prev_net_snapshot {
            let duration = now_instant.duration_since(*prev_time).as_secs_f64();
            if duration > 0.0 {
                let prev_map: HashMap<&str, (u64, u64)> = prev_data
                    .iter()
                    .map(|(n, r, t)| (n.as_str(), (*r, *t)))
                    .collect();
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

        MonitorData {
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
        }
    }
}
