//! Pure parsers for /proc files and common Linux command outputs.
//!
//! Every function in this module takes a `&str` (the file contents) and
//! returns a parsed value. Nothing here touches the filesystem or runs a
//! command. That makes the parsers trivially testable with fixture strings,
//! and — critically — makes them reusable for the multi-host wedge: the
//! same parsers run against locally-read /proc files AND against
//! SSH-fetched output from a remote host.
//!
//! Coverage map:
//!   parse_loadavg          → /proc/loadavg                (FleetVitals)
//!   parse_meminfo          → /proc/meminfo                (FleetVitals)
//!   parse_diskstats        → /proc/diskstats              (busy %)
//!   parse_net_dev          → /proc/net/dev                (bandwidth)
//!   parse_tcp_table        → /proc/net/tcp + /proc/net/tcp6
//!   parse_file_nr          → /proc/sys/fs/file-nr
//!   parse_proc_stat_ctxt   → /proc/stat (ctxt line)
//!   parse_proc_status_ctxt → /proc/<pid>/status
//!   parse_ss_summary       → `ss -s` (remote-only socket overview)
//!   parse_ps_aux           → `ps aux --sort=-%cpu` (remote-only top procs)

use std::collections::HashMap;

// ─── /proc/loadavg ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// Parse `/proc/loadavg` content. Format:
/// `0.42 0.31 0.28 1/234 5678`
/// Returns `None` if the input is empty or unparseable.
pub fn parse_loadavg(content: &str) -> Option<LoadAvg> {
    let line = content.lines().next()?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    Some(LoadAvg {
        one: parts[0].parse().ok()?,
        five: parts[1].parse().ok()?,
        fifteen: parts[2].parse().ok()?,
    })
}

// ─── /proc/meminfo ────────────────────────────────────────────────────────

/// Parsed memory totals. All values are in **bytes** (we convert from
/// the kB units used in /proc/meminfo).
///
/// `swap_total` and `swap_used` may be 0 on kernels with swap disabled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParsedMemInfo {
    pub total: u64,
    pub free: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl ParsedMemInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

/// Parse `/proc/meminfo` content. Tolerates missing fields (returns 0
/// for any field not present), so kernels with swap disabled or
/// without `MemAvailable` (very old kernels) still produce a partial result.
pub fn parse_meminfo(content: &str) -> ParsedMemInfo {
    let mut info = ParsedMemInfo::default();
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let key = match parts.next() {
            Some(k) => k.trim_end_matches(':'),
            None => continue,
        };
        let val_kb: u64 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let val_bytes = val_kb.saturating_mul(1024);
        match key {
            "MemTotal" => info.total = val_bytes,
            "MemFree" => info.free = val_bytes,
            "MemAvailable" => info.available = val_bytes,
            "SwapTotal" => info.swap_total = val_bytes,
            "SwapFree" => info.swap_free = val_bytes,
            _ => {}
        }
    }
    // Fallback for kernels without MemAvailable.
    if info.available == 0 {
        info.available = info.free;
    }
    info
}

// ─── /proc/diskstats ──────────────────────────────────────────────────────

/// Parse `/proc/diskstats` and return a map of `device_name → io_ticks (ms)`
/// for whole block devices only (partitions are filtered out).
///
/// `io_ticks` is field 12 (0-indexed) of each line. From kernel docs:
/// "milliseconds spent doing I/Os" — i.e. busy time, what we use to compute
/// busy %.
pub fn parse_diskstats(content: &str) -> HashMap<String, u64> {
    let mut result = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 13 {
            continue;
        }
        let name = parts[2];
        if !is_block_device(name) {
            continue;
        }
        if let Ok(ticks) = parts[12].parse::<u64>() {
            result.insert(name.to_string(), ticks);
        }
    }
    result
}

