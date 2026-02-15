# sitrep 🎖️

**Situation Report** — A real-time terminal diagnostic tool for server triage.

When your server is busy and not responding, `sitrep` gives you the full picture at a glance: CPU, memory, disk, network, file descriptors, context switches, socket health, Docker containers, and **Docker Swarm clusters** — all in one interactive terminal UI.

## Features

- **System Summary**: 
  - **Load Average**: 1m, 5m, 15m averages with core count context.
  - **Memory & Swap**: Visual progress bars and usage stats.
  - **Disk Usage**: Overview of all mounted disks with warning indicators (< 10% free).
  - **Network**: Monitor interface bandwidth (upload/download) and connection counts.
  - **File Descriptors**: Track system-wide usage and top consumers.
  - **Socket Connections**: Overview of TCP states (ESTABLISHED, TIME_WAIT, etc.).

- **Top Processes List**:
  - **Unified View**: Combines CPU, Memory, Disk I/O, and Network usage in one list.
  - **Sortable**: Dynamically sort by **CPU** (default), **Memory**, **Read**, **Write**, **Download**, or **Upload**.
  - **Expandable**: Grouped by parent process; expand to see child processes.
  - **Network Stats**: Per-process upload/download rates sourced from `nettop`.

- **Docker Containers** (auto-detected):
  - **Container List**: Running containers with name, status, uptime, CPU %, exposed ports, and internal IP.
  - **Live Logs**: Full-screen `tail -f` style log viewer with auto-follow and manual scroll.
  - **Container Actions**: Start, stop, and restart containers directly from the TUI.
  - **Expandable Details**: View image, full status, port mappings, and network info per container.
  - **Auto-hide**: The Containers tab is hidden when Docker is not installed or the daemon is not running.

- **Docker Swarm Cluster** (auto-detected):
  - **Automatic Detection**: `sitrep` detects Swarm mode automatically — no configuration needed.
  - **Cluster Overview**: Node count, manager count, node status (Ready/Down), availability (Active/Drain).
  - **Service & Stack Grouping**: Services grouped by stack (`com.docker.stack.namespace` label) with expandable drill-down.
  - **Task/Replica List**: View all replicas of a service with current state, desired state, node placement, and errors.
  - **Service Logs**: Full-screen aggregated log viewer across all replicas of a service with auto-follow.
  - **Error Filtering**: Toggle error-only mode (`e` key) to surface `ERROR`, `panic`, `fatal`, and `exception` messages.
  - **Rolling Restart**: Force-restart all replicas of a service (`R` key) via `docker service update --force`.
  - **Smart Warnings**: Automatic alerts for down nodes, drained nodes, degraded services, and insufficient manager count.
  - **Auto-hide**: The Swarm tab only appears when running on a Swarm manager node.

- **Interactivity**:
  - **View Titles**: Each view displays a clear title at the top (System, Containers, Swarm Cluster, etc.) so you always know which tab you're in.
  - **Tab Switching**: `Tab` / `Shift+Tab` to cycle between System, Containers, and Swarm views.
  - **Navigation**: Arrow keys to scroll and expand/collapse.
  - **Sorting**: Keys `c`, `m`, `r`, `w`, `d`, `u` to sort the process list.
  - **Pause**: Spacebar to pause/resume updates.

## Installation

### From source (requires [Rust](https://rustup.rs/))

```bash
cargo install --git https://github.com/rshzrh/sitrep
```

### Build locally

```bash
git clone https://github.com/rshzrh/sitrep.git
cd sitrep
cargo build --release
./target/release/sitrep
```

### Testing on Linux (via Docker)

Since `sitrep` uses OS-specific APIs (procfs on Linux), you can verify the Linux build using Docker:

```bash
# Build the Docker image (compiles sitrep for Linux)
docker build -t sitrep-linux .

# Run the container (verifies startup and data collection)
docker run --rm -it sitrep-linux
```

## Usage

```bash
sitrep
```

### Controls

#### Global

- `q` / `Esc`: Quit
- `Ctrl+C`: Force quit
- `Tab`: Switch to next tab (System → Containers → Swarm)
- `Shift+Tab`: Switch to previous tab

#### System Tab

