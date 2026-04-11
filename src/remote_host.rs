//! `RemoteHost` — per-host orchestrator that owns:
//!
//! * The persistent russh session
//! * The refresh task (5s ticker that fetches system + docker + swarm)
//! * Any active log-stream tasks (one per container / service being tailed)
//! * Any active action tasks (one per destructive command in flight)
//! * A shared snapshot of the latest data the main loop can read
//!
//! The per-host refresh task writes directly into `Arc<Mutex<RemoteHostState>>`
//! via short blocking locks (sub-millisecond). The main loop reads the
//! same mutex during render. No mpsc for the refresh path because the
//! data is structured and the lock contention is trivially bounded.
//!
//! Log streaming uses a separate mpsc channel per stream because
//! `LogViewState` contains `RefCell` (non-Send) and must stay on the
//! main thread. The stream task pushes `(container_id, line)` pairs
//! and the main loop's `poll_remote_logs` drains them into the
//! appropriate `LogViewState`.

use crate::collectors::remote::{
    ClientHandler, ConnState, REMOTE_SCRIPT, RemoteError, SshAuth, build_monitor_data_from_blob,
    parse_remote_blob,
};
use crate::layout::Layout;
use crate::model::{
    ContainerUIState, DockerContainerInfo, MonitorData, SwarmClusterInfo, SwarmNodeInfo,
    SwarmServiceInfo, SwarmStackInfo, SwarmTaskInfo, SwarmUIState, UIState,
};
use crate::remote_docker::{run_remote_command, shell_escape_id};
use russh::keys::load_secret_key;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

/// Shared state. Locked briefly by the refresh task (on write) and the
/// main loop (on read during render).
#[derive(Default)]
pub struct RemoteHostState {
    pub conn_state: ConnState,
    pub last_refresh: Option<Instant>,
    pub last_error: Option<String>,
    pub consecutive_failures: u32,

    // System tab
    pub monitor_data: Option<MonitorData>,
    /// UI state for the System tab (selection, sort column, expanded PIDs).
    /// Persisted across refreshes so cursor position / sort order don't
    /// reset every 5s. The render path clones it briefly, render mutates
    /// the clone, and the result is written back — no lock held across I/O.
    pub ui_state: UIState,
    /// Section collapse state for the System tab, same persistence story.
    pub layout: Layout,
    /// The PID the user's cursor was on last frame. Used by the render
    /// path to re-resolve `selected_index` after a refresh reorders the
    /// process list (same PID, new row index). Without this, every 5s
    /// refresh tick causes the cursor to jump to a different process on
    /// busy hosts because the selection tracks by INDEX not PID.
    pub prev_selected_pid: Option<sysinfo::Pid>,

    // Containers tab
    pub containers: Vec<DockerContainerInfo>,
    pub container_ui: ContainerUIState,
    pub status_message: Option<String>,

    // Swarm tab
    pub swarm_info: Option<SwarmClusterInfo>,
    pub swarm_nodes: Vec<SwarmNodeInfo>,
    pub swarm_services: Vec<SwarmServiceInfo>,
    pub swarm_stacks: Vec<SwarmStackInfo>,
    pub swarm_service_tasks: HashMap<String, Vec<SwarmTaskInfo>>,
    pub swarm_warnings: Vec<String>,
    pub swarm_ui: SwarmUIState,

    /// Active multi-log container IDs for the current multi-log view.
    /// Set when user opens multi-log (l/L), cleared on Esc. Used by
    /// poll_remote_logs to route lines into multi_log_state instead of
    /// per-container log_states.
    pub multi_log_container_ids: Vec<String>,

    // Previous /proc/diskstats for computing busy % deltas across ticks.
    prev_diskstats: Option<HashMap<String, u64>>,
    prev_diskstats_at: Option<Instant>,

    // Previous iface (rx, tx) for computing network rate across ticks.
    prev_iface_bytes: Option<HashMap<String, (u64, u64)>>,
    prev_iface_at: Option<Instant>,
}