/// Heuristic: is this device name a whole block device (not a partition)?
/// Used by both the local /proc reader and the remote SSH parser, so it
/// must work without /sys/block lookups.
pub fn is_block_device(name: &str) -> bool {
    // sda, sdb (SCSI/SATA — not sda1)
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

// ─── /proc/net/dev ────────────────────────────────────────────────────────

/// Parse `/proc/net/dev` and return `interface → (rx_bytes, tx_bytes)`,
/// excluding the loopback adapter `lo`. The first two lines of the file
/// are headers and are skipped.
pub fn parse_net_dev(content: &str) -> HashMap<String, (u64, u64)> {
    let mut result = HashMap::new();
    for line in content.lines().skip(2) {
        let line = line.trim();
        let (iface, rest) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
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
    result
}

// ─── /proc/net/tcp{,6} ────────────────────────────────────────────────────

/// Parse `/proc/net/tcp` or `/proc/net/tcp6` content and return a list of
/// `(inode, tcp_state)` pairs. The header line is skipped.
///
/// TCP state codes (from include/net/tcp_states.h):
///   0x01 ESTABLISHED   0x06 TIME_WAIT
///   0x02 SYN_SENT      0x07 CLOSE
///   0x03 SYN_RECV      0x08 CLOSE_WAIT
///   0x04 FIN_WAIT1     0x09 LAST_ACK
///   0x05 FIN_WAIT2     0x0A LISTEN
pub fn parse_tcp_table(content: &str) -> Vec<(u64, u8)> {
    let mut entries = Vec::new();
    for line in content.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // col 3 = state (hex), col 9 = inode
        if parts.len() < 10 {
            continue;
        }
        let state = u8::from_str_radix(parts[3], 16).unwrap_or(0);
        let inode = parts[9].parse::<u64>().unwrap_or(0);
        if inode > 0 {
            entries.push((inode, state));
        }
    }
    entries
}

// ─── /proc/sys/fs/file-nr ─────────────────────────────────────────────────

/// Parse `/proc/sys/fs/file-nr`. Format: `allocated\tfree\tmax`.
/// Returns `(used, max)` where `used = allocated - free`.
pub fn parse_file_nr(content: &str) -> Option<(u64, u64)> {
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let allocated: u64 = parts[0].parse().ok()?;
    let free: u64 = parts[1].parse().ok()?;
    let max: u64 = parts[2].parse().ok()?;
    Some((allocated.saturating_sub(free), max))
}

// ─── /proc/stat ───────────────────────────────────────────────────────────

/// Parse the `ctxt` line from `/proc/stat` and return the system-wide
/// total context-switch count since boot. Returns `None` if the line is
/// missing or unparseable.
pub fn parse_proc_stat_ctxt(content: &str) -> Option<u64> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("ctxt ") {
            return rest.trim().parse().ok();
        }
    }
    None
}

// ─── /proc/<pid>/status ───────────────────────────────────────────────────

/// Per-process info parsed from `/proc/<pid>/status`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcStatusCtxt {
    pub name: String,
    pub voluntary: u64,
    pub nonvoluntary: u64,
}

impl ProcStatusCtxt {
    pub fn total(&self) -> u64 {
        self.voluntary + self.nonvoluntary
    }
}

pub fn parse_proc_status_ctxt(content: &str) -> ProcStatusCtxt {
    let mut info = ProcStatusCtxt::default();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Name:") {
            info.name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("voluntary_ctxt_switches:") {
            info.voluntary = rest.trim().parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("nonvoluntary_ctxt_switches:") {
            info.nonvoluntary = rest.trim().parse().unwrap_or(0);
        }
    }
    info
}

// ─── `ss -s` summary (remote-side socket overview) ───────────────────────

/// Parsed output of `ss -s`. Used as a remote-only fallback for socket
/// overview, since the local Linux collector reads /proc/net/tcp directly
/// (which gives more detail than `ss -s`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SsSummary {
    pub established: u32,
    pub time_wait: u32,
    pub tcp_total: u32,
}

