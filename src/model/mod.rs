// Re-export all model types from submodules for backward compatibility.

pub use app::{AppView, RemoteTab};
pub use docker::{
    ContainerUIState, DockerContainerInfo, LogViewState, MultiLogLine, MultiLogViewState,
};
pub use swarm::{
    ServiceLogState, SwarmClusterInfo, SwarmMode, SwarmNodeInfo, SwarmServiceInfo,
    SwarmStackInfo, SwarmTaskInfo, SwarmUIState, SwarmViewLevel,
};
pub use system::{
    ContextSwitchInfo, DiskSpaceInfo, FdInfo, MemoryInfo, MonitorData, NetworkInfo,
    NetworkInterfaceInfo, NetworkProcessInfo, ProcessGroup, ProcessInfo,
    SocketOverviewInfo, SortColumn, UIState,
};
pub use fleet::{FleetProcessRow, FleetState, FleetVitals, HostEntry, HostStatus};

mod app;
mod docker;
mod swarm;
mod system;
mod fleet;