impl Default for ConnState {
    fn default() -> Self {
        ConnState::Disconnected
    }
}

/// One line from a remote log stream — routed by container or service id.
#[derive(Debug, Clone)]
pub struct RemoteLogLine {
    pub stream_id: String, // container id OR service id
    pub line: String,
}

/// Result of a destructive action (start/stop/restart/rolling-restart).
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub host_index: usize,
    pub kind: String,
    pub target_id: String,
    pub success: bool,
    pub message: String,
}

/// Per-host orchestrator. One instance per host in the fleet.
pub struct RemoteHost {
    pub host_index: usize,
    pub auth: SshAuth,
    pub state: Arc<Mutex<RemoteHostState>>,

    /// SSH session. Wrapped in a Tokio mutex because connect replaces it.
    session: Arc<tokio::sync::Mutex<Option<Arc<russh::client::Handle<ClientHandler>>>>>,

    /// Active log stream tasks, keyed by `stream_id` (container or service id).
    log_tasks: Arc<Mutex<HashMap<String, AbortHandle>>>,

    /// mpsc for log lines streaming from all active log tasks on this host.
    log_tx: UnboundedSender<RemoteLogLine>,
    pub log_rx: Option<UnboundedReceiver<RemoteLogLine>>,

    /// mpsc for action results.
    action_tx: UnboundedSender<ActionResult>,
    pub action_rx: Option<UnboundedReceiver<ActionResult>>,

    /// Refresh task handle (so App::drop can abort it if needed).
    refresh_task: Option<AbortHandle>,
}

impl RemoteHost {
    pub fn new(host_index: usize, auth: SshAuth) -> Self {
        let (log_tx, log_rx) = unbounded_channel();
        let (action_tx, action_rx) = unbounded_channel();
        Self {
            host_index,
            auth,
            state: Arc::new(Mutex::new(RemoteHostState::default())),
            session: Arc::new(tokio::sync::Mutex::new(None)),
            log_tasks: Arc::new(Mutex::new(HashMap::new())),
            log_tx,
            log_rx: Some(log_rx),
            action_tx,
            action_rx: Some(action_rx),
            refresh_task: None,
        }
    }

    /// Spawn the periodic refresh task. Must be called once, typically
    /// right after construction from `App::new`. The task loops forever
    /// (until aborted), reconnecting on failure with exponential backoff.
    pub fn spawn_refresh_loop(&mut self, rt: Arc<tokio::runtime::Runtime>, interval: Duration) {
        let state = Arc::clone(&self.state);
        let session_slot = Arc::clone(&self.session);
        let auth = self.auth.clone();
        let host_name = auth.host.clone();

        let handle = rt.spawn(async move {
            let mut consecutive_failures: u32 = 0;
            loop {
                // Ensure we have a session.
                let session_opt = { session_slot.lock().await.clone() };
                let session = match session_opt {
                    Some(s) => s,
                    None => {
                        match connect_session(&auth).await {
                            Ok(s) => {
                                let arc = Arc::new(s);
                                *session_slot.lock().await = Some(Arc::clone(&arc));
                                consecutive_failures = 0;
                                tracing::info!(host = %host_name, "remote_host: connected");
                                arc
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                {
                                    let mut s = state.lock();
                                    s.conn_state = if consecutive_failures >= 3 {
                                        ConnState::Disconnected
                                    } else {
                                        ConnState::Degraded
                                    };
                                    s.last_error = Some(msg.clone());
                                    s.consecutive_failures = consecutive_failures;
                                }
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                let backoff = Duration::from_secs(
                                    (1u64 << consecutive_failures.min(5)).min(30),
                                );
                                tracing::warn!(
                                    host = %host_name,
                                    error = %msg,
                                    backoff_secs = backoff.as_secs(),
                                    "remote_host: connect failed"
                                );
                                tokio::time::sleep(backoff).await;
                                continue;
                            }
                        }
                    }
                };

                // Do one full refresh.
                match do_full_refresh(&session, &state).await {
                    Ok(()) => {
                        consecutive_failures = 0;
                        let mut s = state.lock();
                        s.conn_state = ConnState::Connected;
                        s.last_error = None;
                        s.last_refresh = Some(Instant::now());
                        s.consecutive_failures = 0;
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(host = %host_name, error = %msg, "remote_host: refresh failed");
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        {
                            let mut s = state.lock();
                            s.conn_state = if consecutive_failures >= 3 {
                                ConnState::Disconnected
                            } else {
                                ConnState::Degraded
                            };
                            s.last_error = Some(msg);
                            s.consecutive_failures = consecutive_failures;
                        }
                        // Drop the session so the next tick reconnects.
                        if consecutive_failures >= 2 {
                            *session_slot.lock().await = None;
                        }
                    }
                }

                tokio::time::sleep(interval).await;
            }
        });
        self.refresh_task = Some(handle.abort_handle());
    }

