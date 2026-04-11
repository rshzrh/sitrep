/// Top-level tab identity.
///
/// `AppView` is richer than this — it carries drill-in state like
/// "which container's logs am I viewing". `TabKind` is the coarser
/// "which top-level tab does this belong to", which is the thing
/// dispatch code actually wants most of the time. One match site
/// maps `AppView → TabKind`; everything else matches on `TabKind`.
/// See A1 + A2-lite in the eng review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    System,
    Containers,
    Swarm,
    Fleet,
    Remote,
}

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

impl AppView {
    /// Map any `AppView` variant to its top-level tab kind. This is
    /// the single source of truth for "which top-level tab is this";
    /// all dispatch logic (tick refresh, tab-switch refresh, active
    /// monitor selection) routes through this method instead of
    /// re-listing the variant→tab mapping.
    pub fn tab_kind(&self) -> TabKind {
        match self {
            AppView::System => TabKind::System,
            AppView::Containers
            | AppView::ContainerLogs(_)
            | AppView::ContainerLogsMulti(_) => TabKind::Containers,
            AppView::Swarm
            | AppView::SwarmServiceTasks(_, _)
            | AppView::SwarmServiceLogs(_, _) => TabKind::Swarm,
            AppView::FleetOverview => TabKind::Fleet,
            AppView::Remote { .. } => TabKind::Remote,
        }
    }
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
