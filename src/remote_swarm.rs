//! Docker Swarm data collection over SSH.
//!
//! The local `SwarmMonitor` shells out to the `docker` CLI directly via
//! std::process::Command. For remote hosts we send the same commands
//! over russh and parse the same JSON output, producing the same model
//! structs. The `swarm_helpers` module provides the warning computation
//! shared between local and remote.
//!
//! Commands run per refresh (if the host is a swarm manager):
//!
//! ```text
//! docker info --format '{{json .Swarm}}'             # active?
//! docker node ls --format '{{json .}}'
//! docker service ls --format '{{json .}}'
//! docker service inspect $(docker service ls -q)     # stack labels
//! docker service ps --no-trunc --format '{{json .}}' $svc  (per service)
//! ```

use crate::collectors::remote::{ClientHandler, RemoteError};
use crate::model::{
    SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo, SwarmTaskInfo,
};
use crate::remote_docker::run_remote_command;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

// ─── JSON shapes ────────────────────────────────────────────────────────

/// The full `docker service inspect` output is enormous — we only want
/// the ID + stack label.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceInspectEntry {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Spec")]
    pub spec: ServiceInspectSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceInspectSpec {
    #[serde(rename = "Name")]
    #[serde(default)]
    pub name: String,
    #[serde(rename = "Labels")]
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Shape of `docker info --format '{{json .Swarm}}'`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DockerInfoSwarm {
    #[serde(rename = "NodeID")]
    #[serde(default)]
    pub node_id: String,
    #[serde(rename = "NodeAddr")]
    #[serde(default)]
    pub node_addr: String,
    #[serde(rename = "LocalNodeState")]
    #[serde(default)]
    pub local_node_state: String, // "active" if in swarm
    #[serde(rename = "ControlAvailable")]
    #[serde(default)]
    pub control_available: bool,
    #[serde(rename = "Managers")]
    #[serde(default)]
    pub managers: u32,
    #[serde(rename = "Nodes")]
    #[serde(default)]
    pub nodes: u32,
}

// ─── pure parsers ───────────────────────────────────────────────────────

/// Parse `docker info --format '{{json .Swarm}}'` into an active-or-not
/// `SwarmClusterInfo`. Returns `None` if the remote isn't in a swarm or
/// isn't a manager.
pub fn parse_docker_info_swarm(stdout: &str) -> Option<SwarmClusterInfo> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return None;
    }
    let info: DockerInfoSwarm = serde_json::from_str(trimmed).ok()?;
    if info.local_node_state != "active" || !info.control_available {
        return None;
    }
    Some(SwarmClusterInfo {
        node_id: info.node_id,
        node_addr: info.node_addr,
        is_manager: info.control_available,
        managers: info.managers,
        nodes_total: info.nodes,
    })
}

/// Parse `docker node ls --format '{{json .}}'`.
pub fn parse_docker_node_ls(stdout: &str) -> Vec<SwarmNodeInfo> {
    parse_jsonl(stdout)
}

/// Parse `docker service ls --format '{{json .}}'`.
pub fn parse_docker_service_ls(stdout: &str) -> Vec<SwarmServiceInfo> {
    parse_jsonl(stdout)
}

/// Parse `docker service inspect` (JSON array).
pub fn parse_docker_service_inspect(stdout: &str) -> Vec<ServiceInspectEntry> {
    serde_json::from_str::<Vec<ServiceInspectEntry>>(stdout.trim()).unwrap_or_default()
}

/// Parse `docker service ps --format '{{json .}}' <svc>`.
pub fn parse_docker_service_ps(stdout: &str) -> Vec<SwarmTaskInfo> {
    parse_jsonl(stdout)
}

/// Generic JSONL parser — one JSON object per line, malformed lines
/// are skipped with a log warning.
fn parse_jsonl<T: for<'de> Deserialize<'de>>(stdout: &str) -> Vec<T> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<T>(line) {
            rows.push(row);
        }
    }
    rows
}