    /// Start streaming `docker logs -f` for a container. Lines are
    /// pushed through `self.log_tx` (drained by `App::poll_remote_logs`).
    /// If a stream for this container is already active, it's replaced.
    pub fn start_container_log_stream(
        &self,
        rt: Arc<tokio::runtime::Runtime>,
        container_id: String,
    ) {
        self.start_log_stream_inner(
            rt,
            container_id.clone(),
            format!(
                "docker logs -f --tail 500 --timestamps {} 2>&1",
                shell_escape_id(&container_id)
            ),
        );
    }

    /// Start streaming `docker service logs -f` for a swarm service.
    pub fn start_service_log_stream(
        &self,
        rt: Arc<tokio::runtime::Runtime>,
        service_id: String,
    ) {
        self.start_log_stream_inner(
            rt,
            service_id.clone(),
            format!(
                "docker service logs -f --tail 500 --timestamps --raw {} 2>&1",
                shell_escape_id(&service_id)
            ),
        );
    }

    fn start_log_stream_inner(
        &self,
        rt: Arc<tokio::runtime::Runtime>,
        stream_id: String,
        cmd: String,
    ) {
        // Abort any existing stream with the same id.
        self.stop_log_stream(&stream_id);

        let session_slot = Arc::clone(&self.session);
        let log_tx = self.log_tx.clone();
        let stream_id_for_task = stream_id.clone();
        let handle = rt.spawn(async move {
            let session_opt = { session_slot.lock().await.clone() };
            let Some(session) = session_opt else {
                let _ = log_tx.send(RemoteLogLine {
                    stream_id: stream_id_for_task.clone(),
                    line: "[log stream: not connected]".into(),
                });
                return;
            };

            // Use run_remote_script which opens a channel, runs `sh -s`,
            // and streams stdout. We feed it a one-liner.
            let script = format!("{}\n", cmd);
            match stream_remote_log(&session, &script, &stream_id_for_task, &log_tx).await {
                Ok(()) => {}
                Err(e) => {
                    let _ = log_tx.send(RemoteLogLine {
                        stream_id: stream_id_for_task,
                        line: format!("[log stream ended: {}]", e),
                    });
                }
            }
        });
        let mut tasks = self.log_tasks.lock();
        tasks.insert(stream_id, handle.abort_handle());
    }

    /// Stop a running log stream for a container/service id. No-op if
    /// no stream was active.
    pub fn stop_log_stream(&self, stream_id: &str) {
        let mut tasks = self.log_tasks.lock();
        if let Some(handle) = tasks.remove(stream_id) {
            handle.abort();
            tracing::info!(stream_id = %stream_id, "remote_host: log stream stopped");
        }
    }

    /// Stop ALL active log streams (called when App shuts down).
    pub fn stop_all_log_streams(&self) {
        let mut tasks = self.log_tasks.lock();
        for (_, handle) in tasks.drain() {
            handle.abort();
        }
    }

