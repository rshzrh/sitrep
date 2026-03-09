/// App-level view state
#[derive(Clone, Debug, PartialEq)]
pub enum AppView {
    System,
    Containers,
    ContainerLogs(String),                     // container ID
    ContainerLogsMulti(Vec<(String, String)>), // Vec of (container_id, container_name)
    Swarm,                                     // Swarm cluster view
    SwarmServiceTasks(String, String),         // (service_id, service_name)
    SwarmServiceLogs(String, String),          // (service_id, service_name)
}
