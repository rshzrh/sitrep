//! Pure (no-I/O, no-state) helpers shared by the local `SwarmMonitor`
//! and the remote `RemoteHost` swarm path. Extracted so both code paths
//! compute warnings / stack groupings / task state the same way.
//!
//! The local path calls these on data read from `docker` CLI subprocess
//! JSON output; the remote path calls them on data read from `docker`
//! CLI over SSH. The inputs are the same Vec<SwarmNodeInfo> / etc., so
//! the logic only needs to live in one place.

use crate::model::{SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo};

/// Group services by stack label. Services with an empty stack name
/// are bucketed under `(no stack)`, which is sorted to the end.
///
/// Returns `SwarmStackInfo` records pointing into the original services
/// slice via indices (mirrors the in-place field used by `SwarmMonitor`).
pub fn build_stacks(services: &[SwarmServiceInfo]) -> Vec<SwarmStackInfo> {
    use std::collections::HashMap;
    let mut stack_map: HashMap<String, Vec<usize>> = HashMap::new();

    for (i, svc) in services.iter().enumerate() {
        let stack_name = if svc.stack.is_empty() {
            "(no stack)".to_string()
        } else {
            svc.stack.clone()
        };
        stack_map.entry(stack_name).or_default().push(i);
    }

    let mut stacks: Vec<SwarmStackInfo> = stack_map
        .into_iter()
        .map(|(name, indices)| SwarmStackInfo {
            name,
            service_indices: indices,
        })
        .collect();

    stacks.sort_by(|a, b| {
        if a.name == "(no stack)" {
            std::cmp::Ordering::Greater
        } else if b.name == "(no stack)" {
            std::cmp::Ordering::Less
        } else {
            a.name.cmp(&b.name)
        }
    });

    stacks
}

/// Compute cluster-health warnings from the current node + service state.
///
/// Returns a Vec of human-readable warning strings, one per detected
/// problem. Empty Vec means "all green". Rules (matches sitrep's
/// pre-existing behavior):
///
///   * **NODE DOWN** — one or more nodes have status containing "down"
///   * **DRAINED** — one or more nodes have availability "drain"
///   * **SERVICE DEGRADED** — service's `replicas` is "N/M" with N < M
///   * **LOW MANAGERS** — cluster has <3 managers but >3 total nodes
pub fn compute_warnings(
    nodes: &[SwarmNodeInfo],
    services: &[SwarmServiceInfo],
    cluster_info: Option<&SwarmClusterInfo>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Down nodes.
    let down_nodes: Vec<&str> = nodes
        .iter()
        .filter(|n| n.status.to_lowercase().contains("down"))
        .map(|n| n.hostname.as_str())
        .collect();
    if !down_nodes.is_empty() {
        warnings.push(format!(
            "NODE DOWN: {} node(s) unreachable: {}",
            down_nodes.len(),
            down_nodes.join(", ")
        ));
    }

    // Drained nodes.
    let drain_nodes: Vec<&str> = nodes
        .iter()
        .filter(|n| n.availability.to_lowercase().contains("drain"))
        .map(|n| n.hostname.as_str())
        .collect();
    if !drain_nodes.is_empty() {
        warnings.push(format!(
            "DRAINED: {} node(s) in drain mode: {}",
            drain_nodes.len(),
            drain_nodes.join(", ")
        ));
    }

    // Degraded services (fewer running replicas than desired).
    for svc in services {
        if let Some((current, desired)) = parse_replicas(&svc.replicas) {
            if desired > 0 && current < desired {
                warnings.push(format!(
                    "SERVICE DEGRADED: {} has {}/{} replicas",
                    svc.name, current, desired
                ));
            }
        }
    }

    // Low manager count for a cluster big enough to warrant it.
    if let Some(info) = cluster_info {
        if info.managers < 3 && info.nodes_total > 3 {
            warnings.push(format!(
                "LOW MANAGERS: Only {} manager(s) for {} nodes (recommend 3+)",
                info.managers, info.nodes_total
            ));
        }
    }

    warnings
}