/// Parse `ss -s` output. Looks for the `TCP:` line which has the format:
/// `TCP:   234 (estab 87, closed 12, orphaned 0, timewait 134)`
pub fn parse_ss_summary(content: &str) -> SsSummary {
    let mut summary = SsSummary::default();
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("TCP:") {
            let rest = rest.trim();
            // First number is the total
            if let Some(total_str) = rest.split_whitespace().next() {
                summary.tcp_total = total_str.parse().unwrap_or(0);
            }
            // Pull (estab N, ..., timewait N) from the parens
            if let (Some(open), Some(close)) = (rest.find('('), rest.find(')')) {
                if open < close {
                    let inner = &rest[open + 1..close];
                    for kv in inner.split(',') {
                        let kv = kv.trim();
                        if let Some(rest) = kv.strip_prefix("estab ") {
                            summary.established = rest.parse().unwrap_or(0);
                        } else if let Some(rest) = kv.strip_prefix("timewait ") {
                            summary.time_wait = rest.parse().unwrap_or(0);
                        }
                    }
                }
            }
            break;
        }
    }
    summary
}

// ─── `ps aux --sort=-%cpu` (remote-side top processes) ───────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct PsRow {
    pub user: String,
    pub pid: u32,
    pub cpu_pct: f32,
    pub mem_pct: f32,
    /// RSS in kilobytes — the actual resident set size from the kernel.
    /// Using this instead of deriving bytes from `mem_pct * total` gives
    /// values that match what `top` / `htop` show for the same process.
    pub rss_kb: u64,
    pub command: String,
}

/// Parse the output of `ps -eo user,pid,pcpu,pmem,rss,comm --no-headers`.
///
/// Format is strictly 6 whitespace-separated columns per line, no header:
///
/// ```text
/// USER PID %CPU %MEM RSS COMM
/// ```
///
/// `rss` is the resident set size in kilobytes (what `ps` natively reports).
/// `comm` is the executable's basename — no arguments, no paths, just one
/// token. First 5 fields are fixed, anything past index 5 is the command
/// (defensive: kernel threads with spaces like `[kworker/0:1H-kblockd]`).
///
/// Skips lines with < 6 tokens. Defensive: skips stale header rows too.
pub fn parse_ps_aux(content: &str) -> Vec<PsRow> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        // Skip any stray header row (defensive — --no-headers should
        // suppress it, but some older `ps` implementations emit it anyway).
        if cols[0] == "USER" && cols[1] == "PID" {
            continue;
        }
        // Skip rows where the "pid" column isn't numeric (also catches
        // stale headers with different casing).
        let Ok(pid) = cols[1].parse::<u32>() else {
            continue;
        };
        let command = cols[5..].join(" ");
        rows.push(PsRow {
            user: cols[0].to_string(),
            pid,
            cpu_pct: cols[2].parse().unwrap_or(0.0),
            mem_pct: cols[3].parse().unwrap_or(0.0),
            rss_kb: cols[4].parse().unwrap_or(0),
            command,
        });
    }
    rows
}

// ─── `df -P -T` output (per-mount disk space) ────────────────────────────

/// One entry from `df -P -T` output.
#[derive(Debug, Clone, PartialEq)]
pub struct DfEntry {
    pub filesystem: String,
    pub fstype: String,
    pub total_kb: u64,
    pub used_kb: u64,
    pub available_kb: u64,
    pub mount_point: String,
}

impl DfEntry {
    pub fn percent_used(&self) -> f64 {
        if self.total_kb == 0 {
            0.0
        } else {
            (self.used_kb as f64 / self.total_kb as f64) * 100.0
        }
    }
    pub fn percent_free(&self) -> f64 {
        100.0 - self.percent_used()
    }
}

