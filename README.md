# sitrep 🎖️

**Situation Report** — A real-time terminal diagnostic tool for server triage.

When your server is busy and not responding, `sitrep` gives you the full picture at a glance: CPU, memory, disk, network, file descriptors, context switches, socket health, and Docker containers — all in one interactive terminal UI.

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

- **Interactivity**:
  - **Tab Switching**: `Tab` / `Shift+Tab` to switch between System and Containers views.
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
- `Tab`: Switch to next tab (System / Containers)
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
- `r`: Restart the selected container

#### Log Viewer (full-screen)

- `Esc` / `←`: Return to container list
- `↑ / ↓`: Scroll through log history (pauses auto-follow)
- `PageUp / PageDown`: Scroll by page
- `f` / `End`: Resume auto-follow


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
- Press `Esc` or `←` to return to the container list.

### Container actions

From the container list, you can manage containers directly:

- `s` — **Start** a stopped container
- `t` — **Stop** a running container (10-second graceful timeout)
- `r` — **Restart** a container (10-second graceful timeout)

Action feedback is displayed as a status message in the container view.

### When Docker is unavailable

If Docker is not installed, the daemon is not running, or the socket is not accessible, the Containers tab is simply hidden. No error is shown and the System tab works as normal.

## Architecture

```
src/
├── main.rs              # Application loop, tab switching & input handling
├── model.rs             # Data structures (system + Docker)
├── view.rs              # Terminal rendering (tab bar, system, containers, logs)
├── controller.rs        # System data collection & processing
├── layout.rs            # Section layout system (collapsible sections)
├── docker.rs            # Docker API client (bollard wrapper)
├── docker_controller.rs # Docker data collection & log streaming
└── collectors/
    ├── mod.rs           # Platform collector trait
    ├── mac.rs           # macOS-specific collector
    └── linux.rs         # Linux-specific collector
```

MVC architecture with a reusable `Layout` system for defining report sections. Docker integration uses a separate `DockerMonitor` backed by the [bollard](https://crates.io/crates/bollard) crate, communicating with the Docker daemon over the local Unix socket.

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
