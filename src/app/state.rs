use std::time::Instant;

use crate::swarm_controller::SwarmMonitor;

/// Pending destructive action awaiting confirmation.
pub struct PendingAction {
    pub description: String,
    pub kind: PendingActionKind,
    pub expires: Instant,
}

pub enum PendingActionKind {
    ContainerStart(String),
    ContainerStop(String),
    ContainerRestart(String),
    SwarmRollingRestart(String),
}

/// What kind of item is at a given row index in the Swarm overview.
pub enum SwarmOverviewItem {
    NodesHeader,
    Node,
    StackHeader(String),
    Service(String, String), // (service_id, service_name)
    None,
}

/// Resolve which item is at the given row index in the Swarm overview.
pub fn resolve_swarm_overview_item(monitor: &SwarmMonitor, selected: usize) -> SwarmOverviewItem {
    let mut row_idx: usize = 0;

    if selected == row_idx {
        return SwarmOverviewItem::NodesHeader;
    }
    row_idx += 1;

    if monitor.ui_state.expanded_ids.contains("__nodes__") {
        for _node in &monitor.nodes {
            if selected == row_idx {
                return SwarmOverviewItem::Node;
            }
            row_idx += 1;
        }
    }

    for stack in &monitor.stacks {
        if selected == row_idx {
            return SwarmOverviewItem::StackHeader(stack.name.clone());
        }
        row_idx += 1;

        if monitor.ui_state.expanded_ids.contains(&stack.name) {
            for &idx in &stack.service_indices {
                if selected == row_idx {
                    let svc = &monitor.services[idx];
                    return SwarmOverviewItem::Service(svc.id.clone(), svc.name.clone());
                }
                row_idx += 1;
            }
        }
    }

    SwarmOverviewItem::None
}

#[cfg(test)]
mod tests {
    use crate::model::{SwarmNodeInfo, SwarmServiceInfo, SwarmStackInfo};
    use crate::swarm_controller::SwarmMonitor;

    use super::{resolve_swarm_overview_item, SwarmOverviewItem};

    fn make_monitor(
        nodes: Vec<SwarmNodeInfo>,
        services: Vec<SwarmServiceInfo>,
        stacks: Vec<SwarmStackInfo>,
        expanded: &[&str],
    ) -> SwarmMonitor {
        let expanded_ids = expanded.iter().map(|s| (*s).to_string()).collect();
        SwarmMonitor::new_minimal(nodes, services, stacks, expanded_ids)
    }

    #[test]
    fn resolve_swarm_overview_nodes_header() {
        let monitor = make_monitor(vec![], vec![], vec![], &[]);
        assert!(matches!(
            resolve_swarm_overview_item(&monitor, 0),
            SwarmOverviewItem::NodesHeader
        ));
    }

    #[test]
    fn resolve_swarm_overview_node_when_expanded() {
        let nodes = vec![
            SwarmNodeInfo {
                id: "n1".into(),
                hostname: "node1".into(),
                ..Default::default()
            },
        ];
        let monitor = make_monitor(nodes, vec![], vec![], &["__nodes__"]);
        assert!(matches!(
            resolve_swarm_overview_item(&monitor, 1),
            SwarmOverviewItem::Node
        ));
    }

    #[test]
    fn resolve_swarm_overview_stack_header() {
        let stacks = vec![SwarmStackInfo {
            name: "mystack".into(),
            service_indices: vec![],
        }];
        let monitor = make_monitor(vec![], vec![], stacks, &[]);
        assert!(matches!(
            resolve_swarm_overview_item(&monitor, 1),
            SwarmOverviewItem::StackHeader(name) if name == "mystack"
        ));
    }

    #[test]
    fn resolve_swarm_overview_service_when_stack_expanded() {
        let services = vec![SwarmServiceInfo {
            id: "svc-id".into(),
            name: "my-service".into(),
            ..Default::default()
        }];
        let stacks = vec![SwarmStackInfo {
            name: "mystack".into(),
            service_indices: vec![0],
        }];
        let monitor = make_monitor(vec![], services, stacks, &["mystack"]);
        assert!(matches!(
            resolve_swarm_overview_item(&monitor, 2),
            SwarmOverviewItem::Service(id, name) if id == "svc-id" && name == "my-service"
        ));
    }

    #[test]
    fn resolve_swarm_overview_none_for_out_of_range() {
        let monitor = make_monitor(vec![], vec![], vec![], &[]);
        assert!(matches!(
            resolve_swarm_overview_item(&monitor, 100),
            SwarmOverviewItem::None
        ));
    }
}
