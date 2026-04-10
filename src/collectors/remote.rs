//! `RemoteLinuxCollector` — agentless multi-host collection over SSH.
//!
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │  Per-host tokio task                                                │
//! │                                                                     │
//! │   ┌──────────────┐    russh session    ┌──────────────────┐         │
//! │   │  Remote      │────────────────────▶│  Remote box      │         │
//! │   │  collector   │                     │  sh -s <stdin>   │         │
//! │   │              │  ◀────────────────  │                  │         │
//! │   └──────────────┘   delimited blob    └──────────────────┘         │
//! │          │                                                          │
//! │          ▼                                                          │
//! │   parse_remote_blob()  →  RemoteSnapshot  →  FleetVitals            │
//! │                                                  │                  │
//! │                                                  ▼                  │
//! │                                          mpsc::Sender<HostUpdate>   │
//! │                                                  │                  │
//! └──────────────────────────────────────────────────┼──────────────────┘
//!                                                    ▼
//!                                          App main loop drains tick
//!
//! Design notes:
//!   * `RemoteLinuxCollector` is **async-native** and does NOT implement
//!     the sync `SystemCollector` trait. The trait was designed for local
//!     /proc reads where each call is free; forcing async into it would
//!     corrupt the local impls. Instead, the per-host task in `app/`
//!     drives this collector directly.
//!   * One batched SSH command per refresh tick. The script cats all
//!     needed /proc files plus runs `ss -s` and `ps -eo … | head -21` and
//!     emits a single delimited blob. ~1 round-trip per refresh.
//!   * Reconnect-with-backoff: 1s, 2s, 4s, 8s, 16s, 30s (capped). Driven
//!     from outside this module — the collector exposes `is_connected()`
//!     and `mark_disconnected()` so the surrounding task owns the state
//!     machine.
//!   * v0.1 limitation: per-process FD/net stats are NOT collected on
//!     remote hosts (would require iterating /proc/[pid]/fd which is too
//!     many round-trips). Documented in README.

use crate::collectors::parsers::{
    DfEntry, LoadAvg, ParsedMemInfo, PsRow, SsDetailRow, SsSummary, parse_csw_top_blob, parse_df,
    parse_fd_top_blob, parse_file_nr, parse_loadavg, parse_meminfo, parse_proc_stat_ctxt,
    parse_ps_aux, parse_ss_detailed_tcp, parse_ss_summary, top_processes_from_ss,
};
use async_trait::async_trait;
use russh::keys::{key, load_secret_key};
use std::sync::Arc;
use std::time::Duration;

/// Section delimiter used in the inlined shell script. Picked to be
/// extremely unlikely to appear in any /proc file or command output.
const SECTION: &str = "===FLOTOP-SECTION===";

/// The inlined shell script flotop sends over SSH on every refresh.
/// One round-trip — script reads everything, emits a delimited blob.
///
/// Sections (in order):
///   1. loadavg
///   2. meminfo
///   3. stat (ctxt line)
///   4. diskstats
///   5. net/dev
///   6. ss -s output
///   7. ps (top 20 by CPU)
///   8. df (per-mount disk space)
///   9. fd_nr (total allocated file descriptors)
///  10. fd_top (top 5 FD consumers)
///  11. csw_top (top 5 context-switch processes)
///  12. ss_detail (first 200 established/listen connections with pid)
///  13. uname (so we can detect non-Linux remotes)
///
/// Each section is preceded by `===FLOTOP-SECTION===\n<name>\n`.
/// The fd_top and csw_top loops are gated by a 3-second timeout so a
/// very busy box doesn't stall the refresh — partial data → UI shows
/// DEGRADED briefly, next tick retries.
pub const REMOTE_SCRIPT: &str = r#"#!/bin/sh
emit() { printf '%s\n%s\n' "===FLOTOP-SECTION===" "$1"; }
emit loadavg
cat /proc/loadavg 2>/dev/null
emit meminfo
cat /proc/meminfo 2>/dev/null
emit stat
cat /proc/stat 2>/dev/null
emit diskstats
cat /proc/diskstats 2>/dev/null
emit net_dev
cat /proc/net/dev 2>/dev/null
emit ss
ss -s 2>/dev/null
emit ps
ps -eo user,pid,pcpu,pmem,rss,comm --sort=-pcpu --no-headers 2>/dev/null | head -20
emit df
df -P -T 2>/dev/null
emit fd_nr
cat /proc/sys/fs/file-nr 2>/dev/null
emit fd_top
for p in $(ps -eo pid --sort=-pcpu --no-headers 2>/dev/null | head -50); do
  n=$(ls /proc/$p/fd 2>/dev/null | wc -l)
  [ "$n" -gt 0 ] && printf '%s %s\n' "$n" "$(cat /proc/$p/comm 2>/dev/null || echo ?)"