/// Attach stack labels from `service inspect` output to the `stack`
/// field of each `SwarmServiceInfo`.
///
/// Primary: look up `com.docker.stack.namespace` in `Spec.Labels`.
/// Fallback: if the label is missing, derive the stack from the service
/// name by splitting on the first underscore. Docker Stack names all
/// services `<stack>_<service>`, so `video-generator_worker` → stack
/// `video-generator`. This handles production swarms where the label
/// is either set on `TaskTemplate.ContainerSpec.Labels` (not
/// `Spec.Labels`) or missing entirely on older deployments.
///
/// Services with no label AND no underscore in the name keep an empty
/// stack and end up in the "(no stack)" bucket by `build_stacks`.
pub fn attach_stack_labels(
    services: &mut [SwarmServiceInfo],
    inspect: &[ServiceInspectEntry],
) {
    let stacks: HashMap<&str, &str> = inspect
        .iter()
        .filter_map(|e| {
            e.spec
                .labels
                .get("com.docker.stack.namespace")
                .map(|s| (e.id.as_str(), s.as_str()))
        })
        .collect();
    for svc in services.iter_mut() {
        if let Some(stack) = stacks.get(svc.id.as_str()) {
            svc.stack = stack.to_string();
        } else if let Some((prefix, _)) = svc.name.split_once('_') {
            svc.stack = prefix.to_string();
        }
    }
}

// ─── async SSH wrappers ─────────────────────────────────────────────────