    /// Run a destructive action on the remote in a background task.
    /// The result is sent via `self.action_tx` (drained by
    /// `App::poll_remote_actions`).
    pub fn run_action(
        &self,
        rt: Arc<tokio::runtime::Runtime>,
        kind: String,
        target_id: String,
        cmd: String,
    ) {
        let session_slot = Arc::clone(&self.session);
        let action_tx = self.action_tx.clone();
        let host_index = self.host_index;
        rt.spawn(async move {
            let session_opt = { session_slot.lock().await.clone() };
            let result = match session_opt {
                None => ActionResult {
                    host_index,
                    kind: kind.clone(),
                    target_id: target_id.clone(),
                    success: false,
                    message: "not connected".into(),
                },
                Some(session) => match run_remote_command(&session, &cmd).await {
                    Ok(out) => ActionResult {
                        host_index,
                        kind,
                        target_id,
                        success: true,
                        message: out.lines().next().unwrap_or("OK").to_string(),
                    },
                    Err(e) => ActionResult {
                        host_index,
                        kind,
                        target_id,
                        success: false,
                        message: e.to_string(),
                    },
                },
            };
            let _ = action_tx.send(result);
        });
    }

    /// Build a container-level action command, shell-escaped. Helper
    /// for callers to not reinvent quoting.
    pub fn container_action_command(kind: &str, container_id: &str) -> String {
        let escaped = shell_escape_id(container_id);
        match kind {
            "start" => format!("docker start {} 2>&1", escaped),
            "stop" => format!("docker stop -t 10 {} 2>&1", escaped),
            "restart" => format!("docker restart -t 10 {} 2>&1", escaped),
            _ => format!("echo 'unknown action: {}'", kind),
        }
    }

    pub fn swarm_rolling_restart_command(service_id: &str) -> String {
        let escaped = shell_escape_id(service_id);
        format!("docker service update --force {} 2>&1", escaped)
    }
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        if let Some(ref handle) = self.refresh_task {
            handle.abort();
        }
        self.stop_all_log_streams();
    }
}

// ─── connection helper ──────────────────────────────────────────────────

async fn connect_session(auth: &SshAuth) -> Result<russh::client::Handle<ClientHandler>, RemoteError> {
    if auth.key_candidates.is_empty() {
        return Err(RemoteError::UnsupportedSshConfig(
            "no SSH key configured".into(),
        ));
    }
    let mut last_err: Option<String> = None;
    for key_path in &auth.key_candidates {
        let config = Arc::new(russh::client::Config::default());
        let handler = ClientHandler;
        let addr = format!("{}:{}", auth.host, auth.port);
        let mut session = match russh::client::connect(config, addr, handler).await {
            Ok(s) => s,
            Err(e) => return Err(RemoteError::ConnectFailed(e.to_string())),
        };
        let key_pair = match load_secret_key(key_path, None) {
            Ok(k) => k,
            Err(e) => {
                last_err = Some(format!("{}: cannot load ({})", key_path.display(), e));
                continue;
            }
        };
        match session
            .authenticate_publickey(&auth.user, Arc::new(key_pair))
            .await
        {
            Ok(true) => return Ok(session),
            Ok(false) => last_err = Some(format!("{}: rejected", key_path.display())),
            Err(e) => last_err = Some(format!("{}: {}", key_path.display(), e)),
        }
    }
    Err(RemoteError::AuthFailed(
        last_err.unwrap_or_else(|| "no key accepted".into()),
    ))
}

// ─── one refresh cycle ──────────────────────────────────────────────────