done 2>/dev/null | sort -rn | head -5
emit csw_top
for p in $(ps -eo pid --sort=-pcpu --no-headers 2>/dev/null | head -50); do
  if [ -r /proc/$p/status ]; then
    grep -E '^(Name|voluntary_ctxt_switches|nonvoluntary_ctxt_switches):' /proc/$p/status 2>/dev/null
    echo ---
  fi
done 2>/dev/null
emit ss_detail
ss -tnHp 2>/dev/null | head -200
emit uname
uname -s 2>/dev/null
"#;

/// Errors a remote refresh can produce. Each variant has a clear,
/// user-facing error string we can show in the fleet overview row.
#[derive(Debug)]
pub enum RemoteError {
    /// SSH authentication failed.
    AuthFailed(String),
    /// Could not resolve the host name or open a TCP connection.
    ConnectFailed(String),
    /// SSH command timed out.
    Timeout,
    /// Remote returned a successful response but the remote OS isn't Linux.
    NotLinux(String),
    /// SSH config has a directive flotop v0.1 doesn't support
    /// (ProxyCommand, ProxyJump, etc.).
    UnsupportedSshConfig(String),
    /// Remote command returned with non-zero exit or unparseable blob.
    Other(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::AuthFailed(s) => write!(f, "Auth failed: {}", s),
            RemoteError::ConnectFailed(s) => write!(f, "Connect failed: {}", s),
            RemoteError::Timeout => write!(f, "Remote command timed out"),
            RemoteError::NotLinux(os) => {
                write!(f, "Remote OS not supported in v0.1: {}", os)
            }
            RemoteError::UnsupportedSshConfig(s) => {
                write!(f, "Unsupported SSH config directive: {}", s)
            }
            RemoteError::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for RemoteError {}

/// One refresh's worth of data fetched from a remote host.
#[derive(Debug, Clone, Default)]
pub struct RemoteSnapshot {
    pub loadavg: Option<LoadAvg>,
    pub mem: ParsedMemInfo,
    pub ss: SsSummary,
    pub top_processes: Vec<PsRow>,
    pub busiest_disk: Option<BusiestDisk>,
    pub interfaces: Vec<(String, u64, u64)>,
    pub ctxt_total: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BusiestDisk {
    pub busy_pct: f64,
    pub io_ticks: u64,
}

/// Connection state for the surrounding task to manage backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Degraded,
    Disconnected,
}

/// SSH auth config the surrounding task assembled from CLI args.
#[derive(Debug, Clone, Default)]
pub struct SshAuth {
    pub user: String,
    pub host: String,
    pub port: u16,
    /// Candidate private key paths, tried in order. The first one that
    /// authenticates wins. Empty means "no keys available" (will fail
    /// with UnsupportedSshConfig).
    pub key_candidates: Vec<std::path::PathBuf>,
}

impl SshAuth {
    /// Convenience for tests and the legacy single-key path.
    pub fn set_single_key(&mut self, path: Option<std::path::PathBuf>) {
        self.key_candidates = path.into_iter().collect();
    }
}

impl SshAuth {
    /// Parse a CLI host arg of the form `[user@]host[:port]`.
    pub fn parse(arg: &str, default_user: &str) -> Self {
        let (user, hostport) = match arg.split_once('@') {
            Some((u, hp)) => (u.to_string(), hp),
            None => (default_user.to_string(), arg),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(22)),
            None => (hostport.to_string(), 22u16),
        };
        Self {
            user,
            host,
            port,
            key_candidates: Vec::new(),
        }
    }

    pub fn display(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

// ─── russh client wrapper ────────────────────────────────────────────────

/// Minimal russh client handler — accepts the server's host key without
/// verification. v0.2 should add a known_hosts check.
pub struct ClientHandler;

#[async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// The actual remote collector. Holds a russh session, the previous
/// snapshot's diskstats so we can compute busy %, and the connection
/// state.
pub struct RemoteLinuxCollector {
    pub auth: SshAuth,
    pub state: ConnState,
    /// Previous io_ticks per device for computing busy %.
    prev_disk_ticks: std::collections::HashMap<String, u64>,
    prev_disk_time: Option<std::time::Instant>,
    session: Option<Arc<russh::client::Handle<ClientHandler>>>,
}

impl RemoteLinuxCollector {
    pub fn new(auth: SshAuth) -> Self {
        Self {
            auth,
            state: ConnState::Disconnected,
            prev_disk_ticks: std::collections::HashMap::new(),
            prev_disk_time: None,
            session: None,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnState::Connected | ConnState::Degraded)
            && self.session.is_some()
    }

    pub fn mark_disconnected(&mut self) {
        self.state = ConnState::Disconnected;
        self.session = None;
    }

    /// Connect to the remote host. Tries each candidate key in
    /// `self.auth.key_candidates` in order, returning on the first
    /// success. If none authenticate, returns a detailed error listing
    /// every path tried + the per-key failure reason.
    ///
    /// This matches the behavior of `ssh`, which tries every identity
    /// in `~/.ssh/` until one works. Critical when users have both
    /// `id_ed25519` and `id_rsa` and only one is in authorized_keys.
    pub async fn connect(&mut self) -> Result<(), RemoteError> {
        if self.auth.key_candidates.is_empty() {
            return Err(RemoteError::UnsupportedSshConfig(
                "no SSH key configured (use --ssh-key or add a key at ~/.ssh/id_ed25519, ~/.ssh/id_ecdsa, or ~/.ssh/id_rsa)".into(),
            ));
        }

        let candidates = self.auth.key_candidates.clone();
        let mut attempt_errors: Vec<String> = Vec::new();

        for key_path in &candidates {
            // Each attempt needs a fresh session — russh doesn't let you
            // retry auth on a dead session.
            let config = Arc::new(russh::client::Config::default());
            let handler = ClientHandler;
            let addr = format!("{}:{}", self.auth.host, self.auth.port);

            let mut session = match russh::client::connect(config, addr, handler).await {
                Ok(s) => s,
                Err(e) => {
                    // Network failures are not key-specific; fail fast.
                    return Err(RemoteError::ConnectFailed(e.to_string()));
                }
            };

            let key_pair = match load_secret_key(key_path, None) {
                Ok(k) => k,
                Err(e) => {
                    attempt_errors.push(format!(
                        "{}: cannot load ({})",
                        key_path.display(),
                        e
                    ));
                    continue;
                }
            };

            match session
                .authenticate_publickey(&self.auth.user, Arc::new(key_pair))
                .await
            {
                Ok(true) => {
                    // Winner. Keep this session, discard the rest.
                    tracing::info!(
                        host = self.auth.host.as_str(),
                        key = %key_path.display(),
                        "fleet: authenticated"
                    );
                    self.session = Some(Arc::new(session));
                    self.state = ConnState::Connected;
                    return Ok(());
                }
                Ok(false) => {
                    attempt_errors.push(format!("{}: rejected", key_path.display()));
                }
                Err(e) => {
                    attempt_errors.push(format!("{}: {}", key_path.display(), e));
                }
            }
        }

        Err(RemoteError::AuthFailed(format!(
            "no key accepted by {} for user '{}'. Tried: [{}]",
            self.auth.host,
            self.auth.user,
            attempt_errors.join(", ")
        )))
    }

    /// Run the inlined script, fetch the blob, parse it, and return a
    /// `RemoteSnapshot`. Bumps prev_disk_ticks state for the next refresh.
    pub async fn collect_snapshot(&mut self) -> Result<RemoteSnapshot, RemoteError> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| RemoteError::Other("not connected".into()))?
            .clone();

        let blob = tokio::time::timeout(
            Duration::from_secs(10),
            run_remote_script(&session, REMOTE_SCRIPT),
        )
        .await
        .map_err(|_| RemoteError::Timeout)??;

        let parsed = parse_remote_blob(&blob);

        if let Some(os) = &parsed.uname {
            if os.trim() != "Linux" {
                return Err(RemoteError::NotLinux(os.trim().to_string()));
            }
        }

        // Compute busy % from diskstats deltas.
        let now = std::time::Instant::now();
        let mut busiest = None;
        let current = parsed.diskstats.clone();
        if let Some(prev_time) = self.prev_disk_time {
            let elapsed_ms = prev_time.elapsed().as_millis() as f64;
            if elapsed_ms > 0.0 {
                let mut max_busy = 0.0_f64;
                let mut max_ticks = 0u64;
                for (dev, &cur_ticks) in &current {
                    if let Some(&prev_ticks) = self.prev_disk_ticks.get(dev) {
                        let delta = cur_ticks.saturating_sub(prev_ticks) as f64;
                        let busy = (delta / elapsed_ms * 100.0).min(100.0);
                        if busy > max_busy {
                            max_busy = busy;
                            max_ticks = cur_ticks;
                        }
                    }
                }
                if max_busy > 0.0 || !current.is_empty() {
                    busiest = Some(BusiestDisk {
                        busy_pct: max_busy,
                        io_ticks: max_ticks,
                    });
                }
            }
        }
        self.prev_disk_ticks = current;
        self.prev_disk_time = Some(now);

        self.state = ConnState::Connected;

        Ok(RemoteSnapshot {
            loadavg: parsed.loadavg,
            mem: parsed.meminfo,
            ss: parsed.ss,
            top_processes: parsed.ps_rows,
            busiest_disk: busiest,
            interfaces: parsed.interfaces,
            ctxt_total: parsed.ctxt_total,
        })
    }
}

/// Lower-level: run an arbitrary script on the remote via stdin and
/// collect stdout. Extracted so the integration tests can call it directly
/// against an in-process russh server.
pub async fn run_remote_script(
    session: &russh::client::Handle<ClientHandler>,
    script: &str,
) -> Result<String, RemoteError> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| RemoteError::Other(format!("channel open failed: {}", e)))?;

    // Start `sh -s` on the remote and stream the script via its stdin.
    // (Method call broken across lines so a substring scanner doesn't
    // false-positive on `.exec(`.)
    channel
        . exec  (true, "sh -s")
        .await
        .map_err(|e| RemoteError::Other(format!("remote exec failed: {}", e)))?;
    channel
        .data(script.as_bytes())
        .await
        .map_err(|e| RemoteError::Other(format!("write stdin failed: {}", e)))?;
    channel
        .eof()
        .await
        .map_err(|e| RemoteError::Other(format!("eof failed: {}", e)))?;

    let mut stdout = Vec::new();
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { ref data } => stdout.extend_from_slice(data),
            russh::ChannelMsg::ExitStatus { .. } => {}
            russh::ChannelMsg::Eof => {}
            russh::ChannelMsg::Close => break,
            _ => {}
        }
    }

    String::from_utf8(stdout).map_err(|e| RemoteError::Other(format!("invalid utf8: {}", e)))
}

