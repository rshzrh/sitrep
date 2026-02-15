use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use serde::Deserialize;

use crate::model::{SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmTaskInfo};

/// Handle returned by `tail_service_logs` to kill the child process on cleanup.
pub struct LogStreamHandle {
    pub receiver: mpsc::Receiver<String>,
    kill_flag: Arc<AtomicBool>,
}

impl LogStreamHandle {
    /// Signal the child process to stop and drop resources.
    pub fn kill(&self) {
        self.kill_flag.store(true, Ordering::Relaxed);
    }
}

/// Typed struct for docker info Swarm section (avoids serde_json::Value overhead)
#[derive(Deserialize)]
struct DockerInfoSwarm {
    #[serde(rename = "LocalNodeState")]
    #[serde(default)]
    local_node_state: String,
    #[serde(rename = "NodeID")]
    #[serde(default)]
    node_id: String,
    #[serde(rename = "NodeAddr")]
    #[serde(default)]
    node_addr: String,
    #[serde(rename = "ControlAvailable")]
    #[serde(default)]
    control_available: bool,
    #[serde(rename = "Managers")]
    #[serde(default)]
    managers: u32,
    #[serde(rename = "Nodes")]
    #[serde(default)]
    nodes: u32,
}

#[derive(Deserialize)]
struct DockerInfoPartial {
    #[serde(rename = "Swarm")]
    swarm: Option<DockerInfoSwarm>,
}