/// Check whether the remote host is a swarm manager. Returns the
/// cluster info if yes, None otherwise. Non-error on a non-swarm host.
pub async fn is_swarm_active_remote(
    session: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<Option<SwarmClusterInfo>, RemoteError> {
    let stdout = run_remote_command(
        session,
        "docker info --format '{{json .Swarm}}' 2>/dev/null",
    )
    .await?;
    Ok(parse_docker_info_swarm(&stdout))
}

/// Fetch nodes, services (with stack labels attached), and tasks-per-service
/// concurrently. Only call this if the host is confirmed to be a swarm
/// manager (i.e. after `is_swarm_active_remote` returned Some).
pub async fn refresh_swarm_remote(
    session: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<SwarmRemoteRefresh, RemoteError> {
    let nodes_fut = run_remote_command(
        session,
        "docker node ls --format '{{json .}}' 2>/dev/null",
    );
    let services_fut = run_remote_command(
        session,
        "docker service ls --format '{{json .}}' 2>/dev/null",
    );
    // Kick off inspect in parallel — it's the heaviest but gives us
    // stack labels for all services in one round-trip.
    let inspect_fut = run_remote_command(
        session,
        "ids=$(docker service ls -q 2>/dev/null); if [ -n \"$ids\" ]; then docker service inspect $ids 2>/dev/null; else echo '[]'; fi",
    );

    let (nodes_out, services_out, inspect_out) =
        tokio::join!(nodes_fut, services_fut, inspect_fut);

    let nodes = parse_docker_node_ls(&nodes_out?);
    let mut services = parse_docker_service_ls(&services_out?);
    let inspect = parse_docker_service_inspect(&inspect_out?);
    attach_stack_labels(&mut services, &inspect);

    // Per-service tasks: batch one call for all services.
    let service_tasks = if services.is_empty() {
        HashMap::new()
    } else {
        let ids: Vec<String> = services
            .iter()
            .map(|s| crate::remote_docker::shell_escape_id(&s.id))
            .collect();
        // `docker service ps svc1 svc2 …` outputs tasks from all named
        // services in one call. We use the per-task "Name" field
        // ("servicename.N") to route tasks back to their parent service.
        let cmd = format!(
            "docker service ps --no-trunc --format '{{{{json .}}}}' {} 2>/dev/null",
            ids.join(" ")
        );
        let ps_out = run_remote_command(session, &cmd).await?;
        let all_tasks = parse_docker_service_ps(&ps_out);
        group_tasks_by_service(&services, &all_tasks)
    };

    let stacks = crate::swarm_helpers::build_stacks(&services);

    Ok(SwarmRemoteRefresh {
        nodes,
        services,
        stacks,
        service_tasks,
    })
}

/// Group a flat task list into `ServiceId → Vec<SwarmTaskInfo>`.
/// `docker service ps` returns task names like `"prod_web.2"` — we
/// match the prefix against service names to route.
pub fn group_tasks_by_service(
    services: &[SwarmServiceInfo],
    tasks: &[SwarmTaskInfo],
) -> HashMap<String, Vec<SwarmTaskInfo>> {
    let mut by_service: HashMap<String, Vec<SwarmTaskInfo>> = HashMap::new();
    for svc in services {
        by_service.insert(svc.id.clone(), Vec::new());
    }
    for task in tasks {
        // Task name format: "servicename.replica_number" or
        // "servicename.node_id.task_id" for global mode.
        let svc_name = task.name.split('.').next().unwrap_or("");
        if let Some(svc) = services.iter().find(|s| s.name == svc_name) {
            by_service
                .entry(svc.id.clone())
                .or_default()
                .push(task.clone());
        }
    }
    by_service
}

/// Aggregated output of one remote swarm refresh.
pub struct SwarmRemoteRefresh {
    pub nodes: Vec<SwarmNodeInfo>,
    pub services: Vec<SwarmServiceInfo>,
    pub stacks: Vec<SwarmStackInfo>,
    pub service_tasks: HashMap<String, Vec<SwarmTaskInfo>>,
}

/// Rolling-restart a service by running `docker service update --force`
/// on the remote. Returns Ok(()) if the command exited 0. The caller is
/// responsible for shell-escaping the service id before passing.
pub async fn force_update_service_remote(
    session: &Arc<russh::client::Handle<ClientHandler>>,
    service_id: &str,
) -> Result<(), RemoteError> {
    let escaped = crate::remote_docker::shell_escape_id(service_id);
    let cmd = format!("docker service update --force {} 2>&1", escaped);
    let _ = run_remote_command(session, &cmd).await?;
    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/docker/{}", name)).unwrap()
    }

    #[test]
    fn parse_docker_info_swarm_active_manager() {
        let input = r#"{"NodeID":"abc","NodeAddr":"10.0.0.1","LocalNodeState":"active","ControlAvailable":true,"Managers":1,"Nodes":3}"#;
        let info = parse_docker_info_swarm(input).expect("should parse");
        assert_eq!(info.managers, 1);
        assert_eq!(info.nodes_total, 3);
        assert!(info.is_manager);
    }

    #[test]
    fn parse_docker_info_swarm_inactive_returns_none() {
        let input = r#"{"LocalNodeState":"inactive","ControlAvailable":false}"#;
        assert!(parse_docker_info_swarm(input).is_none());
    }

    #[test]
    fn parse_docker_info_swarm_non_manager_returns_none() {
        // Worker node — LocalNodeState active but ControlAvailable false.
        let input = r#"{"LocalNodeState":"active","ControlAvailable":false,"Managers":1,"Nodes":3}"#;
        assert!(parse_docker_info_swarm(input).is_none());
    }

    #[test]
    fn parse_docker_info_swarm_empty_returns_none() {
        assert!(parse_docker_info_swarm("").is_none());
        assert!(parse_docker_info_swarm("null").is_none());
    }

    #[test]
    fn parse_docker_node_ls_happy_path() {
        let nodes = parse_docker_node_ls(&fixture("node_ls.jsonl"));
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].hostname, "mgr-1");
        assert_eq!(nodes[0].manager_status, "Leader");
        let down = nodes.iter().find(|n| n.status == "Down").unwrap();
        assert_eq!(down.hostname, "worker-2");
        let drain = nodes
            .iter()
            .find(|n| n.availability == "Drain")
            .unwrap();
        assert_eq!(drain.hostname, "worker-3");
    }

    #[test]
    fn parse_docker_service_ls_happy_path() {
        let services = parse_docker_service_ls(&fixture("service_ls.jsonl"));
        assert_eq!(services.len(), 4);
        let web = services.iter().find(|s| s.name == "prod_web").unwrap();
        assert_eq!(web.replicas, "3/3");
        let worker = services.iter().find(|s| s.name == "prod_worker").unwrap();
        assert_eq!(worker.replicas, "2/5"); // degraded
    }

    #[test]
    fn parse_docker_service_inspect_extracts_stack_labels() {
        let entries = parse_docker_service_inspect(&fixture("service_inspect.json"));
        assert_eq!(entries.len(), 4);
        let prod_web = entries.iter().find(|e| e.id == "svc111").unwrap();
        assert_eq!(
            prod_web.spec.labels.get("com.docker.stack.namespace"),
            Some(&"prod".to_string())
        );
        let metrics = entries.iter().find(|e| e.id == "svc444").unwrap();
        assert!(metrics.spec.labels.get("com.docker.stack.namespace").is_none());
    }

    #[test]
    fn attach_stack_labels_populates_service_stack_field() {
        let mut services = parse_docker_service_ls(&fixture("service_ls.jsonl"));
        let inspect = parse_docker_service_inspect(&fixture("service_inspect.json"));
        attach_stack_labels(&mut services, &inspect);
        let web = services.iter().find(|s| s.name == "prod_web").unwrap();
        assert_eq!(web.stack, "prod");
        // `metrics` has no label AND no underscore in its name → stays empty.
        let metrics = services.iter().find(|s| s.name == "metrics").unwrap();
        assert_eq!(metrics.stack, "");
    }

    #[test]
    fn attach_stack_labels_falls_back_to_name_prefix_when_label_missing() {
        // Simulates the production swarm where `Spec.Labels` is empty
        // but the service names clearly belong to a stack. The fallback
        // derives the stack from the service name's first underscore.
        let mut services = vec![
            SwarmServiceInfo {
                id: "svc1".into(),
                name: "video-generator_worker".into(),
                mode: "replicated".into(),
                replicas: "3/3".into(),
                image: "worker:latest".into(),
                ports: String::new(),
                stack: String::new(),
            },
            SwarmServiceInfo {
                id: "svc2".into(),
                name: "video-generator_redis".into(),
                mode: "replicated".into(),
                replicas: "1/1".into(),
                image: "redis:7".into(),
                ports: String::new(),
                stack: String::new(),
            },
            SwarmServiceInfo {
                id: "svc3".into(),
                name: "standalone".into(),
                mode: "replicated".into(),
                replicas: "1/1".into(),
                image: "alpine".into(),
                ports: String::new(),
                stack: String::new(),
            },
        ];
        // Empty inspect → no labels found → fallback kicks in.
        attach_stack_labels(&mut services, &[]);
        assert_eq!(services[0].stack, "video-generator");
        assert_eq!(services[1].stack, "video-generator");
        assert_eq!(services[2].stack, ""); // no underscore → stays empty
    }

    #[test]
    fn attach_stack_labels_label_takes_precedence_over_name_fallback() {
        // If the service has BOTH a label and an underscore name, the
        // label wins — this prevents breaking services whose name
        // happens to contain an underscore but belong to a different
        // stack (unusual but defensive).
        let mut services = vec![SwarmServiceInfo {
            id: "svc1".into(),
            name: "otherstack_worker".into(),
            mode: "replicated".into(),
            replicas: "1/1".into(),
            image: "x".into(),
            ports: String::new(),
            stack: String::new(),
        }];
        let inspect = vec![ServiceInspectEntry {
            id: "svc1".into(),
            spec: ServiceInspectSpec {
                name: "otherstack_worker".into(),
                labels: {
                    let mut m = HashMap::new();
                    m.insert("com.docker.stack.namespace".into(), "explicit_label".into());
                    m
                },
            },
        }];
        attach_stack_labels(&mut services, &inspect);
        // Label wins — not the name prefix.
        assert_eq!(services[0].stack, "explicit_label");
    }

    #[test]
    fn parse_docker_service_ps_happy_path() {
        let tasks = parse_docker_service_ps(&fixture("service_ps.jsonl"));
        assert_eq!(tasks.len(), 4);
        let failed = tasks
            .iter()
            .find(|t| t.current_state.contains("Failed"))
            .unwrap();
        assert!(failed.error.contains("non-zero"));
    }

    #[test]
    fn group_tasks_by_service_routes_by_name_prefix() {
        let services = vec![SwarmServiceInfo {
            id: "svc111".into(),
            name: "prod_web".into(),
            mode: "replicated".into(),
            replicas: "3/3".into(),
            image: "nginx".into(),
            ports: String::new(),
            stack: "prod".into(),
        }];
        let tasks = parse_docker_service_ps(&fixture("service_ps.jsonl"));
        let grouped = group_tasks_by_service(&services, &tasks);
        assert_eq!(grouped.get("svc111").unwrap().len(), 4);
    }

    #[test]
    fn integration_compute_warnings_against_fixtures() {
        // Exercises the full remote-swarm → warning pipeline.
        let nodes = parse_docker_node_ls(&fixture("node_ls.jsonl"));
        let services = parse_docker_service_ls(&fixture("service_ls.jsonl"));
        let info = SwarmClusterInfo {
            node_id: "n1".into(),
            node_addr: "10.0.0.1".into(),
            is_manager: true,
            managers: 1,
            nodes_total: 4,
        };
        let warnings = crate::swarm_helpers::compute_warnings(&nodes, &services, Some(&info));
        // Expected warnings:
        //   - NODE DOWN (worker-2)
        //   - DRAINED (worker-3)
        //   - SERVICE DEGRADED (prod_worker 2/5)
        //   - LOW MANAGERS (1 of 4)
        assert_eq!(warnings.len(), 4);
        assert!(warnings.iter().any(|w| w.contains("NODE DOWN")));
        assert!(warnings.iter().any(|w| w.contains("DRAINED")));
        assert!(warnings.iter().any(|w| w.contains("SERVICE DEGRADED")));
        assert!(warnings.iter().any(|w| w.contains("LOW MANAGERS")));
    }
}