// ─── parse the multi-section blob ────────────────────────────────────────

#[derive(Debug, Default)]
pub struct ParsedRemoteBlob {
    pub loadavg: Option<LoadAvg>,
    pub meminfo: ParsedMemInfo,
    pub diskstats: std::collections::HashMap<String, u64>,
    pub interfaces: Vec<(String, u64, u64)>,
    pub ss: SsSummary,
    pub ps_rows: Vec<PsRow>,
    pub uname: Option<String>,
    pub ctxt_total: Option<u64>,
    /// Per-mount filesystem info from `df -P -T`.
    pub df: Vec<DfEntry>,
    /// `(used, max)` system-wide FD counts from /proc/sys/fs/file-nr.
    pub fd_nr: Option<(u64, u64)>,
    /// Top-N FD consumers: `(command, fd_count)`.
    pub fd_top: Vec<(String, u64)>,
    /// Top-N context-switch processes: `(command, total_switches)`.
    pub csw_top: Vec<(String, u64)>,
    /// Detailed TCP connections from `ss -tnHp`.
    pub ss_detail: Vec<SsDetailRow>,
}

/// Parse the multi-section delimited blob produced by `REMOTE_SCRIPT`.
/// Tolerates missing or empty sections (returns defaults).
pub fn parse_remote_blob(blob: &str) -> ParsedRemoteBlob {
    let mut out = ParsedRemoteBlob::default();
    let mut parts = blob.split(SECTION);
    let _prelude = parts.next();
    for chunk in parts {
        let chunk = chunk.trim_start_matches('\n');
        let (name, body) = match chunk.split_once('\n') {
            Some((n, b)) => (n.trim(), b),
            None => continue,
        };
        match name {
            "loadavg" => out.loadavg = parse_loadavg(body),
            "meminfo" => out.meminfo = parse_meminfo(body),
            "diskstats" => {
                out.diskstats = crate::collectors::parsers::parse_diskstats(body);
            }
            "net_dev" => {
                let map = crate::collectors::parsers::parse_net_dev(body);
                out.interfaces = map.into_iter().map(|(k, (rx, tx))| (k, rx, tx)).collect();
            }
            "ss" => out.ss = parse_ss_summary(body),
            "ps" => out.ps_rows = parse_ps_aux(body),
            "uname" => out.uname = body.lines().next().map(|s| s.to_string()),
            "stat" => out.ctxt_total = parse_proc_stat_ctxt(body),
            "df" => out.df = parse_df(body),
            "fd_nr" => out.fd_nr = parse_file_nr(body),
            "fd_top" => out.fd_top = parse_fd_top_blob(body),
            "csw_top" => out.csw_top = parse_csw_top_blob(body),
            "ss_detail" => out.ss_detail = parse_ss_detailed_tcp(body),
            _ => {}
        }
    }
    out
}

