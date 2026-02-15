/// App-level view state
#[derive(Clone, Debug, PartialEq)]
pub enum AppView {
    System,
    Containers,
    ContainerLogs(String), // container ID
    Swarm,                 // Swarm cluster view
    SwarmServiceTasks(String, String), // (service_id, service_name)
    SwarmServiceLogs(String, String),  // (service_id, service_name)
}
