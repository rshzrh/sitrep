//! Process grouping and historical top-process computation.

use std::collections::HashMap;
use std::collections::VecDeque;

use sysinfo::{Pid, System};

use crate::model::{ProcessGroup, ProcessInfo, SortColumn};

/// Build process groups from the current system snapshot.
pub fn build_live_groups(
    sys: &System,
    net_stats: &HashMap<Pid, (u64, u64)>,
) -> HashMap<Pid, ProcessGroup> {
    let mut live_groups: HashMap<Pid, ProcessGroup> = HashMap::new();

    for p in sys.processes().values() {
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

    live_groups
}

/// Compute top processes from history, averaged and sorted by the given column.
pub fn compute_top_processes(
    history: &VecDeque<(std::time::Instant, HashMap<Pid, ProcessGroup>)>,
    sort_column: SortColumn,
) -> Vec<ProcessGroup> {
    let mut avg_groups: HashMap<Pid, ProcessGroup> = HashMap::new();
    let snapshots = history.len();
    if snapshots == 0 {
        return Vec::new();
    }

    for (_, groups) in history {
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

    let mut top: Vec<ProcessGroup> = avg_groups
        .into_values()
        .map(|mut g| {
            g.cpu /= snapshots as f64;
            let interval_secs = 3.0;
            g.read_bytes = (g.read_bytes as f64 / snapshots as f64 / interval_secs) as u64;
            g.written_bytes = (g.written_bytes as f64 / snapshots as f64 / interval_secs) as u64;
            g.net_rx_bytes /= snapshots as u64;
            g.net_tx_bytes /= snapshots as u64;
            g
        })
        .collect();

    top.sort_by(|a, b| {
        match sort_column {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sysinfo::Pid;

    use crate::model::{ProcessGroup, SortColumn};

    use super::compute_top_processes;

    #[test]
    fn compute_top_processes_empty_history() {
        let history = std::collections::VecDeque::new();
        let result = compute_top_processes(&history, SortColumn::Cpu);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_top_processes_single_snapshot() {
        let mut history = std::collections::VecDeque::new();
        let mut groups = HashMap::new();
        groups.insert(
            Pid::from(1usize),
            ProcessGroup {
                pid: Pid::from(1usize),
                cpu: 50.0,
                mem: 1000,
                read_bytes: 100,
                written_bytes: 200,
                net_rx_bytes: 10,
                net_tx_bytes: 20,
                child_count: 0,
                name: "test".into(),
                children: vec![],
            },
        );
        history.push_back((std::time::Instant::now(), groups));

        let result = compute_top_processes(&history, SortColumn::Cpu);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "test");
        assert!((result[0].cpu - 50.0).abs() < 0.01);
    }

    #[test]
    fn compute_top_processes_sorts_by_column() {
        let mut history = std::collections::VecDeque::new();
        let mut groups = HashMap::new();
        for (pid, cpu) in [(1u32, 10.0), (2, 50.0), (3, 30.0)] {
            groups.insert(
                Pid::from(pid as usize),
                ProcessGroup {
                    pid: Pid::from(pid as usize),
                    cpu,
                    mem: 0,
                    read_bytes: 0,
                    written_bytes: 0,
                    net_rx_bytes: 0,
                    net_tx_bytes: 0,
                    child_count: 0,
                    name: format!("p{}", pid),
                    children: vec![],
                },
            );
        }
        history.push_back((std::time::Instant::now(), groups));

        let result = compute_top_processes(&history, SortColumn::Cpu);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "p2");
        assert_eq!(result[1].name, "p3");
        assert_eq!(result[2].name, "p1");
    }
}