/// Parse `df -P -T` output. Format (POSIX mode, with fstype):
///
/// ```text
/// Filesystem     Type     1024-blocks     Used Available Capacity Mounted on
/// /dev/nvme0n1p2 ext4       122295420 45823104  70203416      40% /
/// ```
///
/// Filters out pseudo-filesystems that aren't useful for triage
/// (loopback mounts, cgroup, sysfs, proc, devtmpfs), AND filters out
/// per-container Docker overlay mounts (one per container → 10+ rows
/// reporting the same underlying disk). The first overlay mount under
/// `/var/lib/docker/` is kept; subsequent ones are dropped.
pub fn parse_df(content: &str) -> Vec<DfEntry> {
    let mut entries = Vec::new();
    let mut seen_docker_overlay = false;
    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 7 {
            continue;
        }
        let fstype = cols[1];
        if is_noise_fs(fstype) {
            continue;
        }
        let mount_point = cols[6..].join(" ");
        // Drop per-container Docker overlay mounts. Each running
        // container registers one at `/var/lib/docker/overlay2/<hash>/merged`
        // (or the rootfs path on some configurations). They all report
        // the same underlying filesystem and would otherwise dominate
        // the display. Keep the first one as a signal that Docker is
        // active, drop the rest.
        if fstype == "overlay" && mount_point.starts_with("/var/lib/docker/") {
            if seen_docker_overlay {
                continue;
            }
            seen_docker_overlay = true;
        }
        let entry = DfEntry {
            filesystem: cols[0].to_string(),
            fstype: fstype.to_string(),
            total_kb: cols[2].parse().unwrap_or(0),
            used_kb: cols[3].parse().unwrap_or(0),
            available_kb: cols[4].parse().unwrap_or(0),
            mount_point,
        };
        entries.push(entry);
    }
    entries
}

fn is_noise_fs(fstype: &str) -> bool {
    matches!(
        fstype,
        "devtmpfs" | "devpts" | "sysfs" | "proc" | "cgroup" | "cgroup2" | "pstore"
            | "bpf" | "tracefs" | "debugfs" | "mqueue" | "hugetlbfs" | "securityfs"
            | "configfs" | "autofs" | "fusectl" | "nsfs" | "binfmt_misc"
    )
}

// ─── FD top-consumer loop output ─────────────────────────────────────────

/// Parse the output of the remote FD-top shell loop, which emits
/// lines like `"345 dockerd"` (count, command). Returns a Vec of
/// `(command, fd_count)` tuples sorted descending by count.
pub fn parse_fd_top_blob(content: &str) -> Vec<(String, u64)> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (count_str, name) = match line.split_once(char::is_whitespace) {
            Some((c, n)) => (c, n.trim()),
            None => continue,
        };
        let Ok(count) = count_str.parse::<u64>() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        rows.push((name.to_string(), count));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows
}

// ─── Context-switch top-consumer blob ────────────────────────────────────

/// Parse the output of the remote CSW-top shell loop, which concatenates
/// `/proc/<pid>/status` fragments separated by `---`:
///
/// ```text
/// Name:   dockerd
/// voluntary_ctxt_switches:   1234567
/// nonvoluntary_ctxt_switches:   89012
/// ---
/// Name:   postgres
/// ...
/// ```
///
/// Returns a Vec of `(command, total_switches)` sorted descending by total.
pub fn parse_csw_top_blob(content: &str) -> Vec<(String, u64)> {
    let mut rows = Vec::new();
    for block in content.split("---") {
        let parsed = parse_proc_status_ctxt(block);
        let total = parsed.total();
        if total > 0 && !parsed.name.is_empty() {
            rows.push((parsed.name, total));
        }
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows
}

// ─── `ss -tnH state established` output (detailed TCP) ───────────────────

/// One TCP connection as reported by `ss -tnH`. Used by both the top-N
/// by connection-count view and future detailed socket diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct SsDetailRow {
    pub state: String,
    pub local_addr: String,
    pub peer_addr: String,
    /// Process command name, if `-p` was available (requires root).
    pub process: Option<String>,
}

/// Parse `ss -tnH state established` (and variants). Each line looks like:
///
/// ```text
/// ESTAB 0 0 10.0.0.1:5432 10.0.0.5:45678 users:(("postgres",pid=1234,fd=11))
/// ```
///
/// Missing `users:(...)` column means `ss` was run without `-p`. We
/// still parse the address columns.
pub fn parse_ss_detailed_tcp(content: &str) -> Vec<SsDetailRow> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Expect at least state, recv-q, send-q, local, peer.
        if cols.len() < 5 {
            continue;
        }
        let state = cols[0].to_string();
        // Some formats have different column ordering — detect by
        // checking if cols[1] is a number (recv-q).
        if cols[1].parse::<u64>().is_err() {
            continue;
        }
        let local_addr = cols[3].to_string();
        let peer_addr = cols[4].to_string();
        let process = if cols.len() >= 6 {
            parse_ss_process_field(cols[5])
        } else {
            None
        };
        rows.push(SsDetailRow {
            state,
            local_addr,
            peer_addr,
            process,
        });
    }
    rows
}