- `↑ / ↓`: Navigate list
- `→`: Expand process group or uncollapse section
- `←`: Collapse process group or collapse section
- `c`: Sort by CPU
- `m`: Sort by Memory
- `r`: Sort by Disk Read
- `w`: Sort by Disk Write
- `d`: Sort by Network Download
- `u`: Sort by Network Upload

#### Containers Tab

- `↑ / ↓`: Navigate container list
- `→`: Open live log viewer for the selected container
- `←`: Expand/collapse container details (image, ports, IP)
- `s`: Start the selected container
- `t`: Stop the selected container
- `r`: Restart the selected container (confirm with `y`, cancel with `n` or `Esc`)

#### Container Log Viewer (full-screen)

- `Esc` / `←`: Return to container list
- `↑ / ↓`: Scroll through log history (pauses auto-follow)
- `PageUp / PageDown`: Scroll by page
- `f` / `End`: Resume auto-follow
- `/`: Search mode (type query, Enter to confirm, Esc to cancel)

#### Swarm Tab — Overview

- `↑ / ↓`: Navigate nodes, stacks, and services
- `→`: Expand section / drill into service tasks
- `←`: Collapse section / go back
- `R`: Rolling restart the selected service (confirm with `y`, cancel with `n` or `Esc`)

#### Swarm Tab — Task/Replica List

- `↑ / ↓`: Navigate tasks
- `→` / `L`: Open service log viewer
- `R`: Rolling restart the service (confirm with `y`, cancel with `n` or `Esc`)
- `Esc` / `←`: Back to overview

#### Service Log Viewer (full-screen)

- `Esc` / `←`: Return to Swarm overview
- `↑ / ↓`: Scroll through log history (pauses auto-follow)
- `PageUp / PageDown`: Scroll by page
- `f` / `End`: Resume auto-follow
- `e`: Toggle error-only filter (shows ERROR, panic, fatal, exception lines)
- `/`: Search mode (type query, Enter to confirm, Esc to cancel)


## Docker Integration

`sitrep` includes built-in Docker container monitoring. If Docker is installed and the daemon is running, a **Containers** tab appears automatically.

### Requirements

- Docker Engine or Docker Desktop installed and running.
- The user running `sitrep` must have access to the Docker socket:
  - **Linux**: be in the `docker` group (`sudo usermod -aG docker $USER`) or run as root.
  - **macOS**: Docker Desktop handles permissions automatically.

### What it shows

| Column | Description |
|---|---|
| Container ID | Short 12-character container ID |
| Name | Container name |
| Status | Current state (running, paused, etc.) |
| Uptime | Time since container was created |
| CPU % | Live CPU usage percentage |
| Ports | Exposed port mappings (e.g. `0.0.0.0:8080->80/tcp`) |
| IP | Internal container IP address |

### Live log viewer

Press `→` on any container to enter a full-screen log viewer:

- Streams logs in real time (`tail -f` behavior).
- Auto-scrolls to the latest output by default.
- Press `↑` or `↓` to pause auto-follow and scroll through history.
- Press `f` or `End` to resume following.
- Press `/` to search (type query, Enter to confirm, Esc to cancel).
- Press `Esc` or `←` to return to the container list.

### Container actions

From the container list, you can manage containers directly:

- `s` — **Start** a stopped container
- `t` — **Stop** a running container (10-second graceful timeout)
- `r` — **Restart** a container (10-second graceful timeout)

Destructive actions (stop, restart) require confirmation: press `y` to confirm or `n` / `Esc` to cancel. Action feedback is displayed as a status message in the container view.

### When Docker is unavailable

If Docker is not installed, the daemon is not running, or the socket is not accessible, the Containers tab is simply hidden. No error is shown and the System tab works as normal.

## Docker Swarm Integration

`sitrep` includes built-in Docker Swarm cluster monitoring. If the current node is part of a Swarm cluster and is a **manager node**, a **Swarm** tab appears automatically alongside the Containers tab.

### Requirements

