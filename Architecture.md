# Sitrep Architecture

A detailed technical document describing the architecture, data flow, and component responsibilities of the sitrep TUI diagnostic tool.

---

## Table of Contents

1. [High-Level Architecture](#1-high-level-architecture)
2. [Component Hierarchy](#2-component-hierarchy)
3. [Main Event Loop](#3-main-event-loop)
4. [Data Flow by Scenario](#4-data-flow-by-scenario)
5. [Sequence Diagrams](#5-sequence-diagrams)
6. [Refresh & Polling Strategy](#6-refresh--polling-strategy)
7. [Component Responsibility Matrix](#7-component-responsibility-matrix)

---

## 1. High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              main.rs (Entry Point)                              │
│  • Panic hook (restore terminal)                                                │
│  • SIGTERM/SIGINT handlers → should_quit                                        │
│  • app::run(should_quit)                                                        │
└─────────────────────────────────────────────────────────────────────────────────┘
                                        │
                                        ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              app (Application Layer)                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │  event_loop │  │   input     │  │   render    │  │   state     │             │
│  │  • tick     │  │  • keys     │  │  • dispatch │  │  • pending  │             │
│  │  • poll     │  │  • tabs     │  │  • views    │  │  • resolve  │             │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘             │
└─────────────────────────────────────────────────────────────────────────────────┘
                                        │
                    ┌───────────────────┼───────────────────┐
                    ▼                   ▼                   ▼
┌───────────────────────┐ ┌───────────────────────┐ ┌───────────────────────┐
│   controller/         │ │  docker_controller    │ │  swarm_controller     │
│   (System Monitor)    │ │  (Docker Monitor)     │ │  (Swarm Monitor)      │
│   • sysinfo           │ │  • bollard API        │ │  • docker CLI         │
│   • collectors        │ │  • containers         │ │  • nodes/services     │
└───────────────────────┘ └───────────────────────┘ └───────────────────────┘
                    │                   │                   │
                    ▼                   ▼                   ▼
┌───────────────────────┐ ┌───────────────────────┐ ┌───────────────────────┐
│   collectors/         │ │  docker (bollard)     │ │  swarm (CLI)          │
│   • mac / linux       │ │  • list_containers    │ │  • list_nodes         │
│   • OS-specific I/O   │ │  • tail_logs          │ │  • list_services      │
└───────────────────────┘ └───────────────────────┘ └───────────────────────┘
                                        │
                                        ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              view (Presentation Layer)                          │
│  • Presenter (tab_bar, system, containers, swarm, logs, confirmation)           │
│  • Reads from monitors' data + UI state                                         │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Component Hierarchy

```
src/
├── main.rs                    # Thin entry point
├── lib.rs                     # Library root, re-exports
│
├── app/                       # Application orchestration
│   ├── mod.rs                 # App struct, run(), restore_terminal()
│   ├── event_loop.rs          # Tick, poll_logs, poll_actions, refresh_on_tab_switch
│   ├── input.rs               # Key handling, view-specific handlers
│   ├── render.rs              # Render dispatch by AppView
│   └── state.rs               # PendingAction, SwarmOverviewItem, resolve_swarm_overview_item
│
├── model/                     # Data structures (no logic)
│   ├── app.rs                 # AppView enum
│   ├── system.rs              # MonitorData, UIState, ProcessGroup, etc.
│   ├── docker.rs              # DockerContainerInfo, LogViewState, ContainerUIState
│   └── swarm.rs               # SwarmNodeInfo, SwarmServiceInfo, SwarmUIState, etc.
│
├── view/                      # Terminal rendering
│   ├── mod.rs                 # Presenter, RowKind
│   ├── tab_bar.rs             # Tab bar rendering
│   ├── system.rs              # System report (sections, processes)
│   ├── containers.rs          # Container list
│   ├── swarm.rs               # Swarm overview, tasks
│   ├── logs.rs                # Container + service logs
│   ├── confirmation.rs        # Pending action prompt
│   └── shared.rs              # truncate_str, safe_truncate, progress_bar
│
├── controller/                # System data collection
│   ├── mod.rs                 # Monitor, update()
│   └── process.rs             # build_live_groups, compute_top_processes
│
├── docker_controller.rs        # Docker container + log management
├── swarm_controller.rs         # Swarm cluster + service management
│
├── docker.rs                   # Bollard API wrapper (async)
├── swarm.rs                    # Docker CLI wrapper (sync subprocess)
│
├── layout.rs                   # Section layout (collapsible)
│
└── collectors/                 # OS-specific system metrics
    ├── mod.rs                 # SystemCollector trait
    ├── mac.rs                 # macOS: iostat, nettop, lsof, sysctl
    └── linux.rs               # Linux: /proc, ss
```

---

## 3. Main Event Loop

The main loop in `app::run()` follows this flow every iteration:

```mermaid
flowchart TD
    subgraph "Per-Iteration (≈100ms poll timeout)"
        A[Check should_quit] --> B{Quit?}
        B -->|Yes| EXIT[restore_terminal, exit]
        B -->|No| C[expire_pending_action]
        C --> D[process_tick]
        D --> E[poll_logs]
        E --> F[poll_actions]
        F --> G[refresh_on_tab_switch]
        G --> H{needs_render?}
        H -->|Yes| I[render_size_guard]
        I --> J{Terminal OK?}
        J -->|No| K[Show resize message, continue]
        J -->|Yes| L[render]
        L --> M{pending_action?}
        M -->|Yes| N[render_confirmation]
        M -->|No| O[needs_render = false]
        N --> O
        H -->|No| P[poll for key]
        O --> P
        P --> Q{Key event?}
        Q -->|Yes| R[handle_key]
        R --> S{Quit?}
        S -->|Yes| EXIT
        S -->|Consumed| T[needs_render = true]
        S -->|None| U[continue]
        Q -->|No| U
        T --> U
        U --> A
    end
```

### Loop Timing

| Phase | Trigger | Interval |
|-------|---------|----------|
| **Tick** | `process_tick()` | Every 3 seconds (only active view's monitor) |
| **Log poll** | `poll_logs()` | Every loop (~100ms) when in log view |
| **Action poll** | `poll_actions()` | Every loop when action in progress |
| **Tab switch** | `refresh_on_tab_switch()` | On view change, min 500ms between refreshes |
| **Key poll** | `crossterm::event::poll` | Up to 100ms or remaining tick time |

---

## 4. Data Flow by Scenario

### 4.1 System View

```mermaid
flowchart LR
    subgraph "Data Sources"
        SYS[sysinfo]
        COL[collectors]
    end

    subgraph "Controller"
        MON[Monitor]
    end

    subgraph "Model"
        MD[MonitorData]
        UI[UIState]
        LAY[Layout]
    end

    subgraph "View"
        PRES[Presenter]
    end

    SYS --> MON
    COL --> MON
    MON --> MD
    MON --> UI
    MON --> LAY
    MD --> PRES
    UI --> PRES
    LAY --> PRES
    PRES --> STDOUT[stdout]
```

**Data ownership:**
- `Monitor` owns: `last_data` (MonitorData), `ui_state`, `layout`
- `Monitor::update()`: sysinfo refresh → collectors → process grouping → `last_data`
- `Presenter::render()`: reads `last_data`, mutates `ui_state` (expansion), returns `row_mapping`

---

### 4.2 Containers View

```mermaid
flowchart LR
    subgraph "Data Source"
        DOCK[Docker daemon]
    end

    subgraph "Client"
        DC[DockerClient]
    end

    subgraph "Controller"
        DM[DockerMonitor]
    end

    subgraph "Model"
        CONT[containers]
        CUI[ContainerUIState]
    end

    subgraph "View"
        PRES[Presenter]
    end

    DOCK --> DC
    DC --> DM
    DM --> CONT
    DM --> CUI
    CONT --> PRES
    CUI --> PRES
    PRES --> STDOUT[stdout]
```

**Data flow:**
- `DockerMonitor::update()`: `DockerClient::list_containers()` + `get_all_cpu_percents()` → `containers`
- `Presenter::render_containers()`: reads `containers`, `ui_state`, `status_message`

---

### 4.3 Container Logs View

```mermaid
flowchart TB
    subgraph "Background"
        DOCK[Docker daemon]
        TASK[Tokio task]
    end

    subgraph "Channels"
        TX[mpsc::Sender]
        RX[mpsc::Receiver]
    end

    subgraph "Controller"
        DM[DockerMonitor]
    end

    subgraph "Model"
        LOG[LogViewState]
    end

    subgraph "View"
        PRES[Presenter]
    end

    DOCK -->|logs stream| TASK
    TASK -->|lines| TX
    TX --> RX
    RX -->|poll_logs| DM
    DM -->|push_line| LOG
    LOG --> PRES
    PRES --> STDOUT[stdout]
```

**Data flow:**
- `DockerMonitor::start_log_stream()`: spawns tokio task, returns `mpsc::Receiver`
- `poll_logs()`: drains receiver (up to 100 lines) into `LogViewState.lines`
- `Presenter::render_logs()`: reads `log_state` (lines, scroll_offset, etc.)

---

### 4.4 Swarm Overview

```mermaid
flowchart LR
    subgraph "Data Source"
        CLI[docker CLI]
    end

    subgraph "Swarm Module"
        SW[swarm::list_nodes]
        SW2[swarm::list_services]
    end

    subgraph "Controller"
        SM[SwarmMonitor]
    end

    subgraph "Model"
        NODES[nodes]
        SVCS[services]
        STACKS[stacks]
        SUI[SwarmUIState]
    end

    subgraph "View"
        PRES[Presenter]
    end

    CLI --> SW
    CLI --> SW2
    SW --> NODES
    SW2 --> SVCS
    SVCS --> STACKS
    NODES --> SM
    SVCS --> SM
    STACKS --> SM
    SUI --> SM
    SM --> PRES
    PRES --> STDOUT[stdout]
```

**Data flow:**
- `SwarmMonitor::update()`: `list_nodes()`, `list_services()` → `build_stacks()` → `generate_warnings()`
- `Presenter::render_swarm_overview()`: reads cluster_info, nodes, stacks, services, ui_state, warnings

---

### 4.5 Swarm Service Tasks

```mermaid
flowchart LR
    CLI[docker CLI] --> swarm::list_service_tasks
    swarm::list_service_tasks --> SM[SwarmMonitor.tasks]
    SM --> PRES[Presenter]
    PRES --> STDOUT[stdout]
```

**Data flow:**
- `enter_task_view(service_id, service_name)`: `list_service_tasks()` → `tasks`, `view_level = ServiceTasks`
- `update()` when in ServiceTasks: refreshes `tasks` on each tick

---

### 4.6 Swarm Service Logs

```mermaid
flowchart TB
    subgraph "Background"
        CLI[docker CLI]
        PROC[Child process]
    end

    subgraph "Channels"
        TX[mpsc::Sender]
        RX[mpsc::Receiver]
    end

    subgraph "Controller"
        SM[SwarmMonitor]
    end

    subgraph "Model"
        SLOG[ServiceLogState]
    end

    CLI -->|docker service logs -f| PROC
    PROC -->|lines| TX
    TX --> RX
    RX -->|poll_logs| SM
    SM -->|push_line| SLOG
    SLOG --> PRES[Presenter]
    PRES --> STDOUT[stdout]
```

**Data flow:**
- `start_service_log_stream()`: spawns `docker service logs -f` child, `LogStreamHandle` with receiver
- `poll_logs()`: drains receiver (up to 200 lines) into `ServiceLogState.lines`

---

### 4.7 Destructive Actions (Pending Confirmation)

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Pending: User presses R/S/T (restart/stop/start)
    Pending --> Idle: User presses N/Esc or timeout
    Pending --> Executing: User presses Y
    Executing --> Idle: poll_action() receives result
```

**Data flow:**
- User triggers action → `app.pending_action = Some(PendingAction { kind, expires })`
- Render shows confirmation overlay
- User presses Y → execute action (background thread) → `action_receiver`
- `poll_actions()` drains result → `status_message`, `action_in_progress = false`

---

## 5. Sequence Diagrams

### 5.1 Startup & First Render

```mermaid
sequenceDiagram
    participant M as main
    participant A as app
    participant Mon as Monitor
    participant DM as DockerMonitor
    participant SM as SwarmMonitor
    participant V as Presenter

    M->>A: run(should_quit)
    A->>A: enable_raw_mode, EnterAlternateScreen
    A->>A: App::new(rt)
    A->>Mon: Monitor::new()
    A->>DM: DockerMonitor::new(rt)
    A->>SM: SwarmMonitor::new()
    Note over A: app_view = System

    loop Main loop
        A->>A: process_tick()
        A->>Mon: update() [System view active]
        Mon->>Mon: sysinfo + collectors → last_data
        A->>A: render()
        A->>V: render_tab_bar()
        A->>V: render(data, ui_state, layout)
        V->>V: stdout
    end
```

---

### 5.2 Tab Switch (System → Containers)

```mermaid
sequenceDiagram
    participant User
    participant A as app
    participant DM as DockerMonitor
    participant Mon as Monitor

    User->>A: Tab key
    A->>A: handle_key() → app_view = Containers
    A->>A: needs_render = true

    Note over A: Next loop iteration
    A->>A: refresh_on_tab_switch()
    Note over A: app_view != prev_app_view
    A->>DM: update()
    DM->>DM: list_containers + get_all_cpu_percents
    A->>A: render()
    A->>A: Presenter::render_containers()
```

---

### 5.3 Enter Container Logs

```mermaid
sequenceDiagram
    participant User
    participant A as app
    participant DM as DockerMonitor
    participant DC as DockerClient
    participant Tokio as Tokio runtime

    User->>A: Enter on container (in Containers view)
    A->>A: handle_containers() → app_view = ContainerLogs(id)
    A->>DM: start_log_stream(id, name)
    DM->>DC: tail_logs(id, handle)
    DC->>Tokio: spawn(async { stream logs → tx.send(line) })
    DC-->>DM: mpsc::Receiver
    DM->>DM: log_state = Some(LogViewState), log_receiver = Some(rx)

    loop Every ~100ms
        A->>A: poll_logs()
        A->>DM: poll_logs()
        DM->>DM: rx.try_recv() × 100 → log_state.push_line()
        A->>A: render() → Presenter::render_logs()
    end
```

---

### 5.4 Swarm: Drill into Service Tasks

```mermaid
sequenceDiagram
    participant User
    participant A as app
    participant SM as SwarmMonitor
    participant swarm as swarm module

    User->>A: Enter on service (in Swarm overview)
    A->>A: resolve_swarm_overview_item() → Service(id, name)
    A->>A: app_view = SwarmServiceTasks(id, name)
    A->>SM: enter_task_view(id, name)
    SM->>swarm: list_service_tasks(id)
    swarm->>SM: tasks
    SM->>SM: view_level = ServiceTasks, tasks = [...]

    Note over A: Next tick
    A->>A: process_tick()
    A->>SM: update()
    SM->>swarm: list_service_tasks(id)
    swarm->>SM: tasks (refreshed)
    A->>A: render() → render_swarm_tasks()
```

---

### 5.5 Rolling Restart (with Confirmation)

```mermaid
sequenceDiagram
    participant User
    participant A as app
    participant SM as SwarmMonitor
    participant swarm as swarm module

    User->>A: R on service (in Swarm overview)
    A->>A: resolve_swarm_overview_item() → Service(id, name)
    A->>A: pending_action = PendingAction { SwarmRollingRestart(id), expires }
    A->>A: render() + render_confirmation("Rolling restart ... ? [Y/N]")

    User->>A: Y
    A->>A: handle_key() → pending_action.take()
    A->>SM: force_restart_service(id)
    SM->>SM: action_receiver, action_in_progress = true
    SM->>swarm: thread::spawn(force_update_service)
    swarm->>SM: tx.send(Ok/Err)

    loop Every loop
        A->>A: poll_actions()
        A->>SM: poll_action()
        SM->>SM: rx.try_recv() → status_message, action_in_progress = false
        A->>A: needs_render = true
    end
```

---

## 6. Refresh & Polling Strategy

### Selective Refresh (Tab-Based)

Only the **active view's** monitor is refreshed on each tick. This reduces I/O when the user is on a single tab.

| AppView | Monitor Updated on Tick |
|---------|-------------------------|
| System | Monitor (controller) |
| Containers, ContainerLogs | DockerMonitor |
| Swarm, SwarmServiceTasks, SwarmServiceLogs | SwarmMonitor |

### Tab Switch Refresh

When the user switches tabs, `refresh_on_tab_switch()` triggers an **immediate** update of the new view's monitor. A minimum 500ms throttle prevents rapid tab switching from hammering I/O.

### Swarm Recheck (Standalone Mode)

When Docker is in standalone mode (not Swarm), `SwarmMonitor::recheck_swarm()` runs every **10th tick** (~30 seconds) to detect if the user has initialized a Swarm.

### Log Polling

- **Container logs**: `poll_logs()` drains up to 100 lines per call when `AppView::ContainerLogs`
- **Service logs**: `poll_logs()` drains up to 200 lines per call when `AppView::SwarmServiceLogs`

### Action Polling

Background actions (start/stop/restart container, rolling restart, scale) use `try_recv()` so the main loop stays responsive. When a result arrives, `needs_render = true` triggers a redraw.

---

## 7. Component Responsibility Matrix

| Component | Owns | Updates | Reads From |
|-----------|------|---------|------------|
| **Monitor** | last_data, ui_state, layout, history | sysinfo, collectors, process grouping | — |
| **DockerMonitor** | containers, ui_state, log_state, status_message | DockerClient (list, stats, logs) | — |
| **SwarmMonitor** | nodes, services, stacks, tasks, ui_state, log_state, warnings | swarm module (CLI) | — |
| **App** | app_view, row_mapping, pending_action, tick state | event_loop, input | all monitors |
| **Presenter** | — | ui_state (expansion), row_mapping (returned) | monitors' data |
| **input** | — | app_view, monitors' ui_state, pending_action | App |

### Data Ownership Summary

```
App
├── monitor: Monitor          → last_data, ui_state, layout
├── docker_monitor: DockerMonitor  → containers, ui_state, log_state
├── swarm_monitor: SwarmMonitor    → nodes, services, stacks, tasks, ui_state, log_state
├── app_view: AppView
├── row_mapping: Vec<(Pid, RowKind)>   ← from Presenter::render (System view)
└── pending_action: Option<PendingAction>
```

### No Shared Mutable State

Each monitor owns its data. The app holds references to monitors and passes them to render/input. There is no global mutable state beyond the `App` struct itself.

---

## 8. View Navigation (AppView State Machine)

```mermaid
stateDiagram-v2
    [*] --> System

    System --> Containers: Tab (if Docker available)
    System --> Swarm: Tab (if Swarm, no Docker)
    Containers --> Swarm: Tab (if Swarm)
    Containers --> System: Tab (if no Swarm)
    Swarm --> System: Tab
    Swarm --> Containers: Tab (if Docker)

    Containers --> ContainerLogs: Enter on container
    ContainerLogs --> Containers: Esc

    Swarm --> SwarmServiceTasks: Enter on service
    SwarmServiceTasks --> SwarmServiceLogs: L (logs)
    SwarmServiceTasks --> Swarm: Esc
    SwarmServiceLogs --> Swarm: Esc
```

### Tab Visibility

| Tab | Shown When |
|-----|------------|
| System | Always |
| Containers | `docker_monitor.is_available()` |
| Swarm | `swarm_monitor.is_swarm()` |

Tab order: **System → Containers → Swarm → System** (wraps). Tabs are hidden when their backend is unavailable.

---

## 9. Model Type Relationships

```mermaid
erDiagram
    MonitorData ||--o{ ProcessGroup : "historical_top"
    MonitorData ||--o{ DiskSpaceInfo : "disk_space"
    MonitorData ||--o{ NetworkInterfaceInfo : "interfaces"
    MonitorData ||--|| MemoryInfo : "memory"
    MonitorData ||--|| NetworkInfo : "network"
    MonitorData ||--|| FdInfo : "fd_info"
    MonitorData ||--|| ContextSwitchInfo : "context_switches"
    MonitorData ||--|| SocketOverviewInfo : "socket_overview"

    ProcessGroup ||--o{ ProcessInfo : "children"

    SwarmStackInfo ||--o{ SwarmServiceInfo : "service_indices"

    LogViewState ||--o{ String : "lines"
    ServiceLogState ||--o{ String : "lines"

    UIState ||--o{ Pid : "expanded_pids"
    SwarmUIState ||--o{ String : "expanded_ids"
    ContainerUIState ||--o{ String : "expanded_ids"
```

---

## 10. I/O and Concurrency Summary

| Operation | Blocking? | Concurrency |
|-----------|-----------|-------------|
| System monitor update | Yes (sysinfo, collectors) | Main thread |
| Docker list_containers | Yes (block_on) | Main thread |
| Docker get_all_cpu_percents | Yes (block_on, but concurrent inside) | join_all futures |
| Docker tail_logs | No (async task) | Tokio spawn, mpsc channel |
| Swarm list_nodes/services | Yes (Command::output) | Main thread |
| Swarm tail_service_logs | No (child process) | std::thread, mpsc channel |
| Container start/stop/restart | No (background thread) | std::thread, mpsc channel |
| Swarm force_restart/scale | No (background thread) | std::thread, mpsc channel |
