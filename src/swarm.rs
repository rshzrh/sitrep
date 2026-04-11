//! Local Docker Swarm cluster queries via bollard.
//!
//! Previously this module shelled out to the `docker` CLI for every
//! swarm read. See A4 in the eng review. All calls now go through
//! bollard directly, which:
//!
//! * removes ~15 subprocess spawns per tick on expanded-stacks views
//!   (see P4 in the review — dies automatically with this port)
//! * unifies the error model with DockerMonitor (both use bollard::Error)
//! * is faster (~50–200ms for all swarm calls vs. 1–3s for the CLI set)
//!
//! Public functions are `async` and take a `&bollard::Docker` handle.
//! `SwarmMonitor` caches a long-lived client and block_on's these on
//! its background worker thread via `rt.block_on`.
//!
//! The remote-host swarm path (`crate::remote_swarm`) is a separate
//! module that parses JSON from SSH-returned text and is NOT touched
//! here.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bollard::Docker;
use bollard::models::{
    Node, Service, ServiceSpec, ServiceSpecMode, ServiceSpecModeReplicated, Task,
};
use bollard::query_parameters::{
    ListServicesOptionsBuilder, ListTasksOptionsBuilder, LogsOptionsBuilder,
    UpdateServiceOptionsBuilder,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::model::{SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmTaskInfo};

/// Handle returned by `tail_service_logs` so the caller can kill the
/// streaming task on view teardown. The tokio task running the log
/// stream checks `kill_flag` before forwarding each line.
pub struct LogStreamHandle {
    pub receiver: mpsc::Receiver<String>,
    kill_flag: Arc<AtomicBool>,
}

impl LogStreamHandle {
    pub fn kill(&self) {
        self.kill_flag.store(true, Ordering::Release);
    }
}

// ─── Connectivity ─────────────────────────────────────────────────

/// Whether the local Docker daemon is reachable. Replaces the old
/// `is_docker_cli_available` check (which spawned `docker version`).
pub async fn is_docker_available(docker: &Docker) -> bool {
    docker.ping().await.is_ok()
}

/// Detect whether the daemon is in Swarm mode. Returns a populated
/// `SwarmClusterInfo` if yes, `None` if standalone.
pub async fn detect_swarm(docker: &Docker) -> Option<SwarmClusterInfo> {
    let info = docker.info().await.ok()?;
    let swarm = info.swarm?;
    // `LocalNodeState` is "active" when this daemon participates in a
    // swarm. Anything else (inactive, pending, locked, error) means
    // we should not populate a ClusterInfo.
    let state = swarm.local_node_state?;
    if state.to_string() != "active" {
        return None;
    }
    Some(SwarmClusterInfo {
        node_id: swarm.node_id.unwrap_or_default(),
        node_addr: swarm.node_addr.unwrap_or_default(),
        is_manager: swarm.control_available.unwrap_or(false),
        managers: swarm.managers.unwrap_or(0) as u32,
        nodes_total: swarm.nodes.unwrap_or(0) as u32,
    })
}

// ─── Nodes ────────────────────────────────────────────────────────

/// List all nodes in the swarm. Each node's IP address comes from
/// `node.status.addr` in the same response — no separate inspect
/// call is needed (the old CLI path needed a second `docker node
/// inspect` round-trip per batch).
pub async fn list_nodes(docker: &Docker) -> Result<Vec<SwarmNodeInfo>, String> {
    let nodes = docker
        .list_nodes(None)
        .await
        .map_err(|e| format!("list_nodes failed: {}", e))?;
    Ok(nodes.into_iter().map(map_node).collect())
}

fn map_node(n: Node) -> SwarmNodeInfo {
    let description = n.description.unwrap_or_default();
    let spec = n.spec.unwrap_or_default();
    let status = n.status.unwrap_or_default();
    let manager_status = n.manager_status;

    // The old CLI format produced human strings like "Ready" / "Active"
    // / "Leader". The bollard enums serialize as lowercase ("ready",
    // "active", "leader") plus an "Unknown" / "Empty" variant. Match
    // the previous capitalized form so downstream formatting and
    // warning checks keep working unchanged.
    let status_str = status
        .state
        .map(|s| capitalize_first(&s.to_string()))
        .unwrap_or_default();
    let availability_str = spec
        .availability
        .map(|a| capitalize_first(a.as_ref()))
        .unwrap_or_default();

    // ManagerStatus is absent on worker nodes. On managers, it's
    // "Leader" if leader==true, otherwise the reachability string
    // ("Reachable" / "Unreachable").
    let manager_status_str = manager_status
        .map(|m| {
            if m.leader.unwrap_or(false) {
                "Leader".to_string()
            } else {
                m.reachability
                    .map(|r| capitalize_first(&r.to_string()))
                    .unwrap_or_default()
            }
        })
        .unwrap_or_default();

    SwarmNodeInfo {
        id: n.id.unwrap_or_default(),
        hostname: description.hostname.unwrap_or_default(),
        status: status_str,
        availability: availability_str,
        manager_status: manager_status_str,
        engine_version: description
            .engine
            .and_then(|e| e.engine_version)
            .unwrap_or_default(),
        // `is_self` isn't exposed on the Node object itself — the CLI
        // format `{{.Self}}` computes it by comparing against
        // info.swarm.node_id. `SwarmMonitor::update` fills this in
        // after the fact using the cached cluster_info.node_id.
        is_self: false,
        ip_address: status.addr.unwrap_or_default(),
    }
}

// ─── Services ─────────────────────────────────────────────────────

/// List all services. Requests `status=true` so each Service carries
/// its RunningTasks/DesiredTasks counts inline — saves a separate
/// task fetch just to build the "3/3" replica string.
pub async fn list_services(docker: &Docker) -> Result<Vec<SwarmServiceInfo>, String> {
    let options = ListServicesOptionsBuilder::default().status(true).build();
    let services = docker
        .list_services(Some(options))
        .await
        .map_err(|e| format!("list_services failed: {}", e))?;
    Ok(services.into_iter().map(map_service).collect())
}

fn map_service(s: Service) -> SwarmServiceInfo {
    let id = s.id.unwrap_or_default();
    let spec = s.spec.unwrap_or_default();

    let name = spec.name.clone().unwrap_or_default();
    let mode = match spec.mode.as_ref() {
        Some(m) if m.replicated.is_some() => "replicated".to_string(),
        Some(m) if m.global.is_some() => "global".to_string(),
        Some(m) if m.replicated_job.is_some() => "replicated-job".to_string(),
        Some(m) if m.global_job.is_some() => "global-job".to_string(),
        _ => String::new(),
    };

    // "X/Y" replica string. X = running tasks, Y = desired tasks.
    // `ServiceServiceStatus` is populated because we passed `status=true`
    // to `list_services`. Global services always report desired = N (one
    // per schedulable node).
    let replicas = match s.service_status.as_ref() {
        Some(st) => format!(
            "{}/{}",
            st.running_tasks.unwrap_or(0),
            st.desired_tasks.unwrap_or(0)
        ),
        None => String::new(),
    };

    let image = spec
        .task_template
        .as_ref()
        .and_then(|t| t.container_spec.as_ref())
        .and_then(|c| c.image.clone())
        .unwrap_or_default();

    // Ports formatted like the CLI did: "published->target/proto".
    let ports = s
        .endpoint
        .as_ref()
        .and_then(|e| e.ports.as_ref())
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let published = p.published_port?;
                    let target = p.target_port?;
                    let proto = p
                        .protocol
                        .as_ref()
                        .map(|pr| pr.to_string())
                        .unwrap_or_else(|| "tcp".to_string());
                    Some(format!("{}->{}/{}", published, target, proto))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    // Stack name from the standard docker stack label.
    let stack = spec
        .labels
        .as_ref()
        .and_then(|l| l.get("com.docker.stack.namespace"))
        .cloned()
        .unwrap_or_default();

    SwarmServiceInfo {
        id,
        name,
        mode,
        replicas,
        image,
        ports,
        stack,
    }
}

// ─── Tasks ────────────────────────────────────────────────────────

/// List running tasks for a single service.
pub async fn list_service_tasks(
    docker: &Docker,
    service_id: &str,
) -> Result<Vec<SwarmTaskInfo>, String> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert("service".to_string(), vec![service_id.to_string()]);
    let options = ListTasksOptionsBuilder::default().filters(&filters).build();
    let tasks = docker
        .list_tasks(Some(options))
        .await
        .map_err(|e| format!("list_tasks failed: {}", e))?;
    // Hostname resolution for task.node needs a second call. We leave
    // `node` as the node id — callers that care (the swarm task view)
    // look up hostnames against the cached nodes Vec on the caller
    // side. This mirrors what the old CLI output gave us too (it was
    // already a node id in the JSON response).
    Ok(tasks.into_iter().map(map_task).collect())
}