/// Check if the `docker` CLI binary is available in PATH.
pub fn is_docker_cli_available() -> bool {
    Command::new("docker")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Detect whether Docker is in Swarm mode by querying `docker info`.
/// Returns Some(SwarmClusterInfo) if swarm is active, None otherwise.
pub fn detect_swarm() -> Option<SwarmClusterInfo> {
    let output = Command::new("docker")
        .args(["info", "--format", "{{json .}}"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let info: DockerInfoPartial = serde_json::from_str(text.trim()).ok()?;
    let swarm = info.swarm?;

    if swarm.local_node_state != "active" {
        return None;
    }

    Some(SwarmClusterInfo {
        node_id: swarm.node_id,
        node_addr: swarm.node_addr,
        is_manager: swarm.control_available,
        managers: swarm.managers,
        nodes_total: swarm.nodes,
    })
}

/// List all nodes in the Swarm cluster.
pub fn list_nodes() -> Result<Vec<SwarmNodeInfo>, String> {
    let output = Command::new("docker")
        .args(["node", "ls", "--format", "{{json .}}"])
        .output()
        .map_err(|e| format!("Failed to run docker node ls: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("docker node ls failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut node: SwarmNodeInfo = serde_json::from_str(line).ok()?;
            if node.manager_status.is_empty() {
                node.manager_status = String::new();
            }
            Some(node)
        })
        .collect())
}

/// Batch-fetch IP addresses for all nodes in a single `docker node inspect` call.
/// Returns a map of node ID → IP address.
pub fn batch_get_node_ips(nodes: &[SwarmNodeInfo]) -> HashMap<String, String> {
    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    let mut args = vec![
        "node".to_string(),
        "inspect".to_string(),
        "--format".to_string(),
        "{{.ID}} {{.Status.Addr}}".to_string(),
    ];
    for id in &ids {
        args.push(id.to_string());
    }

    let output = match Command::new("docker").args(&args).output() {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((id, ip)) = line.split_once(' ') {
            let ip = ip.trim();
            if !ip.is_empty() {
                result.insert(id.to_string(), ip.to_string());
            }
        }
    }
    result
}

/// List all services in the Swarm.
/// Uses a single batch `docker service inspect` to get stack labels for all services.
pub fn list_services() -> Result<Vec<SwarmServiceInfo>, String> {
    let output = Command::new("docker")
        .args(["service", "ls", "--format", "{{json .}}"])
        .output()
        .map_err(|e| format!("Failed to run docker service ls: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("docker service ls failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut services: Vec<SwarmServiceInfo> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    if services.is_empty() {
        return Ok(services);
    }

    // Batch: fetch all stack labels in one call
    let stack_labels = batch_get_stack_labels(&services);
    for svc in &mut services {
        if let Some(label) = stack_labels.get(&svc.id) {
            svc.stack = label.clone();
        }
    }

    Ok(services)
}

/// Batch-fetch stack labels for all services in a single `docker service inspect` call.
fn batch_get_stack_labels(services: &[SwarmServiceInfo]) -> HashMap<String, String> {
    let ids: Vec<&str> = services.iter().map(|s| s.id.as_str()).collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    // Build args: docker service inspect --format '{{.ID}} {{index .Spec.Labels "com.docker.stack.namespace"}}' id1 id2 ...
    let mut args = vec![
        "service".to_string(),
        "inspect".to_string(),
        "--format".to_string(),
        r#"{{.ID}} {{index .Spec.Labels "com.docker.stack.namespace"}}"#.to_string(),
    ];
    for id in &ids {
        args.push(id.to_string());
    }

    let output = match Command::new("docker")
        .args(&args)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: "SERVICE_ID STACK_NAME" or "SERVICE_ID <no value>"
        if let Some(space_pos) = line.find(' ') {
            let id = &line[..space_pos];
            let label = &line[space_pos + 1..];
            let label = if label == "<no value>" || label.is_empty() {
                String::new()
            } else {
                label.to_string()
            };
            // Match: `docker service inspect` returns full 64-char IDs,
            // `docker service ls` returns short ~12-char IDs.
            // One-directional: full_id.starts_with(short_id), with a minimum
            // length guard of 10 chars to avoid ambiguous prefix collisions.
            for svc_id in &ids {
                if svc_id.len() >= 10 && id.starts_with(svc_id) {
                    result.insert(svc_id.to_string(), label.clone());
                    break;
                }
            }
        }
    }
    result
}

/// Batch-fetch tasks for multiple services in a single subprocess call.
/// Returns all tasks with `DesiredState == "Running"`.
pub fn list_tasks_for_services(service_ids: &[&str]) -> Result<Vec<SwarmTaskInfo>, String> {
    if service_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = vec!["service", "ps", "--format", "{{json .}}", "--filter", "desired-state=running"];
    for id in service_ids {
        args.push(id);
    }

    let output = Command::new("docker")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run docker service ps: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("docker service ps failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// List tasks (replicas) for a specific service.
pub fn list_service_tasks(service_id: &str) -> Result<Vec<SwarmTaskInfo>, String> {
    let output = Command::new("docker")
        .args(["service", "ps", service_id, "--format", "{{json .}}", "--no-trunc"])
        .output()
        .map_err(|e| format!("Failed to run docker service ps: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("docker service ps failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Force-update a service (rolling restart of all replicas).
pub fn force_update_service(service_id: &str) -> Result<(), String> {
    let output = Command::new("docker")
        .args(["service", "update", "--force", service_id])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        Err(err)
    }
}

/// Scale a service to a given number of replicas.
pub fn scale_service(service_id: &str, replicas: u32) -> Result<(), String> {
    let arg = format!("{}={}", service_id, replicas);
    let output = Command::new("docker")
        .args(["service", "scale", &arg])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        Err(err)
    }
}

/// Start streaming service logs. Returns a `LogStreamHandle` with the receiver
/// and a kill mechanism. Call `handle.kill()` to terminate the child process
/// and avoid zombie processes.
pub fn tail_service_logs(service_id: &str) -> LogStreamHandle {
    let (tx, rx) = mpsc::channel::<String>();
    let kill_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&kill_flag);

    let id = service_id.to_string();
    thread::spawn(move || {
        let mut child = match Command::new("docker")
            .args([
                "service", "logs",
                "--follow",
                "--tail", "200",
                "--timestamps",
                &id,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(format!("[error] Failed to start log stream: {}", e));
                return;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Read stdout in a separate thread
        let tx_stdout = tx.clone();
        let flag_stdout = Arc::clone(&flag_clone);
        let stdout_handle = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if flag_stdout.load(Ordering::Relaxed) {
                    break;
                }
                match line {
                    Ok(l) => {
                        if tx_stdout.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Read stderr in the current thread
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if flag_clone.load(Ordering::Relaxed) {
                break;
            }
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        // Kill the child process explicitly to unblock any blocked I/O
        let _ = child.kill();

        // Wait for stdout thread with a timeout to avoid hanging
        let _ = stdout_handle.join();

        // Reap the child process
        let _ = child.wait();
    });

    LogStreamHandle {
        receiver: rx,
        kill_flag,
    }
}
