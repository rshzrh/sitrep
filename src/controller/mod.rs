//! System data collection and processing.

mod process;

/// Shared lifecycle contract implemented by every data-producing
/// monitor: `Monitor` (system metrics), `DockerMonitor` (containers),
/// `SwarmMonitor` (swarm cluster). A narrow trait by design — it only
/// covers the calls the event loop makes during a tick (`update`,
/// `poll_update`, `is_available`, `set_active`). Each monitor still
/// exposes its own typed accessors for render and input handlers.
///
/// Adding a 4th monitor (Kubernetes, systemd, etc.) means: implement
/// this trait + add a `TabKind` variant + wire it into the `App`
/// dispatch helper. See A1 in the eng review.
pub trait DataMonitor {
    /// Kick a background refresh. Idempotent — calling while an
    /// update is already in flight is a no-op.
    fn update(&mut self);

    /// Drain any completed background update. Returns `true` if
    /// visible state changed (i.e. a render is needed).
    fn poll_update(&mut self) -> bool;

    /// Whether the monitor's data source is reachable and its tab
    /// should be shown/refreshed. Default: always available (for
    /// monitors with no initialization failure mode).
    fn is_available(&self) -> bool {
        true
    }

    /// Tell the monitor whether its tab is currently the active
    /// (user-visible) tab. Monitors with expensive background work
    /// (e.g. the macOS `nettop` loop on `Monitor`) use this to pause
    /// that work when the user has switched away. Default: no-op.
    fn set_active(&self, _active: bool) {}
}

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Shared flag telling the collector whether the System tab is the
    /// active (user-visible) tab. The MacCollector's background nettop
    /// loop reads this and pauses its subprocess spawn loop when the
    /// user has switched away. Held as an Arc so it stays reachable
    /// even while `worker_state` is moved into the background update
    /// thread.
    active_flag: Arc<AtomicBool>,
}

struct MonitorWorkerState {
    sys: System,
    core_count: f64,
    history: VecDeque<(Instant, HashMap<Pid, ProcessGroup>)>,
    disks: Disks,
    networks: Networks,
    prev_net_snapshot: Option<(Instant, Vec<(String, u64, u64)>)>,
    collector: Box<dyn SystemCollector>,
    /// `false` until the first `collect_snapshot` has run. The very
    /// first snapshot skips `sys.refresh_all()` / `disks.refresh()` /
    /// `networks.refresh()` because `Monitor::new` already did a full
    /// `System::new_all()` a few hundred ms earlier — refreshing again
    /// immediately is pure wasted work and shows up as first-frame
    /// latency.
    warmed_up: bool,
}

struct MonitorUpdateResult {
    worker_state: MonitorWorkerState,
    data: MonitorData,
}

impl Monitor {
    pub fn new() -> Self {
        // `System::new_all()` is `System::new() + refresh_all()` in one
        // shot — no need to call `refresh_all()` after it. Dropping the
        // redundant refresh saves 100–500ms of cold-start latency.
        let sys = System::new_all();
        let core_count = sys.cpus().len() as f64;
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();

        // Start active — System is the default tab on launch.
        let active_flag = Arc::new(AtomicBool::new(true));

        let collector: Box<dyn SystemCollector> = if cfg!(target_os = "macos") {
            Box::new(MacCollector::new_with_active_flag(Arc::clone(&active_flag)))
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
                warmed_up: false,
            }),
            update_receiver: None,
            active_flag,
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

    /// Tell the collector whether the System tab is active. Collectors
    /// with expensive background work (macOS `nettop`) use this to pause
    /// that work while the user is looking at another tab. The flag is
    /// an `Arc<AtomicBool>` shared with the collector, so this call
    /// lands even while a background update is in flight.
    pub fn set_active(&self, active: bool) {
        self.active_flag.store(active, Ordering::Release);
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

impl DataMonitor for Monitor {
    fn update(&mut self) {
        Monitor::update(self);
    }

    fn poll_update(&mut self) -> bool {
        Monitor::poll_update(self)
    }

    fn set_active(&self, active: bool) {
        Monitor::set_active(self, active);
    }
    // is_available: uses trait default — the local system monitor
    // has no "unavailable" state; sysinfo always works.
}

impl MonitorWorkerState {
    fn collect_snapshot(&mut self, sort_column: crate::model::SortColumn) -> MonitorData {
        // First tick skips all three refreshes — `Monitor::new` did a
        // full `System::new_all()` plus disk/network refresh only
        // milliseconds ago. Refreshing again now just pays the cost
        // twice and gates the first frame on it. Subsequent ticks
        // refresh normally.
        if self.warmed_up {
            self.sys.refresh_all();
            self.disks.refresh(true);
            self.networks.refresh(true);
        } else {
            self.warmed_up = true;
        }

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