- Docker Engine running in **Swarm mode** (`docker swarm init` or `docker swarm join`).
- `sitrep` must be run on a **manager node** (workers don't have access to the full cluster API).
- The `docker` CLI must be available in `$PATH`.

### How it works

`sitrep` automatically detects Swarm mode by querying `docker info`. When Swarm is active:

1. **Cluster Overview**: Shows all nodes with status, availability, role, and engine version. Down or drained nodes are highlighted in red/yellow.
2. **Stack Grouping**: Services are automatically grouped by their stack name (from the `com.docker.stack.namespace` label). Services not part of a stack are shown under "(no stack)".
3. **Service Drill-down**: Press `→` on a service to see all its tasks/replicas with node placement, desired state, current state, and any error messages. Failed/rejected tasks are highlighted in red, running tasks in green.
4. **Aggregated Service Logs**: Press `→` or `L` from the task list to open a full-screen log viewer that streams logs from **all replicas** of the service (`docker service logs --follow`).
5. **Error Filtering**: Press `e` in the log viewer to filter to only lines containing `error`, `panic`, `fatal`, `exception`, or `fail`.
6. **Search**: Press `/` in the log viewer to search for text (Enter to confirm, Esc to cancel).

### Smart warnings

`sitrep` automatically generates warnings for common cluster issues:

| Warning | Condition |
|---|---|
| **NODE DOWN** | One or more nodes are unreachable |
| **DRAINED** | Nodes in drain mode (won't accept new tasks) |
| **SERVICE DEGRADED** | Service has fewer running replicas than desired (e.g. 2/3) |
| **LOW MANAGERS** | Fewer than 3 managers in a cluster with more than 3 nodes |

### Service actions

From the Swarm overview or task list:

- `R` — **Rolling restart**: Force-updates the service (`docker service update --force`), which triggers a rolling restart of all replicas according to the service's update configuration. Press `y` to confirm or `n` / `Esc` to cancel.

### Typical workflow

1. Launch `sitrep` on a Swarm manager node
2. Press `Tab` to reach the **Swarm** tab
3. Expand a stack with `→` to see its services
4. Press `→` on a service to see all replicas and which nodes they're running on
5. Press `→` or `L` to tail aggregated logs across all replicas
6. Press `e` to filter for error messages
7. Press `R` to rolling restart the service if needed
8. Press `Esc` to navigate back up the hierarchy

### When Swarm is unavailable

If Docker is not in Swarm mode, or `sitrep` is running on a worker node, the Swarm tab is hidden. The System and Containers tabs continue to work normally.

## Architecture

```
src/
├── main.rs              # Thin entry point (panic hook, signals, app::run)
├── app/                 # Application orchestration
│   ├── mod.rs           # App struct, main loop, run()
│   ├── event_loop.rs   # Tick refresh, log polling, action polling, tab-switch refresh
│   ├── input.rs        # Key handling, view-specific handlers
│   ├── render.rs       # Render dispatch by AppView
│   └── state.rs        # PendingAction, SwarmOverviewItem, resolve_swarm_overview_item
├── model/               # Data structures (system + Docker + Swarm)
│   ├── app.rs          # AppView enum
│   ├── system.rs       # MonitorData, UIState, ProcessGroup, etc.
│   ├── docker.rs       # DockerContainerInfo, LogViewState, ContainerUIState
│   └── swarm.rs        # SwarmNodeInfo, SwarmServiceInfo, SwarmUIState, etc.
├── view/                # Terminal rendering
│   ├── mod.rs          # Presenter, RowKind
│   ├── tab_bar.rs      # Tab bar with view titles
│   ├── system.rs       # System report (sections, processes)
│   ├── containers.rs   # Container list
│   ├── swarm.rs        # Swarm overview, tasks
│   ├── logs.rs         # Container + service logs
│   ├── confirmation.rs # Pending action prompt
│   └── shared.rs       # truncate_str, progress_bar, etc.
├── controller/          # System data collection & processing
│   ├── mod.rs          # Monitor, update()
│   └── process.rs      # Process grouping, compute_top_processes
├── layout.rs            # Section layout system (collapsible sections)
├── docker.rs            # Docker API client (bollard wrapper)
├── docker_controller.rs # Docker data collection & log streaming
├── swarm.rs             # Swarm CLI client (node, service, task, log operations)
├── swarm_controller.rs  # Swarm data collection, state management & actions
└── collectors/
    ├── mod.rs           # Platform collector trait
    ├── mac.rs           # macOS-specific collector
    └── linux.rs         # Linux-specific collector
```

MVC architecture with a reusable `Layout` system for defining report sections. Docker container integration uses [bollard](https://crates.io/crates/bollard) (async Docker API) for standalone containers. Swarm integration uses the `docker` CLI with JSON output for cluster-wide operations (nodes, services, tasks, service logs).

For a detailed technical breakdown of data flow, sequence diagrams, and component responsibilities, see [Architecture.md](Architecture.md).

## Roadmap

### 🔵 Phase 1 — Cross-Platform (Linux Support)

The #1 priority. `sitrep` currently uses macOS-specific system commands (`iostat`, `netstat`, `lsof`, `sysctl`). Linux support requires platform-aware backends:

- [x] **Platform abstraction layer** — introduce a trait-based backend so each collector (disk I/O, FDs, sockets, context switches) dispatches to OS-specific implementations at compile time via `#[cfg(target_os)]`
- [x] **Linux: Disk I/O busy %** — read from `/proc/diskstats` or `/sys/block/*/stat` instead of `iostat`
- [x] **Linux: File descriptors** — read `/proc/sys/fs/file-nr` for system-wide FD counts instead of `sysctl kern.maxfiles`; use `/proc/<pid>/fd` for per-process counts instead of `lsof`
- [x] **Linux: Connection counts & socket overview** — parse `/proc/net/tcp` and `/proc/net/tcp6`, or use `ss -s` instead of `netstat`
- [x] **Linux: Context switches** — read `/proc/<pid>/status` (`voluntary_ctxt_switches`, `nonvoluntary_ctxt_switches`) instead of `ps -eo comm,nivcsw`
- [ ] **Linux: Top bandwidth processes** — use `/proc/net/dev` + `/proc/<pid>/net/dev` or integrate `nethogs`-style accounting instead of `lsof -i`
- [ ] **CI matrix** — add GitHub Actions builds for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` alongside macOS

### 🟢 Phase 2 — Enhanced Diagnostics

- [ ] **GPU monitoring** — NVIDIA (`nvidia-smi`) and Apple Silicon GPU usage
- [ ] **Per-disk I/O breakdown** — show read/write rates per individual disk, not just aggregate busy %
- [x] **Container awareness** — Docker container monitoring with live stats, log tailing, and start/stop/restart actions
- [x] **Swarm cluster monitoring** — Automatic Swarm detection, node/service/task views, aggregated service logs, error filtering, rolling restarts, and smart warnings
- [ ] **Alerting & thresholds** — configurable warning thresholds (not just the hardcoded 10% disk), with optional desktop notifications
- [ ] **Process tree view** — full hierarchical process tree with fold/unfold, not just parent grouping
- [ ] **Historical sparklines** — tiny inline graphs showing trends for CPU, memory, and network over the last few minutes

### 🟡 Phase 3 — Advanced Features

- [ ] **Config file** — `~/.config/sitrep/config.toml` for refresh rate, default collapsed sections, custom thresholds, and theme colors
- [ ] **Snapshot / export** — save a point-in-time report as JSON or plain text for sharing in incident postmortems
- [ ] **Remote mode** — SSH into a remote host and run `sitrep` against it, or accept metrics over a Unix socket
- [ ] **Custom sections & plugins** — let users define their own diagnostic sections via shell commands or scripts
- [ ] **Multi-host dashboard** — aggregate multiple `sitrep` instances into a single view (stretch goal)

### 🏁 Ecosystem

- [ ] Publish to [crates.io](https://crates.io/)
- [ ] Homebrew formula (`brew install sitrep`)
- [ ] AUR package for Arch Linux
- [ ] Prebuilt binaries via GitHub Releases (macOS universal + Linux x86_64/aarch64)
- [ ] `man` page and shell completions

---

## Feedback

`sitrep` is under active development and shaped by real-world server incidents. **Your feedback matters!**

If you have ideas, bug reports, or feature requests:

- 🐛 **Bug reports & feature requests** — [open an issue](https://github.com/rshzrh/sitrep/issues)
- 💬 **General discussion** — [start a discussion](https://github.com/rshzrh/sitrep/discussions)
- 🙌 **Pull requests welcome** — see the architecture section above to get oriented

> **What would make `sitrep` useful for *your* workflow?**
> I would love to hear what diagnostics you reach for first during an incident, what's missing, and what's noisy. Drop a note in [Discussions](https://github.com/rshzrh/sitrep/discussions) or [Issues](https://github.com/rshzrh/sitrep/issues) — even a quick "I wish it showed X" is super helpful.

---

## License

[MIT](LICENSE)