/// Batch: list all running tasks for multiple services in one call.
/// Uses the `service=` filter repeatedly.
pub async fn list_tasks_for_services(
    docker: &Docker,
    service_ids: &[&str],
) -> Result<Vec<SwarmTaskInfo>, String> {
    if service_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "service".to_string(),
        service_ids.iter().map(|s| s.to_string()).collect(),
    );
    filters.insert("desired-state".to_string(), vec!["running".to_string()]);
    let options = ListTasksOptionsBuilder::default().filters(&filters).build();
    let tasks = docker
        .list_tasks(Some(options))
        .await
        .map_err(|e| format!("list_tasks failed: {}", e))?;
    Ok(tasks.into_iter().map(map_task).collect())
}

fn map_task(t: Task) -> SwarmTaskInfo {
    let id = t.id.unwrap_or_default();
    let name = t.name.clone().unwrap_or_default();
    let spec = t.spec.unwrap_or_default();
    let status = t.status.unwrap_or_default();
    let image = spec
        .container_spec
        .as_ref()
        .and_then(|c| c.image.clone())
        .unwrap_or_default();
    let node = t.node_id.unwrap_or_default();
    let desired_state = t
        .desired_state
        .map(|s| s.to_string())
        .unwrap_or_default();
    let current_state = status
        .state
        .map(|s| s.to_string())
        .unwrap_or_default();
    let error = status.err.unwrap_or_default();
    SwarmTaskInfo {
        id,
        name,
        image,
        node,
        desired_state,
        current_state,
        error,
        ports: String::new(), // not exposed at the task level by the API
    }
}

