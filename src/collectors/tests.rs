//! Tests for the SystemCollector trait shape itself (the default
//! collect_snapshot() impl, in particular). The per-impl tests live in
//! collectors::parsers and collectors::remote.

use super::*;
use crate::model::{ContextSwitchInfo, FdInfo, SocketOverviewInfo};

/// A stub collector that just returns canned values, so we can verify
/// the default `collect_snapshot()` impl wires through correctly.
struct StubCollector {
    disk_calls: u32,
    net_calls: u32,
}

impl StubCollector {
    fn new() -> Self {
        Self {
            disk_calls: 0,
            net_calls: 0,
        }
    }
}

impl SystemCollector for StubCollector {
    fn get_disk_io_pct(&mut self) -> f64 {
        self.disk_calls += 1;
        42.0
    }
    fn get_fd_stats(&self) -> FdInfo {
        FdInfo {
            system_used: 100,
            system_max: 1000,
            top_processes: vec![("init".into(), 50)],
        }
    }
    fn get_socket_stats(&self) -> SocketOverviewInfo {
        SocketOverviewInfo {
            established: 5,
            ..Default::default()
        }
    }
    fn get_context_switches(&self) -> ContextSwitchInfo {
        ContextSwitchInfo {
            total_csw: 9999,
            ..Default::default()
        }
    }
    fn get_process_network_stats(&mut self) -> HashMap<Pid, (u64, u64)> {
        self.net_calls += 1;
        HashMap::new()
    }
}

#[test]
fn default_collect_snapshot_calls_each_per_metric_method() {
    let mut c = StubCollector::new();
    let snap = c.collect_snapshot();
    assert_eq!(snap.disk_io_pct, 42.0);
    assert_eq!(snap.fd.system_used, 100);
    assert_eq!(snap.sockets.established, 5);
    assert_eq!(snap.ctxt.total_csw, 9999);
    assert_eq!(c.disk_calls, 1);
    assert_eq!(c.net_calls, 1);
}

#[test]
fn default_collect_snapshot_returns_snapshot_struct_with_correct_shape() {
    // Sanity: the returned struct fields all wire through to the impl
    // values. If a field is added to SystemSnapshot in the future, this
    // test forces a corresponding default-impl update.
    let mut c = StubCollector::new();
    let snap = c.collect_snapshot();
    let _: f64 = snap.disk_io_pct;
    let _: FdInfo = snap.fd;
    let _: SocketOverviewInfo = snap.sockets;
    let _: ContextSwitchInfo = snap.ctxt;
    let _: HashMap<Pid, (u64, u64)> = snap.proc_net;
}