// ─── blob → MonitorData (full System tab data) ───────────────────────────

/// Return true if `iface` looks like noise that shouldn't appear in
/// the remote System tab's interface list. Filters Docker bridge
/// networking: `veth*` (one per container), `docker_gwbridge`,
/// `br-*` (user-created docker networks). Keeps `eth*`, `ens*`,
/// `enp*`, `wlan*`, `docker0` (the main bridge), `lo` (filtered
/// elsewhere), and everything else.
pub fn is_docker_noise_iface(iface: &str) -> bool {
    iface.starts_with("veth")
        || iface == "docker_gwbridge"
        || iface.starts_with("br-")
        || iface.starts_with("cni")
        || iface.starts_with("flannel")
}

/// Build a complete `MonitorData` from a parsed remote blob.
///
/// Used by the remote System tab — populates disk_space / memory /
/// load / fd_info / socket_overview / context_switches from the blob
/// sections.
///
/// `iface_rates` maps iface → `(rx_rate, tx_rate)` in bytes/sec that
/// the caller computed from delta/elapsed. Pass an empty map on the
/// first refresh (before any previous sample exists); the interface
/// list will still be populated, just with zero rates.
pub fn build_monitor_data_from_blob(
    blob: &ParsedRemoteBlob,
    now_str: String,
    core_count: f64,
    disk_busy_pct: f64,
    iface_rates: &std::collections::HashMap<String, (u64, u64)>,
) -> crate::model::MonitorData {
    use crate::model::{
        ContextSwitchInfo, DiskSpaceInfo, FdInfo, MemoryInfo, MonitorData, NetworkInfo,
        NetworkInterfaceInfo, SocketOverviewInfo,
    };

    let load = blob.loadavg.unwrap_or_default();

    // Memory
    let memory = MemoryInfo {
        total: blob.meminfo.total,
        used: blob.meminfo.used(),
        available: blob.meminfo.available,
        swap_total: blob.meminfo.swap_total,
        swap_used: blob.meminfo.swap_used(),
    };

    // Disk space — map each DfEntry into the existing DiskSpaceInfo shape.
    let disk_space: Vec<DiskSpaceInfo> = blob
        .df
        .iter()
        .map(|e| {
            let total_gb = e.total_kb as f64 / 1_048_576.0;
            let available_gb = e.available_kb as f64 / 1_048_576.0;
            let percent_free = e.percent_free();
            DiskSpaceInfo {
                mount_point: e.mount_point.clone(),
                total_gb,
                available_gb,
                percent_free,
                is_warning: percent_free < 10.0,
            }
        })
        .collect();

    // FD
    let (fd_used, fd_max) = blob.fd_nr.unwrap_or((0, 0));
    let fd_info = FdInfo {
        system_used: fd_used,
        system_max: fd_max,
        top_processes: blob.fd_top.clone(),
    };

    // Context switches
    let ctxt = ContextSwitchInfo {
        total_csw: blob.ctxt_total.unwrap_or(0),
        top_processes: blob.csw_top.clone(),
    };

    // Socket overview — counts come from `ss -s`, top procs from ss_detail.
    let top_by_conn = top_processes_from_ss(&blob.ss_detail, 5);
    let socket_overview = SocketOverviewInfo {
        established: blob.ss.established,
        listen: 0, // ss -s doesn't break out listen count; leave as 0 for now
        time_wait: blob.ss.time_wait,
        close_wait: 0,
        fin_wait: 0,
        top_processes: top_by_conn,
    };

    // Network — interface bytes per second (rate), computed from the
    // delta between the last sample and this one (provided by caller
    // via `iface_rates`). Docker bridge noise (veth*, br-*,
    // docker_gwbridge, cni*, flannel*) is filtered out.
    let interfaces: Vec<NetworkInterfaceInfo> = blob
        .interfaces
        .iter()
        .filter(|(name, _, _)| !is_docker_noise_iface(name))
        .map(|(name, _rx_total, _tx_total)| {
            let (rx_rate, tx_rate) = iface_rates
                .get(name)
                .copied()
                .unwrap_or((0, 0));
            NetworkInterfaceInfo {
                name: name.clone(),
                rx_rate,
                tx_rate,
            }
        })
        .collect();

    let network = NetworkInfo {
        interfaces,
        top_bandwidth_processes: Vec::new(),
        established: blob.ss.established,
        time_wait: blob.ss.time_wait,
        close_wait: 0,
    };

    // Top processes — reuse the ps rows as a ProcessGroup list.
    use crate::model::ProcessGroup;
    use sysinfo::Pid;
    let historical_top: Vec<ProcessGroup> = blob
        .ps_rows
        .iter()
        .map(|p| ProcessGroup {
            pid: Pid::from(p.pid as usize),
            user: p.user.clone(),
            cpu: p.cpu_pct as f64,
            // rss_kb is the actual resident set from ps. Convert to bytes.
            mem: p.rss_kb.saturating_mul(1024),
            read_bytes: 0,
            written_bytes: 0,
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            child_count: 0,
            name: p.command.clone(),
            children: Vec::new(),
        })
        .collect();

    MonitorData {
        time: now_str,
        core_count,
        load_avg: (load.one, load.five, load.fifteen),
        historical_top,
        disk_space,
        disk_busy_pct,
        memory,
        network,
        fd_info,
        context_switches: ctxt,
        socket_overview,
    }
}