async fn do_full_refresh(
    session: &Arc<russh::client::Handle<ClientHandler>>,
    state: &Arc<Mutex<RemoteHostState>>,
) -> Result<(), RemoteError> {
    // ── SYSTEM (one SSH round-trip) ──
    //
    // We go through `run_remote_script` which expects a
    // `Handle<ClientHandler>`. Rather than duplicate the whole function
    // here, we have a thin wrapper that accepts our ClientHandler. The
    // only difference between the two handlers is identity — both
    // accept any server key — so we transmute the session by running
    // `sh -s` directly via channel_open_session ourselves.
    let blob_str = run_blob_script(session, REMOTE_SCRIPT).await?;
    let parsed = parse_remote_blob(&blob_str);

    // Compute disk busy % + per-iface network rate from deltas against
    // the previous sample. First refresh returns zero rates because
    // there's nothing to compare against.
    let now = Instant::now();
    let current_diskstats = parsed.diskstats.clone();
    let current_ifaces: HashMap<String, (u64, u64)> = parsed
        .interfaces
        .iter()
        .map(|(name, rx, tx)| (name.clone(), (*rx, *tx)))
        .collect();
    let (busy_pct, iface_rates) = {
        let mut s = state.lock();

        // Disk busy % delta
        let busy = match (&s.prev_diskstats, s.prev_diskstats_at) {
            (Some(prev), Some(prev_t)) => {
                let elapsed_ms = (now - prev_t).as_millis() as f64;
                if elapsed_ms > 0.0 {
                    let mut max_busy: f64 = 0.0;
                    for (dev, &cur) in &current_diskstats {
                        if let Some(&p) = prev.get(dev) {
                            let delta = cur.saturating_sub(p) as f64;
                            let busy = (delta / elapsed_ms * 100.0).min(100.0);
                            if busy > max_busy {
                                max_busy = busy;
                            }
                        }
                    }
                    max_busy
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };

        // Per-interface rate (bytes/sec) from delta.
        let mut rates: HashMap<String, (u64, u64)> = HashMap::new();
        if let (Some(prev), Some(prev_t)) = (&s.prev_iface_bytes, s.prev_iface_at) {
            let elapsed_s = (now - prev_t).as_secs_f64();
            if elapsed_s > 0.0 {
                for (name, (cur_rx, cur_tx)) in &current_ifaces {
                    if let Some(&(prev_rx, prev_tx)) = prev.get(name) {
                        let rx_rate = (cur_rx.saturating_sub(prev_rx) as f64 / elapsed_s) as u64;
                        let tx_rate = (cur_tx.saturating_sub(prev_tx) as f64 / elapsed_s) as u64;
                        rates.insert(name.clone(), (rx_rate, tx_rate));
                    }
                }
            }
        }

        s.prev_diskstats = Some(current_diskstats);
        s.prev_diskstats_at = Some(now);
        s.prev_iface_bytes = Some(current_ifaces);
        s.prev_iface_at = Some(now);
        (busy, rates)
    };

    let time_str = chrono::Local::now().format("%H:%M:%S").to_string();
    let core_count = 1.0; // Not available from remote; shown for context
    let monitor_data =
        build_monitor_data_from_blob(&parsed, time_str, core_count, busy_pct, &iface_rates);

    // ── CONTAINERS (three SSH round-trips, run concurrently) ──
    // These go through `run_remote_command` which accepts a
    // `Handle<ClientHandler>` — we need a wrapper for ClientHandler.
    // Since russh's session type is generic over the handler, and our
    // ClientHandler::check_server_key just returns Ok(true) like the
    // other one, we'll pass the session through a pointer-cast-free
    // path by using the blob script runner.
    //
    // Pragmatic approach: run the three docker commands ourselves via
    // `run_blob_script` (which takes the ClientHandler session).
    let containers_result = fetch_containers(session).await;

    // ── SWARM (conditional) ──
    let swarm_result = fetch_swarm(session).await;

    // ── Apply updates ──
    {
        let mut s = state.lock();
        s.monitor_data = Some(monitor_data);
        if let Ok(containers) = containers_result {
            s.containers = containers;
        }
        if let Ok(Some(swarm)) = &swarm_result {
            s.swarm_info = Some(swarm.cluster_info.clone());
            s.swarm_nodes = swarm.nodes.clone();
            s.swarm_services = swarm.services.clone();
            s.swarm_stacks = swarm.stacks.clone();
            s.swarm_service_tasks = swarm.service_tasks.clone();
            s.swarm_warnings = crate::swarm_helpers::compute_warnings(
                &swarm.nodes,
                &swarm.services,
                Some(&swarm.cluster_info),
            );
        } else if let Ok(None) = &swarm_result {
            s.swarm_info = None;
            s.swarm_nodes.clear();
            s.swarm_services.clear();
            s.swarm_stacks.clear();
            s.swarm_service_tasks.clear();
            s.swarm_warnings.clear();
        }
    }

    Ok(())
}

/// Fetch containers via the three-command parallel pattern, but using
/// our ClientHandler session. Mirrors `remote_docker::list_containers_remote`
/// but parameterized for ClientHandler.
async fn fetch_containers(
    session: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<Vec<DockerContainerInfo>, RemoteError> {
    // Running containers only. `docker ps` without --all excludes exited
    // containers (user preference — exited containers are never fetched,
    // never in memory, never rendered). See Polish iteration 2 in the plan.
    let ps_fut = run_blob_script(session, "docker ps --format '{{json .}}' 2>/dev/null\n");
    let stats_fut = run_blob_script(
        session,
        "docker stats --no-stream --format '{{json .}}' 2>/dev/null\n",
    );
    let inspect_fut = run_blob_script(
        session,
        "ids=$(docker ps -q 2>/dev/null); if [ -n \"$ids\" ]; then docker inspect $ids 2>/dev/null; else echo '[]'; fi\n",
    );
    let (ps_out, stats_out, inspect_out) = tokio::join!(ps_fut, stats_fut, inspect_fut);
    let ps = crate::remote_docker::parse_docker_ps(&ps_out?);
    let stats = crate::remote_docker::parse_docker_stats(&stats_out?);
    let inspect = crate::remote_docker::parse_docker_inspect(&inspect_out?);
    Ok(crate::remote_docker::merge_docker_rows(&ps, &stats, &inspect))
}

/// Fetch swarm state if the host is a manager. Returns None if not a
/// swarm manager (not an error).
struct SwarmResult {
    cluster_info: SwarmClusterInfo,
    nodes: Vec<SwarmNodeInfo>,
    services: Vec<SwarmServiceInfo>,
    stacks: Vec<SwarmStackInfo>,
    service_tasks: HashMap<String, Vec<SwarmTaskInfo>>,
}

async fn fetch_swarm(
    session: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<Option<SwarmResult>, RemoteError> {
    let info_out = run_blob_script(session, "docker info --format '{{json .Swarm}}' 2>/dev/null\n")
        .await?;
    let cluster_info = match crate::remote_swarm::parse_docker_info_swarm(&info_out) {
        Some(info) => info,
        None => return Ok(None),
    };

    let nodes_fut = run_blob_script(
        session,
        "docker node ls --format '{{json .}}' 2>/dev/null\n",
    );
    let services_fut = run_blob_script(
        session,
        "docker service ls --format '{{json .}}' 2>/dev/null\n",
    );
    let inspect_fut = run_blob_script(
        session,
        "ids=$(docker service ls -q 2>/dev/null); if [ -n \"$ids\" ]; then docker service inspect $ids 2>/dev/null; else echo '[]'; fi\n",
    );
    let (nodes_out, services_out, inspect_out) =
        tokio::join!(nodes_fut, services_fut, inspect_fut);
    let nodes = crate::remote_swarm::parse_docker_node_ls(&nodes_out?);
    let mut services = crate::remote_swarm::parse_docker_service_ls(&services_out?);
    let inspect = crate::remote_swarm::parse_docker_service_inspect(&inspect_out?);
    crate::remote_swarm::attach_stack_labels(&mut services, &inspect);

    let service_tasks = if services.is_empty() {
        HashMap::new()
    } else {
        let ids: Vec<String> = services.iter().map(|s| shell_escape_id(&s.id)).collect();
        let cmd = format!(
            "docker service ps --no-trunc --format '{{{{json .}}}}' {} 2>/dev/null\n",
            ids.join(" ")
        );
        let ps_out = run_blob_script(session, &cmd).await?;
        let all_tasks = crate::remote_swarm::parse_docker_service_ps(&ps_out);
        crate::remote_swarm::group_tasks_by_service(&services, &all_tasks)
    };

    let stacks = crate::swarm_helpers::build_stacks(&services);

    Ok(Some(SwarmResult {
        cluster_info,
        nodes,
        services,
        stacks,
        service_tasks,
    }))
}

/// Run a shell script on the remote via `sh -s` (stdin), return the
/// stdout as a String. ClientHandler version of `run_remote_script`.
pub async fn run_blob_script(
    session: &russh::client::Handle<ClientHandler>,
    script: &str,
) -> Result<String, RemoteError> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| RemoteError::Other(format!("channel open failed: {}", e)))?;

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

/// Stream a long-running command (e.g. `docker logs -f`) line-by-line
/// via mpsc. Runs until the channel closes or the task is aborted.
async fn stream_remote_log(
    session: &russh::client::Handle<ClientHandler>,
    script: &str,
    stream_id: &str,
    tx: &UnboundedSender<RemoteLogLine>,
) -> Result<(), RemoteError> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| RemoteError::Other(format!("channel open failed: {}", e)))?;

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

    let mut buf = Vec::new();
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { ref data } => {
                buf.extend_from_slice(data);
                // Flush complete lines to the mpsc.
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line_bytes = buf.drain(..=pos).collect::<Vec<u8>>();
                    let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1])
                        .trim_end_matches('\r')
                        .to_string();
                    if tx
                        .send(RemoteLogLine {
                            stream_id: stream_id.to_string(),
                            line,
                        })
                        .is_err()
                    {
                        // Receiver dropped → task is orphaned.
                        return Ok(());
                    }
                }
            }
            russh::ChannelMsg::ExtendedData { ref data, .. } => {
                buf.extend_from_slice(data);
            }
            russh::ChannelMsg::Close | russh::ChannelMsg::Eof => break,
            _ => {}
        }
    }
    // Flush any trailing partial line.
    if !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf).to_string();
        let _ = tx.send(RemoteLogLine {
            stream_id: stream_id.to_string(),
            line,
        });
    }
    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_action_command_start_stop_restart() {
        assert_eq!(
            RemoteHost::container_action_command("start", "abc123"),
            "docker start abc123 2>&1"
        );
        assert_eq!(
            RemoteHost::container_action_command("stop", "abc123"),
            "docker stop -t 10 abc123 2>&1"
        );
        assert_eq!(
            RemoteHost::container_action_command("restart", "abc123"),
            "docker restart -t 10 abc123 2>&1"
        );
    }

    #[test]
    fn container_action_command_escapes_malicious_ids() {
        let cmd = RemoteHost::container_action_command("restart", "abc; rm -rf /");
        // Malicious content MUST be inside single quotes so the shell
        // treats it as a single literal argument, not a chained command.
        assert!(cmd.contains("'abc; rm -rf /'"));
        // Sanity: the command starts with the expected prefix and the
        // `;` and ` rm` are NOT at the top level (always preceded by `'`).
        assert!(cmd.starts_with("docker restart -t 10 '"));
        // No unquoted `; rm` sequence.
        let unquoted_part = cmd.split_once('\'').unwrap().0;
        assert!(!unquoted_part.contains("; rm"));
    }

    #[test]
    fn swarm_rolling_restart_command_escapes_service_id() {
        let cmd = RemoteHost::swarm_rolling_restart_command("svc; evil");
        assert!(cmd.starts_with("docker service update --force '"));
        assert!(cmd.contains("'svc; evil'"));
    }

    #[test]
    fn remote_host_new_starts_disconnected() {
        let auth = SshAuth::parse("test@example.com", "root");
        let host = RemoteHost::new(0, auth);
        let state = host.state.lock();
        assert!(matches!(state.conn_state, ConnState::Disconnected));
        assert!(state.monitor_data.is_none());
        assert!(state.containers.is_empty());
    }

    #[test]
    fn remote_host_log_line_roundtrip() {
        let line = RemoteLogLine {
            stream_id: "abc".into(),
            line: "hello from remote".into(),
        };
        assert_eq!(line.stream_id, "abc");
    }
}
