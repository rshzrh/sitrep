use serde::Deserialize;
use std::collections::HashSet;
use std::collections::VecDeque;

/// Whether we're running standalone Docker or in a Swarm cluster
#[derive(Clone, Debug, PartialEq)]
pub enum SwarmMode {
    Standalone,
    Swarm,
}

/// Cluster-level overview
#[derive(Clone, Debug, Default)]
pub struct SwarmClusterInfo {
    pub node_id: String,
    pub node_addr: String,
    pub is_manager: bool,
    pub managers: u32,
    pub nodes_total: u32,
}

/// A single Swarm node
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmNodeInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "Status")]
    pub status: String,         // "Ready", "Down"
    #[serde(rename = "Availability")]
    pub availability: String,   // "Active", "Pause", "Drain"
    #[serde(rename = "ManagerStatus")]
    #[serde(default)]
    pub manager_status: String, // "Leader", "Reachable", ""
    #[serde(rename = "EngineVersion")]
    #[serde(default)]
    pub engine_version: String,
    #[serde(rename = "Self")]
    #[serde(default)]
    pub is_self: bool,
}

/// A Swarm service
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmServiceInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Mode")]
    #[serde(default)]
    pub mode: String,           // "replicated", "global"
    #[serde(rename = "Replicas")]
    #[serde(default)]
    pub replicas: String,       // "3/3"
    #[serde(rename = "Image")]
    #[serde(default)]
    pub image: String,
    #[serde(rename = "Ports")]
    #[serde(default)]
    pub ports: String,
    // Derived: stack name from label com.docker.stack.namespace
    #[serde(skip)]
    pub stack: String,
}

/// A Swarm task (replica of a service)
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SwarmTaskInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Image")]
    #[serde(default)]
    pub image: String,
    #[serde(rename = "Node")]
    #[serde(default)]
    pub node: String,
    #[serde(rename = "DesiredState")]
    #[serde(default)]
    pub desired_state: String,
    #[serde(rename = "CurrentState")]
    #[serde(default)]
    pub current_state: String,
    #[serde(rename = "Error")]
    #[serde(default)]
    pub error: String,
    #[serde(rename = "Ports")]
    #[serde(default)]
    pub ports: String,
}

/// Stack: a group of services deployed together (uses indices into SwarmMonitor.services)
#[derive(Clone, Debug)]
pub struct SwarmStackInfo {
    pub name: String,
    pub service_indices: Vec<usize>,
}

/// What level of the Swarm drill-down we're viewing
#[derive(Clone, Debug, PartialEq)]
pub enum SwarmViewLevel {
    Overview,                        // nodes + stacks/services
    ServiceTasks(String, String),    // (service_id, service_name) -> task list
    ServiceLogs(String, String),     // (service_id, service_name) -> log viewer
}

/// UI state for the Swarm tab
pub struct SwarmUIState {
    pub view_level: SwarmViewLevel,
    pub selected_index: usize,
    pub expanded_ids: HashSet<String>,
}

impl Default for SwarmUIState {
    fn default() -> Self {
        Self {
            view_level: SwarmViewLevel::Overview,
            selected_index: 0,
            expanded_ids: HashSet::new(),
        }
    }
}

/// Log viewer state for service logs
pub struct ServiceLogState {
    pub service_id: String,
    pub service_name: String,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize,
    pub auto_follow: bool,
    pub filter_errors: bool,
    pub search_mode: bool,
    pub search_query: String,
}

impl ServiceLogState {
    pub fn new(service_id: String, service_name: String) -> Self {
        Self {
            service_id,
            service_name,
            lines: VecDeque::with_capacity(10000),
            scroll_offset: 0,
            auto_follow: true,
            filter_errors: false,
            search_mode: false,
            search_query: String::new(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.lines.len() >= 10000 {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_log_state_push_line_caps_at_10000() {
        let mut state = ServiceLogState::new("svc1".into(), "my-service".into());
        for i in 0..10010 {
            state.push_line(format!("log {}", i));
        }
        assert_eq!(state.lines.len(), 10000);
        assert_eq!(state.lines.front(), Some(&"log 10".to_string()));
        assert_eq!(state.lines.back(), Some(&"log 10009".to_string()));
    }

    #[test]
    fn swarm_node_info_deserialize() {
        let json = r#"{"ID":"abc123","Hostname":"node1","Status":"Ready","Availability":"Active","ManagerStatus":"Leader","EngineVersion":"24.0","Self":true}"#;
        let node: SwarmNodeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "abc123");
        assert_eq!(node.hostname, "node1");
        assert_eq!(node.status, "Ready");
        assert!(node.is_self);
    }

    #[test]
    fn swarm_ui_state_default() {
        let state = SwarmUIState::default();
        assert!(matches!(state.view_level, SwarmViewLevel::Overview));
        assert_eq!(state.selected_index, 0);
        assert!(state.expanded_ids.is_empty());
    }
}