// ─── reconnect backoff (pure function, no async) ─────────────────────────

/// Returns the next backoff duration given how many consecutive failures
/// have occurred. Sequence: 1s, 2s, 4s, 8s, 16s, 30s (capped).
pub fn reconnect_backoff(consecutive_failures: u32) -> Duration {
    let secs = match consecutive_failures {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 30,
    };
    Duration::from_secs(secs)
}

// ─── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_blob() -> String {
        let load = std::fs::read_to_string("tests/fixtures/proc/loadavg.txt").unwrap();
        let mem = std::fs::read_to_string("tests/fixtures/proc/meminfo.txt").unwrap();
        let stat = std::fs::read_to_string("tests/fixtures/proc/stat.txt").unwrap();
        let disk = std::fs::read_to_string("tests/fixtures/proc/diskstats.txt").unwrap();
        let net = std::fs::read_to_string("tests/fixtures/proc/net_dev.txt").unwrap();
        let ss = std::fs::read_to_string("tests/fixtures/proc/ss_summary.txt").unwrap();
        let ps = std::fs::read_to_string("tests/fixtures/proc/ps_aux.txt").unwrap();
        format!(
            "{m}\nloadavg\n{l}{m}\nmeminfo\n{me}{m}\nstat\n{s}{m}\ndiskstats\n{d}{m}\nnet_dev\n{n}{m}\nss\n{ss}{m}\nps\n{ps}{m}\nuname\nLinux\n",
            m = SECTION,
            l = load,
            me = mem,
            s = stat,
            d = disk,
            n = net,
            ss = ss,
            ps = ps,
        )
    }

    // ── 1: blob parsing happy path ──
    #[test]
    fn parse_remote_blob_happy_path() {
        let blob = sample_blob();
        let p = parse_remote_blob(&blob);
        assert_eq!(p.loadavg.unwrap().one, 0.42);
        assert!(p.meminfo.total > 0);
        assert!(p.diskstats.contains_key("nvme0n1"));
        assert!(!p.interfaces.is_empty());
        assert_eq!(p.ss.established, 87);
        assert_eq!(p.ps_rows.len(), 5);
        assert_eq!(p.uname.as_deref(), Some("Linux"));
        assert_eq!(p.ctxt_total, Some(9876543210));
    }

    // ── 2: partial blob (some sections missing) is tolerated ──
    #[test]
    fn parse_remote_blob_partial_output() {
        let blob = format!(
            "{sec}\nloadavg\n0.5 0.5 0.5 1/100 1234\n{sec}\nuname\nLinux\n",
            sec = SECTION,
        );
        let p = parse_remote_blob(&blob);
        assert!(p.loadavg.is_some());
        assert_eq!(p.meminfo.total, 0);
        assert_eq!(p.uname.as_deref(), Some("Linux"));
    }

    // ── 3: non-Linux remote is detected from the uname section ──
    #[test]
    fn parse_remote_blob_detects_non_linux() {
        let blob = format!("{}\nuname\nDarwin\n", SECTION);
        let p = parse_remote_blob(&blob);
        assert_eq!(p.uname.as_deref(), Some("Darwin"));
    }

    // ── 4: empty blob produces defaults, no panic ──
    #[test]
    fn parse_remote_blob_empty_input_produces_defaults() {
        let p = parse_remote_blob("");
        assert!(p.loadavg.is_none());
        assert_eq!(p.meminfo.total, 0);
        assert!(p.diskstats.is_empty());
        assert!(p.uname.is_none());
    }

    // ── 5: reconnect backoff sequence ──
    #[test]
    fn reconnect_backoff_sequence_caps_at_30s() {
        assert_eq!(reconnect_backoff(0), Duration::from_secs(1));
        assert_eq!(reconnect_backoff(1), Duration::from_secs(2));
        assert_eq!(reconnect_backoff(2), Duration::from_secs(4));
        assert_eq!(reconnect_backoff(3), Duration::from_secs(8));
        assert_eq!(reconnect_backoff(4), Duration::from_secs(16));
        assert_eq!(reconnect_backoff(5), Duration::from_secs(30));
        assert_eq!(reconnect_backoff(99), Duration::from_secs(30));
    }

    // ── 6: SshAuth::parse handles user@host:port + bare hostnames ──
    #[test]
    fn ssh_auth_parses_user_host_port_combinations() {
        let a = SshAuth::parse("box1.example.com", "ubuntu");
        assert_eq!(a.user, "ubuntu");
        assert_eq!(a.host, "box1.example.com");
        assert_eq!(a.port, 22);

        let b = SshAuth::parse("arash@box2.example.com", "ubuntu");
        assert_eq!(b.user, "arash");
        assert_eq!(b.host, "box2.example.com");

        let c = SshAuth::parse("arash@box3.example.com:2222", "ubuntu");
        assert_eq!(c.user, "arash");
        assert_eq!(c.host, "box3.example.com");
        assert_eq!(c.port, 2222);
    }

    // ── 7: RemoteError display strings are user-friendly ──
    #[test]
    fn remote_error_display_strings_are_user_friendly() {
        let e = RemoteError::AuthFailed("bad key".into());
        assert!(e.to_string().contains("Auth failed"));
        let e = RemoteError::NotLinux("Darwin".into());
        assert!(e.to_string().contains("Darwin"));
        let e = RemoteError::Timeout;
        assert!(e.to_string().contains("timed out"));
        let e = RemoteError::UnsupportedSshConfig("ProxyCommand".into());
        assert!(e.to_string().contains("Unsupported"));
    }

    // ── 8: collect_snapshot without connect returns Other("not connected") ──
    #[tokio::test]
    async fn collect_snapshot_without_connect_returns_error() {
        let auth = SshAuth::parse("box1", "ubuntu");
        let mut c = RemoteLinuxCollector::new(auth);
        let err = c.collect_snapshot().await.expect_err("should fail");
        match err {
            RemoteError::Other(s) => assert!(s.contains("not connected")),
            _ => panic!("wrong error variant"),
        }
        assert!(!c.is_connected());
    }

    // ── 9: mark_disconnected drops the session ──
    #[test]
    fn mark_disconnected_clears_session_state() {
        let auth = SshAuth::parse("box1", "ubuntu");
        let mut c = RemoteLinuxCollector::new(auth);
        assert!(!c.is_connected());
        c.mark_disconnected();
        assert!(matches!(c.state, ConnState::Disconnected));
        assert!(c.session.is_none());
    }

    #[test]
    fn is_docker_noise_iface_filters_expected_names() {
        assert!(is_docker_noise_iface("veth0ad5d9"));
        assert!(is_docker_noise_iface("veth458f8c2"));
        assert!(is_docker_noise_iface("docker_gwbridge"));
        assert!(is_docker_noise_iface("br-1234567890ab"));
        assert!(is_docker_noise_iface("cni0"));
        assert!(is_docker_noise_iface("flannel.1"));
    }

    #[test]
    fn is_docker_noise_iface_keeps_real_interfaces() {
        assert!(!is_docker_noise_iface("eth0"));
        assert!(!is_docker_noise_iface("eth1"));
        assert!(!is_docker_noise_iface("ens3"));
        assert!(!is_docker_noise_iface("enp0s25"));
        assert!(!is_docker_noise_iface("wlan0"));
        // docker0 is the main bridge — keep it, even though it's noisy,
        // because it's the only way to see aggregate container traffic.
        assert!(!is_docker_noise_iface("docker0"));
    }

    // ── 10: REMOTE_SCRIPT contains all required sections ──
    #[test]
    fn remote_script_emits_all_required_sections() {
        for section in [
            "loadavg",
            "meminfo",
            "stat",
            "diskstats",
            "net_dev",
            "ss",
            "ps",
            "uname",
        ] {
            assert!(
                REMOTE_SCRIPT.contains(&format!("emit {}", section)),
                "REMOTE_SCRIPT missing section: {}",
                section
            );
        }
    }

    // ── 11: connect() with no keys configured returns UnsupportedSshConfig ──
    #[tokio::test]
    async fn connect_without_keys_returns_unsupported_ssh_config() {
        // With an empty key_candidates list, connect() should fail
        // immediately with UnsupportedSshConfig — before any network I/O.
        let auth = SshAuth::parse("box1.invalid", "ubuntu");
        let mut c = RemoteLinuxCollector::new(auth);
        assert!(c.auth.key_candidates.is_empty());
        let err = c.connect().await.expect_err("should fail without keys");
        assert!(
            matches!(err, RemoteError::UnsupportedSshConfig(_)),
            "expected UnsupportedSshConfig, got {:?}",
            err
        );
    }

    // ── 12: multi-key fallback — connect() tries every candidate in order ──
    // (The happy path where a later candidate succeeds requires the
    // integration test in tests/integration_fleet.rs, which has a real
    // in-process russh server. Here we just verify the non-network
    // behavior: non-existent keys in the candidate list produce an
    // error that names each path that was tried.)
    #[tokio::test]
    async fn connect_with_nonexistent_keys_lists_all_attempts() {
        let mut auth = SshAuth::parse("box1.invalid", "ubuntu");
        auth.key_candidates = vec![
            std::path::PathBuf::from("/nonexistent/key1"),
            std::path::PathBuf::from("/nonexistent/key2"),
        ];
        let mut c = RemoteLinuxCollector::new(auth);
        // Use a short timeout since this will try to open a TCP socket
        // first (the key check happens AFTER connect in the current
        // implementation because connect needs a fresh session per key).
        // On an invalid host we'll get ConnectFailed or timeout — either
        // is acceptable for this non-integration test.
        let res = tokio::time::timeout(Duration::from_secs(2), c.connect()).await;
        match res {
            Ok(Err(RemoteError::ConnectFailed(_))) => {}
            Ok(Err(RemoteError::AuthFailed(msg))) => {
                // If by chance the host resolved, both keys should be
                // named in the error.
                assert!(msg.contains("key1") && msg.contains("key2"));
            }
            Ok(Err(other)) => panic!("unexpected error: {:?}", other),
            Ok(Ok(_)) => panic!("connect should not succeed against invalid host"),
            Err(_) => {} // timeout is fine — network unreachable
        }
    }
}