/// Parse a replica-count string of the form `"N/M"` into `(current, desired)`.
/// Returns None for global-mode services or malformed strings.
pub fn parse_replicas(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('/')?;
    let current: u32 = a.trim().parse().ok()?;
    let desired: u32 = b.trim().parse().ok()?;
    Some((current, desired))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SwarmClusterInfo, SwarmNodeInfo, SwarmServiceInfo};

    fn node(hostname: &str, status: &str, availability: &str) -> SwarmNodeInfo {
        SwarmNodeInfo {
            id: format!("id-{}", hostname),
            hostname: hostname.to_string(),
            status: status.to_string(),
            availability: availability.to_string(),
            manager_status: String::new(),
            engine_version: "25.0".to_string(),
            is_self: false,
            ip_address: String::new(),
        }
    }

    fn svc(name: &str, replicas: &str, stack: &str) -> SwarmServiceInfo {
        SwarmServiceInfo {
            id: format!("id-{}", name),
            name: name.to_string(),
            mode: "replicated".to_string(),
            replicas: replicas.to_string(),
            image: "app:1".to_string(),
            ports: String::new(),
            stack: stack.to_string(),
        }
    }

    #[test]
    fn parse_replicas_handles_standard_and_edge_cases() {
        assert_eq!(parse_replicas("3/3"), Some((3, 3)));
        assert_eq!(parse_replicas("1/5"), Some((1, 5)));
        assert_eq!(parse_replicas("0/0"), Some((0, 0)));
        assert_eq!(parse_replicas("global"), None);
        assert_eq!(parse_replicas("garbage"), None);
        assert_eq!(parse_replicas(""), None);
    }

    #[test]
    fn compute_warnings_empty_fleet_has_no_warnings() {
        assert!(compute_warnings(&[], &[], None).is_empty());
    }

    #[test]
    fn compute_warnings_detects_down_node() {
        let nodes = vec![node("box1", "Down", "Active")];
        let w = compute_warnings(&nodes, &[], None);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("NODE DOWN"));
        assert!(w[0].contains("box1"));
    }

    #[test]
    fn compute_warnings_detects_drained_node() {
        let nodes = vec![node("box2", "Ready", "Drain")];
        let w = compute_warnings(&nodes, &[], None);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("DRAINED"));
    }

    #[test]
    fn compute_warnings_detects_degraded_service() {
        let services = vec![svc("web", "2/3", "prod")];
        let w = compute_warnings(&[], &services, None);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("SERVICE DEGRADED"));
        assert!(w[0].contains("web"));
        assert!(w[0].contains("2/3"));
    }

    #[test]
    fn compute_warnings_ignores_global_mode_services() {
        let mut services = vec![svc("metrics", "global", "prod")];
        services[0].mode = "global".into();
        assert!(compute_warnings(&[], &services, None).is_empty());
    }

    fn cluster(nodes_total: u32, managers: u32) -> SwarmClusterInfo {
        SwarmClusterInfo {
            node_id: "n1".into(),
            node_addr: "10.0.0.1".into(),
            is_manager: true,
            managers,
            nodes_total,
        }
    }

    #[test]
    fn compute_warnings_detects_low_manager_count() {
        let info = cluster(5, 1);
        let w = compute_warnings(&[], &[], Some(&info));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("LOW MANAGERS"));
    }

    #[test]
    fn compute_warnings_small_cluster_doesnt_flag_low_managers() {
        // 1 manager on a 2-node cluster is fine (too small to warrant 3).
        let info = cluster(2, 1);
        assert!(compute_warnings(&[], &[], Some(&info)).is_empty());
    }

    #[test]
    fn build_stacks_groups_by_stack_label_with_unstacked_last() {
        let services = vec![
            svc("web", "1/1", "prod"),
            svc("db", "1/1", "prod"),
            svc("lonely", "1/1", ""),
            svc("worker", "1/1", "staging"),
        ];
        let stacks = build_stacks(&services);
        assert_eq!(stacks.len(), 3);
        assert_eq!(stacks[0].name, "prod");
        assert_eq!(stacks[1].name, "staging");
        assert_eq!(stacks[2].name, "(no stack)");
        assert_eq!(stacks[0].service_indices.len(), 2);
        assert_eq!(stacks[2].service_indices.len(), 1);
    }
}