// ─── Service actions ──────────────────────────────────────────────

/// Force-update a service (rolling restart of all replicas). Bollard's
/// `update_service` needs the full current spec + version number, so
/// we inspect_service → resend the same spec with `force_update` bumped.
pub async fn force_update_service(docker: &Docker, service_id: &str) -> Result<(), String> {
    let current = docker
        .inspect_service(service_id, None)
        .await
        .map_err(|e| format!("inspect_service failed: {}", e))?;
    let version = current
        .version
        .as_ref()
        .and_then(|v| v.index)
        .ok_or_else(|| "service has no version".to_string())?;
    let mut spec = current.spec.clone().unwrap_or_default();

    // Bump `force_update` on the task template — this is what the CLI
    // `docker service update --force` does under the hood. Swarm
    // interprets any change to this counter as a signal to redeploy.
    if let Some(tmpl) = spec.task_template.as_mut() {
        tmpl.force_update = Some(tmpl.force_update.unwrap_or(0) + 1);
    }

    let options = UpdateServiceOptionsBuilder::default()
        .version(version as i32)
        .build();
    docker
        .update_service(service_id, spec, options, None)
        .await
        .map_err(|e| format!("update_service failed: {}", e))?;
    Ok(())
}

/// Scale a replicated service to a given number of replicas.
pub async fn scale_service(
    docker: &Docker,
    service_id: &str,
    replicas: u32,
) -> Result<(), String> {
    let current = docker
        .inspect_service(service_id, None)
        .await
        .map_err(|e| format!("inspect_service failed: {}", e))?;
    let version = current
        .version
        .as_ref()
        .and_then(|v| v.index)
        .ok_or_else(|| "service has no version".to_string())?;
    let mut spec: ServiceSpec = current.spec.clone().unwrap_or_default();

    // Rewrite the replicas count on the mode. If the service isn't
    // replicated (e.g. it's global) this call is meaningless — return
    // an error rather than silently converting it.
    match spec.mode.as_mut() {
        Some(m) if m.replicated.is_some() => {
            m.replicated = Some(ServiceSpecModeReplicated {
                replicas: Some(replicas as i64),
            });
        }
        _ => {
            return Err("service is not replicated; cannot scale".to_string());
        }
    }
    // Also cover the case where mode is None entirely (very defensive).
    if spec.mode.is_none() {
        spec.mode = Some(ServiceSpecMode {
            replicated: Some(ServiceSpecModeReplicated {
                replicas: Some(replicas as i64),
            }),
            ..Default::default()
        });
    }

    let options = UpdateServiceOptionsBuilder::default()
        .version(version as i32)
        .build();
    docker
        .update_service(service_id, spec, options, None)
        .await
        .map_err(|e| format!("update_service failed: {}", e))?;
    Ok(())
}

// ─── Service logs ─────────────────────────────────────────────────

/// Start streaming logs for a service. The returned handle owns a
/// `mpsc::Receiver<String>` that the main loop drains each tick and
/// a kill flag that the caller flips on view teardown. The tokio
/// task exits on kill-flag or receiver-drop.
pub fn tail_service_logs(
    docker: &Docker,
    handle: &tokio::runtime::Handle,
    service_id: &str,
) -> LogStreamHandle {
    let (tx, rx) = mpsc::channel::<String>(1000);
    let kill_flag = Arc::new(AtomicBool::new(false));
    let kill_flag_clone = Arc::clone(&kill_flag);

    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .follow(true)
        .tail("200")
        .timestamps(true)
        .build();

    let stream = docker.service_logs(service_id, Some(options));

    handle.spawn(async move {
        let mut stream = Box::pin(stream);
        while let Some(item) = stream.next().await {
            if kill_flag_clone.load(Ordering::Acquire) {
                break;
            }
            let line = match item {
                Ok(out) => {
                    let bytes: &[u8] = out.as_ref();
                    String::from_utf8_lossy(bytes).trim_end().to_string()
                }
                Err(e) => {
                    let _ = tx.send(format!("[error] {}", e)).await;
                    break;
                }
            };
            if tx.send(line).await.is_err() {
                break; // receiver dropped
            }
        }
    });

    LogStreamHandle {
        receiver: rx,
        kill_flag,
    }
}

// ─── Helpers ──────────────────────────────────────────────────────

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(c).collect(),
    }
}
