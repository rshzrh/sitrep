# flotop

Agentless multi-host terminal UI for triaging Linux servers over SSH.

flotop connects to one or more remote Linux boxes via SSH and shows system metrics, Docker containers, and Docker Swarm clusters in a single interactive terminal. Nothing is installed on the remote hosts. One command, one screen, all your boxes.

## Quick start

```bash
# Monitor a single remote host
flotop root@your-server.com

# Monitor multiple hosts at once
flotop root@web-01 root@db-01 root@worker-01

# Local-only mode (no SSH, monitor the current machine)
flotop
```

## Installation

### Prebuilt binaries (Linux + macOS)

```bash
curl -sSL https://github.com/rshzrh/flotop/releases/latest/download/install.sh | sh
```

### From crates.io

```bash
cargo install flotop
```

### From source

```bash
git clone https://github.com/rshzrh/flotop
cd flotop
cargo build --release
./target/release/flotop
```

### Docker (build locally)

A `Dockerfile` is included for building flotop on Linux — useful for testing the Linux code path from a macOS host. There is no prebuilt image on any registry.

```bash
docker build -t flotop .
docker run --rm -it --pid=host --net=host flotop
```

## How it works

flotop opens one SSH connection per host (using [russh](https://crates.io/crates/russh), a pure-Rust SSH client). On each refresh tick (every 5 seconds for remote, 3 seconds for local), it sends a small shell script via `sh -s` that reads `/proc` files, runs `ss -s`, `ps`, `df`, and `docker` commands, and returns all the output in a single round-trip. The output is parsed locally and rendered in the terminal.

No binary, daemon, or agent is installed on the remote. The remote only needs a standard shell (`sh`) and, for container features, the `docker` CLI.

## What flotop shows

### Fleet overview

When you pass one or more host arguments, flotop starts in fleet overview mode. Each host is a row showing:

- Connection status (UP / DEGRADED / DOWN) with color coding
- Load average (1m / 5m / 15m)
- Memory usage percentage
- Busiest disk percentage
- Top CPU-consuming process
- Number of established TCP connections
- Time since last successful refresh

Hosts that disconnect are automatically reconnected with exponential backoff (1s, 2s, 4s, 8s, 16s, 30s cap).

### Per-host drill-in

Press `Enter` on any host row to drill into a full per-host view with three tabs:

**System tab** shows the same information you would get from running `htop`, `df`, `ss`, and `lsof` on the remote box:

- CPU and memory usage with progress bars
- Swap usage
- Per-mount disk space with low-space warnings (< 10% free)
- Network interface bandwidth (bytes/sec)
- File descriptor usage (system-wide total + top 5 consumers)
- Socket overview (established, listen, time-wait, close-wait, fin-wait counts + top processes by connection count)
- Context switches since boot + top 5 processes
- Process list sortable by CPU, memory, disk read, disk write, network download, or network upload

**Containers tab** shows Docker containers running on that host:

- Container ID, name, state, uptime, live CPU %, exposed ports, internal IP
- Container details (expand with `<-` to see image, full status, port mappings)
- Multi-select containers with `Space`, open aggregated log stream with `l`
- Start, stop, or restart containers with `S`, `T`, `R` (with y/n confirmation)
- Full-screen live log viewer (`->` on a container): streams `docker logs -f` over SSH in real time, with search (`/`) and scroll

**Swarm tab** appears when the remote host is a Docker Swarm manager node:

- Cluster overview: node count, manager count, node status and availability
- Services grouped by stack name
- Per-service replica count with degraded-service warnings
- Drill into a service to see all tasks/replicas with state, node placement, and errors
- Aggregated service log viewer across all replicas, with error filter (`e`) and search (`/`)
- Rolling restart (`R`) via `docker service update --force` (with y/n confirmation)
- Smart warnings: node down, node drained, service degraded, low manager count

### Snapshot mode

Generate a Markdown report of the current state of your fleet and exit:

```bash
flotop --snapshot root@web-01 root@db-01 > report.md
```

Add `--anonymize` to replace real hostnames with `host1`, `host2`, etc. before sharing:

```bash
flotop --snapshot --anonymize root@web-01 root@db-01 > incident-report.md
```

The output includes load averages, memory, disk, TCP connections, context switches, and top processes for each host. Paste it into a GitHub issue, a Slack message, or an incident postmortem.

## Command-line options

```
flotop [OPTIONS] [HOST]...
```

| Option | Default | Description |
|--------|---------|-------------|
| `[HOST]...` | (none) | Hosts to monitor. Format: `[user@]host[:port]`. If empty, monitors the local machine only. |
| `--user` | `root` | Default SSH user when a host argument doesn't include `user@`. |
| `--ssh-key` | auto-detect | Path to an SSH private key. If not set, tries `~/.ssh/id_ed25519`, then `~/.ssh/id_ecdsa`, then `~/.ssh/id_rsa`. Tries each until one authenticates. |
| `--refresh-rate` | `3` | Refresh interval in seconds for the local collector. |
| `--remote-refresh` | `5` | Refresh interval in seconds for remote hosts. |
| `--snapshot` | off | Print a Markdown report of the fleet and exit (no TUI). |
| `--anonymize` | off | Replace real hostnames with host1/host2/... in `--snapshot` output. |
| `--no-docker` | off | Disable Docker container monitoring. |
| `--log-file` | `~/.flotop/flotop.log` | Path to the log file. |
| `--log-level` | `info` | Log verbosity: error, warn, info, debug, trace. |

## Key bindings

### Fleet overview

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate host list |
| `Enter` / `Right` | Drill into the selected host |
| `q` / `Esc` | Quit |

### System tab (per-host)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate sections and process list |
| `Right` | Expand a collapsed section |
| `Left` | Collapse an expanded section |
| `c` | Sort processes by CPU |
| `m` | Sort processes by memory |
| `r` | Sort processes by disk read |
| `w` | Sort processes by disk write |
| `d` | Sort processes by network download |
| `u` | Sort processes by network upload |
| `Tab` / `Shift-Tab` | Switch to next/previous tab |
| `Esc` | Return to fleet overview |

### Containers tab (per-host)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate container list |
| `Right` | Open live log viewer for the selected container |
| `Left` | Expand/collapse container details (image, ports, IP) |
| `Space` | Toggle multi-select on the selected container |
| `l` / `L` | Open aggregated log view for all selected containers |
| `S` | Start the selected container (with confirmation) |
| `T` | Stop the selected container (with confirmation) |
| `R` | Restart the selected container (with confirmation) |
| `Tab` / `Shift-Tab` | Switch to next/previous tab |
| `Esc` | Return to fleet overview |

### Log viewer (container or service)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Scroll through log history |
| `PageUp` / `PageDown` | Scroll by page |
| `f` / `End` | Resume auto-follow (jump to latest) |
| `/` | Enter search mode (type query, Enter to confirm, Esc to cancel) |
| `e` | Toggle error-only filter (service logs only) |
| `Esc` / `Left` | Return to the container or service list |

### Swarm tab (per-host, manager nodes only)

| Key | Action |
|-----|--------|
| `Up` / `Down` | Navigate nodes, stacks, and services |
| `Right` / `Enter` | Expand section or drill into service tasks |
| `R` | Rolling restart the selected service (with confirmation) |
| `L` / `Right` (from task list) | Open aggregated service log viewer |
| `Esc` / `Left` | Go back one level |
| `Tab` / `Shift-Tab` | Switch to next/previous tab |

## Requirements

**Local machine (where you run flotop):**
- macOS or Linux
- Rust 1.85+ (if building from source)

**Remote hosts:**
- Linux with `/proc` filesystem (any modern distribution)
- SSH access with key-based authentication
- For container features: `docker` CLI installed and the SSH user in the `docker` group (or root)
- For Swarm features: the host must be a Swarm manager node

flotop does not require Docker to be installed on the remote for the System tab. If Docker is not available, the Containers and Swarm tabs will show empty or an error message, and the System tab continues to work normally.

## SSH authentication

flotop tries SSH keys in this order:

1. The key specified by `--ssh-key` (if provided)
2. `~/.ssh/id_ed25519`
3. `~/.ssh/id_ecdsa`
4. `~/.ssh/id_rsa`

It tries each key against the remote host until one is accepted, matching the behavior of the `ssh` command. If none work, the connection fails with a message listing every path that was tried and why each failed.

SSH agent forwarding and password authentication are not supported in this version. `ProxyCommand` and `ProxyJump` directives in `~/.ssh/config` are not supported (russh limitation).

## Known limitations

- **Linux remotes only.** macOS and Windows remotes are not supported because flotop reads `/proc` files which only exist on Linux.
- **Remote Docker requires the `docker` CLI.** flotop shells out to `docker ps`, `docker stats`, `docker node ls`, etc. over SSH. It does not connect to the Docker socket directly.
- **Per-process network bandwidth is local-only.** Remote hosts show per-interface totals but not per-process bandwidth.
- **No history persistence.** When a host disconnects, the in-memory metric history is lost. The reconnected view starts fresh.
- **No `ProxyCommand` / `ProxyJump`.** If your hosts require a jump host, set up an SSH tunnel manually and connect flotop to `localhost:port`.

## Architecture

flotop is written in Rust (edition 2024) and follows an MVC architecture:

- **Model** (`src/model/`): data structs for system metrics, Docker containers, Swarm clusters, and UI state
- **View** (`src/view/`): terminal rendering via crossterm. All render functions take plain model structs and don't depend on where the data came from.
- **Controller** (`src/controller/`, `src/remote_host.rs`, `src/remote_docker.rs`, `src/remote_swarm.rs`): local system data collection via sysinfo + OS-specific collectors, and remote collection via russh + shell commands
- **App** (`src/app/`): single-threaded event loop (~100ms poll), key handling, render dispatch

Remote hosts are driven by background tokio tasks (one per host). Each task maintains a persistent russh session, sends commands via concurrent session channels, and pushes updates to the main loop via `mpsc`. The main loop drains updates every tick and re-renders.

For a detailed breakdown with sequence diagrams, see [Architecture.md](Architecture.md).

## Building and testing

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # Run all tests (144 tests)
cargo test --test integration_fleet   # End-to-end russh round-trip test
cargo clippy                   # Lint
```

### Testing on Linux via Docker

```bash
docker build -t flotop-linux .
docker run --rm -it flotop-linux
```

### Cross-compilation

The release workflow (`.github/workflows/release.yml`) builds prebuilt binaries for four targets on every tag push:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu` (via [cross](https://github.com/cross-rs/cross))
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

## Contributing

Bug reports, feature requests, and pull requests are welcome.

- [Open an issue](https://github.com/rshzrh/flotop/issues)
- [Start a discussion](https://github.com/rshzrh/flotop/discussions)

## License

[MIT](LICENSE)
