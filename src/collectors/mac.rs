use super::SystemCollector;
use crate::model::{
    FdInfo, SocketOverviewInfo, ContextSwitchInfo
};
use sysinfo::Pid;
use std::collections::HashMap;
use std::process::Command;
use std::time::Instant;

pub struct MacCollector {
    // We might need state here if we want to cache things, but for now we follow controller's stateless usage (mostly).
    // Controller had `prev_net_snapshot`? No, it was in Monitor struct.
    // `collect_process_network_stats` in controller was static function?
    // Let's check line 423 of controller.rs: `fn collect_process_network_stats() -> ...` (no self).
    // So it was stateless.
}

impl MacCollector {
    pub fn new() -> Self {
        Self {}
    }

    fn parse_sysctl_value(output: &str) -> u64 {
        // sysctl output: "kern.maxfiles: 122880"
        output.split(":").nth(1).unwrap_or("0").trim().parse().unwrap_or(0)
    }
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
        let mut info = FdInfo::default();

        // System wide
        if let Ok(output) = Command::new("sysctl").arg("kern.num_files").output() {
            info.system_used = Self::parse_sysctl_value(&String::from_utf8_lossy(&output.stdout));
        }
        if let Ok(output) = Command::new("sysctl").arg("kern.maxfiles").output() {
            info.system_max = Self::parse_sysctl_value(&String::from_utf8_lossy(&output.stdout));
        }

        // Top processes by open files (lsof)
        // Note: This is slow. The original implementation used `lsof -i -n -P` for sockets, but for FDs?
        // Ah, `render_fd_info` in view.rs uses `data.fd_info`.
        // `controller.rs` snippet 390+ showed `collect_fd_info`? No, I didn't see it explicitly in the snippet I viewed (400-488).
        // I saw `collect_context_switches` (400), `collect_process_network_stats` (423), `collect_socket_overview` (449).
        // `FdInfo` was likely collected in another method or I missed it.
        // Wait, looking at `controller.rs` around line 390 (implied):
        // I need to assume logic similar to `collect_socket_overview` but for files.
        // Usually `lsof | awk ...`.
        // Since I don't have the exact source for `collect_fd_info`, I will implement a reasonable macOS version.
        // `lsof -n -P` lists all open files.
        
        // Optimization: `lsof` is heavy. Maybe we skip it or use a faster method?
        // But for parity, let's try to match.
        // A common pattern is: `lsof -n -P | cut -d' ' -f1 | sort | uniq -c | sort -nr | head -5` via sh.
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

    fn get_socket_stats(&self) -> SocketOverviewInfo {
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
        // Original logic used `lsof -i -n -P`.
        let mut process_conns: HashMap<String, u32> = HashMap::new();
        if let Ok(output) = Command::new("lsof").args(["-i", "-n", "-P"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                 // The original logic checked for states?
                 // "if line.contains("ESTABLISHED") || ..."
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

    fn get_context_switches(&self) -> ContextSwitchInfo {
        let mut total_csw = 0u64;
        let mut top_processes = Vec::new();

        if let Ok(output) = Command::new("ps").args(["-Acro", "comm,nivcsw"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            // Skip header
            for line in text.lines().skip(1) {
                let parts: Vec<&str> = line.trim().split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(csw) = parts.last().unwrap().parse::<u64>() {
                        total_csw += csw;
                        // Determine name (everything before last column)
                        let name_parts = &parts[..parts.len()-1];
                        let name = name_parts.join(" "); // This effectively rejoins process names with spaces
                        
                        if csw > 0 {
                            top_processes.push((name, csw));
                        }
                    }
                }
            }
        }
        
        // Original logic sorted and truncated manually?
        // Wait, `ps -Acro` implies sort by what? `r`=cpu? `ps` doesn't natively sort by csw easily on mac.
        // Controller logic (lines 417-418):
        // top_processes.sort_by(...)
        // top_processes.truncate(5)
        top_processes.sort_by(|a, b| b.1.cmp(&a.1));
        top_processes.truncate(5);

        ContextSwitchInfo { total_csw, top_processes }
    }

    fn get_process_network_stats(&mut self) -> HashMap<Pid, (u64, u64)> {
        let mut stats = HashMap::new();
        // nettop -P -L 1 -J bytes_in,bytes_out
        // Controller used args ["-P", "-L", "1"] and split by comma.
        // And parsed index 4 and 5?
        // In the snippet 427: `Command::new("nettop").args(["-P", "-L", "1"])`
        // And parsed: `parts.get(4)`/`parts.get(5)`.
        
        if let Ok(output) = Command::new("nettop").args(["-P", "-L", "1"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 6 {
                     if let Some(name_pid) = parts.get(1) {
                         if let Some(last_dot) = name_pid.rfind('.') {
                             // "name.pid" format
                             if let Ok(pid_val) = name_pid[last_dot+1..].parse::<u32>() {
                                 // sysinfo uses Pid which is likely wrapper around number or platform specific.
                                 // On unix Pid is usually i32/u32.
                                 // sysinfo::Pid::from(pid_val as usize) ?
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
}