/// Extract the command name from `users:(("nginx",pid=2345,fd=18))`.
fn parse_ss_process_field(field: &str) -> Option<String> {
    // Find the first `"..."` substring inside the field.
    let start = field.find('"')? + 1;
    let end = field[start..].find('"')? + start;
    Some(field[start..end].to_string())
}

/// Given the detailed SS rows, count the top-N processes by connection count.
/// Mirrors what `get_socket_stats` does locally, but from `ss` output.
pub fn top_processes_from_ss(rows: &[SsDetailRow], n: usize) -> Vec<(String, u32)> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for row in rows {
        if let Some(ref p) = row.process {
            // Only count connections that carry an established-ish state.
            if row.state == "ESTAB" || row.state == "LISTEN" || row.state == "CLOSE-WAIT" {
                *counts.entry(p.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<(String, u32)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(n);
    v
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/proc/{}", name))
            .unwrap_or_else(|e| panic!("missing fixture {}: {}", name, e))
    }

    // ── parse_loadavg ──

    #[test]
    fn parse_loadavg_happy_path() {
        let parsed = parse_loadavg(&fixture("loadavg.txt")).unwrap();
        assert_eq!(parsed.one, 0.42);
        assert_eq!(parsed.five, 0.31);
        assert_eq!(parsed.fifteen, 0.28);
    }

    #[test]
    fn parse_loadavg_empty_returns_none() {
        assert!(parse_loadavg("").is_none());
    }

    #[test]
    fn parse_loadavg_malformed_missing_fields_returns_none() {
        assert!(parse_loadavg("0.42 0.31").is_none());
        assert!(parse_loadavg("garbage garbage garbage").is_none());
    }

    // ── parse_meminfo ──

    #[test]
    fn parse_meminfo_happy_path() {
        let m = parse_meminfo(&fixture("meminfo.txt"));
        assert_eq!(m.total, 16_384_000 * 1024);
        assert_eq!(m.free, 2_048_000 * 1024);
        assert_eq!(m.available, 4_096_000 * 1024);
        assert_eq!(m.swap_total, 8_388_608 * 1024);
        assert_eq!(m.swap_free, 8_000_000 * 1024);
        // used = total - available
        assert_eq!(m.used(), (16_384_000 - 4_096_000) * 1024);
    }

    #[test]
    fn parse_meminfo_no_swap_kernel() {
        // SwapTotal/SwapFree absent — should be 0 (not a parse failure).
        let s = "MemTotal:       1024 kB\nMemFree:         512 kB\nMemAvailable:    768 kB\n";
        let m = parse_meminfo(s);
        assert_eq!(m.total, 1024 * 1024);
        assert_eq!(m.swap_total, 0);
        assert_eq!(m.swap_used(), 0);
    }

    #[test]
    fn parse_meminfo_old_kernel_no_memavailable_falls_back_to_free() {
        // Old kernels (<3.14) don't have MemAvailable.
        let s = "MemTotal: 1000 kB\nMemFree:  300 kB\n";
        let m = parse_meminfo(s);
        assert_eq!(m.available, 300 * 1024);
    }

    // ── parse_diskstats ──

    #[test]
    fn parse_diskstats_filters_partitions_and_loops() {
        let map = parse_diskstats(&fixture("diskstats.txt"));
        // Whole devices: nvme0n1, sda, dm-0
        assert!(map.contains_key("nvme0n1"));
        assert!(map.contains_key("sda"));
        assert!(map.contains_key("dm-0"));
        // Partitions and loops are filtered
        assert!(!map.contains_key("nvme0n1p1"));
        assert!(!map.contains_key("sda1"));
        assert!(!map.contains_key("loop0"));
        // io_ticks lives at column 12 of /proc/diskstats (after the 3-col
        // major/minor/name prefix). For the nvme0n1 fixture line that's 12345.
        assert_eq!(*map.get("nvme0n1").unwrap(), 12345);
    }

    // ── parse_net_dev ──

    #[test]
    fn parse_net_dev_excludes_loopback() {
        let map = parse_net_dev(&fixture("net_dev.txt"));
        assert!(!map.contains_key("lo"));
        assert!(map.contains_key("eth0"));
        // eth0: rx_bytes col 0 = 9876543210, tx_bytes col 8 = 1234567890
        let (rx, tx) = map.get("eth0").unwrap();
        assert_eq!(*rx, 9876543210);
        assert_eq!(*tx, 1234567890);
    }

    // ── parse_tcp_table ──

    #[test]
    fn parse_tcp_table_extracts_inode_and_state() {
        let entries = parse_tcp_table(&fixture("net_tcp.txt"));
        // header skipped, malformed line (sl 3, only 9 fields) skipped,
        // 5 valid rows expected.
        assert_eq!(entries.len(), 5);
        // First entry: state 0x0A (LISTEN), inode 12345
        assert_eq!(entries[0], (12345, 0x0A));
        // Third entry: state 0x01 (ESTABLISHED), inode 34567
        assert!(entries.iter().any(|&(i, s)| i == 34567 && s == 0x01));
        // sl 5: state 0x08 (CLOSE_WAIT), inode 56789
        assert!(entries.iter().any(|&(i, s)| i == 56789 && s == 0x08));
    }

    // ── parse_file_nr ──

    #[test]
    fn parse_file_nr_happy_path() {
        let (used, max) = parse_file_nr(&fixture("file_nr.txt")).unwrap();
        // 3456 - 0 = 3456 used; max = 1048576
        assert_eq!(used, 3456);
        assert_eq!(max, 1048576);
    }

    #[test]
    fn parse_file_nr_malformed_returns_none() {
        assert!(parse_file_nr("").is_none());
        assert!(parse_file_nr("not numbers here").is_none());
    }

    // ── parse_proc_stat_ctxt ──

    #[test]
    fn parse_proc_stat_ctxt_finds_ctxt_line() {
        let total = parse_proc_stat_ctxt(&fixture("stat.txt")).unwrap();
        assert_eq!(total, 9876543210);
    }

    #[test]
    fn parse_proc_stat_ctxt_missing_returns_none() {
        assert!(parse_proc_stat_ctxt("cpu  1 2 3 4\n").is_none());
    }

    // ── parse_proc_status_ctxt ──

    #[test]
    fn parse_proc_status_ctxt_happy_path() {
        let s = parse_proc_status_ctxt(&fixture("proc_status.txt"));
        assert_eq!(s.name, "flotop");
        assert_eq!(s.voluntary, 8765);
        assert_eq!(s.nonvoluntary, 432);
        assert_eq!(s.total(), 9197);
    }

    // ── parse_ss_summary ──

    #[test]
    fn parse_ss_summary_extracts_estab_and_timewait() {
        let s = parse_ss_summary(&fixture("ss_summary.txt"));
        assert_eq!(s.tcp_total, 234);
        assert_eq!(s.established, 87);
        assert_eq!(s.time_wait, 134);
    }

    // ── parse_ps_aux ──

    #[test]
    fn parse_ps_aux_extracts_top_processes() {
        let rows = parse_ps_aux(&fixture("ps_aux.txt"));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].user, "postgres");
        assert_eq!(rows[0].pid, 1234);
        assert_eq!(rows[0].cpu_pct, 87.5);
        assert_eq!(rows[0].rss_kb, 345678);
        assert_eq!(rows[0].command, "postgres");
        assert_eq!(rows[2].command, "node");
        assert_eq!(rows[2].rss_kb, 98765);
    }

    #[test]
    fn parse_ps_aux_skips_legacy_header_line_defensively() {
        // Some old `ps` implementations still emit the header even with
        // --no-headers. The parser must skip it.
        let input = "USER       PID %CPU %MEM RSS COMMAND\n\
                     postgres  1234 50.0  3.0 123456 postgres\n";
        let rows = parse_ps_aux(input);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user, "postgres");
        assert_eq!(rows[0].rss_kb, 123456);
    }

    #[test]
    fn parse_ps_aux_handles_kernel_threads_with_bracketed_names() {
        // Kernel threads show up as `[kworker/0:1H-kblockd]` — a single
        // token, but worth testing explicitly.
        let input = "root         5  0.0  0.0 0 [kworker/0:1H-kblockd]\n";
        let rows = parse_ps_aux(input);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "[kworker/0:1H-kblockd]");
        assert_eq!(rows[0].rss_kb, 0);
    }

    // ── parse_df ──

    #[test]
    fn parse_df_happy_path_multi_mount() {
        let entries = parse_df(&fixture("df.txt"));
        // 7 input lines → 7 entries (none are noise fs types in the
        // fixture — tmpfs, ext4, vfat, overlay all stay).
        assert_eq!(entries.len(), 7);
        let root = entries.iter().find(|e| e.mount_point == "/").unwrap();
        assert_eq!(root.fstype, "ext4");
        assert_eq!(root.total_kb, 122_295_420);
        assert_eq!(root.used_kb, 45_823_104);
        assert_eq!(root.available_kb, 70_203_416);
        // 45823104 / 122295420 ≈ 37.47%
        assert!((root.percent_used() - 37.47).abs() < 0.1);
    }

    #[test]
    fn parse_df_keeps_tmpfs_and_first_docker_overlay() {
        let entries = parse_df(&fixture("df.txt"));
        assert!(entries.iter().any(|e| e.fstype == "tmpfs"));
        // First Docker overlay kept, subsequent ones filtered.
        let overlay_count = entries.iter().filter(|e| e.fstype == "overlay").count();
        assert!(overlay_count <= 1);
    }

    #[test]
    fn parse_df_dedupes_per_container_docker_overlays() {
        // Simulate a busy Docker host with 10 container overlays.
        let mut input = String::from(
            "Filesystem     Type     1024-blocks     Used Available Capacity Mounted on\n\
             /dev/vda1      ext4        59848952 33949304  25883264      57% /\n",
        );
        for i in 0..10 {
            input.push_str(&format!(
                "overlay        overlay     59848952 33949304  25883264      57% /var/lib/docker/overlay2/hash{}/merged\n",
                i
            ));
        }
        let entries = parse_df(&input);
        // 1 root + exactly 1 overlay (the first).
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.iter().filter(|e| e.fstype == "overlay").count(),
            1
        );
    }

    #[test]
    fn parse_df_filters_noise_pseudo_filesystems() {
        let input = "Filesystem     Type     1024-blocks     Used Available Capacity Mounted on\n\
                     sysfs          sysfs              0        0         0       0% /sys\n\
                     proc           proc               0        0         0       0% /proc\n\
                     cgroup2        cgroup2            0        0         0       0% /sys/fs/cgroup\n\
                     /dev/sda1      ext4         1000000   500000    500000      50% /\n";
        let entries = parse_df(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mount_point, "/");
    }

    #[test]
    fn parse_df_empty_input_returns_empty() {
        assert!(parse_df("").is_empty());
        assert!(parse_df("Filesystem Type\n").is_empty());
    }

    #[test]
    fn df_entry_percent_free_is_complement_of_used() {
        let e = DfEntry {
            filesystem: "/dev/foo".into(),
            fstype: "ext4".into(),
            total_kb: 1000,
            used_kb: 400,
            available_kb: 600,
            mount_point: "/".into(),
        };
        assert_eq!(e.percent_used(), 40.0);
        assert_eq!(e.percent_free(), 60.0);
    }

    #[test]
    fn df_entry_zero_total_does_not_divide_by_zero() {
        let e = DfEntry {
            filesystem: "/dev/foo".into(),
            fstype: "tmpfs".into(),
            total_kb: 0,
            used_kb: 0,
            available_kb: 0,
            mount_point: "/run".into(),
        };
        assert_eq!(e.percent_used(), 0.0);
    }

    // ── parse_fd_top_blob ──

    #[test]
    fn parse_fd_top_blob_happy_path() {
        let rows = parse_fd_top_blob(&fixture("fd_top.txt"));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], ("dockerd".to_string(), 345));
        assert_eq!(rows[4], ("python3.12".to_string(), 45));
    }

    #[test]
    fn parse_fd_top_blob_sorts_descending_by_count() {
        let input = "5 a\n100 b\n50 c\n";
        let rows = parse_fd_top_blob(input);
        assert_eq!(rows[0].1, 100);
        assert_eq!(rows[1].1, 50);
        assert_eq!(rows[2].1, 5);
    }

    #[test]
    fn parse_fd_top_blob_skips_malformed_lines() {
        let input = "\n\n345 dockerd\nbogus line\n234 postgres\n   \n";
        let rows = parse_fd_top_blob(input);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn parse_fd_top_blob_empty_input_returns_empty() {
        assert!(parse_fd_top_blob("").is_empty());
    }

    // ── parse_csw_top_blob ──

    #[test]
    fn parse_csw_top_blob_happy_path() {
        let rows = parse_csw_top_blob(&fixture("csw_top.txt"));
        assert_eq!(rows.len(), 3);
        // First row should be dockerd (highest total: 1234567 + 89012 = 1323579).
        assert_eq!(rows[0].0, "dockerd");
        assert_eq!(rows[0].1, 1_234_567 + 89_012);
        assert_eq!(rows[1].0, "postgres");
    }

    #[test]
    fn parse_csw_top_blob_empty_input_returns_empty() {
        assert!(parse_csw_top_blob("").is_empty());
    }

    #[test]
    fn parse_csw_top_blob_missing_ctxt_fields_skipped() {
        let input = "Name:\tfoo\n---\nName:\tbar\nvoluntary_ctxt_switches:\t5\nnonvoluntary_ctxt_switches:\t3\n---\n";
        let rows = parse_csw_top_blob(input);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "bar");
    }

    // ── parse_ss_detailed_tcp ──

    #[test]
    fn parse_ss_detailed_tcp_happy_path() {
        let rows = parse_ss_detailed_tcp(&fixture("ss_detail.txt"));
        // 7 lines all parseable (LISTEN + TIME-WAIT also have valid shape)
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].state, "ESTAB");
        assert_eq!(rows[0].local_addr, "10.0.0.1:5432");
        assert_eq!(rows[0].peer_addr, "10.0.0.5:45678");
        assert_eq!(rows[0].process.as_deref(), Some("postgres"));
    }

    #[test]
    fn parse_ss_detailed_tcp_row_without_process_field() {
        // TIME-WAIT entries don't have users: column.
        let rows = parse_ss_detailed_tcp(&fixture("ss_detail.txt"));
        let tw = rows.iter().find(|r| r.state == "TIME-WAIT").unwrap();
        assert!(tw.process.is_none());
    }

    #[test]
    fn top_processes_from_ss_counts_and_sorts() {
        let rows = parse_ss_detailed_tcp(&fixture("ss_detail.txt"));
        let top = top_processes_from_ss(&rows, 5);
        // nginx has 2 conns, postgres has 2, redis 1, sshd 1 (LISTEN counts)
        assert!(top.iter().any(|(name, _)| name == "nginx"));
        assert!(top.iter().any(|(name, _)| name == "postgres"));
        // Top two should both be at 2.
        assert_eq!(top[0].1, 2);
    }

    #[test]
    fn parse_ss_process_field_extracts_command() {
        assert_eq!(
            parse_ss_process_field("users:((\"nginx\",pid=2345,fd=18))"),
            Some("nginx".to_string())
        );
        assert_eq!(parse_ss_process_field("garbage"), None);
    }
}
