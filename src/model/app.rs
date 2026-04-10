/// App-level view state
#[derive(Clone, Debug, PartialEq)]
pub enum AppView {
    /// Fleet overview — N hosts as rows. Only used when flotop is invoked
    /// with one or more host arguments.
    FleetOverview,
    /// Drill-in to a remote host. The `tab` field mirrors the local
    /// tab set — System / Containers / Logs / Swarm. Each variant is
    /// rendered using the SAME view functions as the local tabs, just
    /// against the remote host's cached data.
    Remote {
        host: usize,
        tab: RemoteTab,
    },
    System,
    Containers,
    ContainerLogs(String),                     // container ID
    ContainerLogsMulti(Vec<(String, String)>), // Vec of (container_id, container_name)
    Swarm,                                     // Swarm cluster view
    SwarmServiceTasks(String, String),         // (service_id, service_name)
    SwarmServiceLogs(String, String),          // (service_id, service_name)
}

/// Which tab the user is currently looking at within a remote drill-in.
/// Mirrors the local tab set.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteTab {
    System,
    Containers,
    ContainerLogs(String),                      // container id
    ContainerLogsMulti(Vec<(String, String)>),  // (container_id, container_name) pairs
    Swarm,
    SwarmServiceTasks(String, String),          // service_id, service_name
    SwarmServiceLogs(String, String),
}
