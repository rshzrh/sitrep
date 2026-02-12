# sitrep 🎖️

**Situation Report** — A real-time terminal diagnostic tool for server triage.

When your server is busy and not responding, `sitrep` gives you the full picture at a glance: CPU, memory, disk, network, file descriptors, context switches, and socket health — all in one interactive terminal UI.

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

- **Interactivity**:
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

## Usage

```bash
sitrep
```

### Controls

- `q`: Quit
- `Space`: Pause/Resumze
- `↑ / ↓`: Navigate list
- `→`: Expand process group (or collapse section)
- `←`: Collapse process group (or expand section)
- `c`: Sort by CPU
- `m`: Sort by Memory
- `r`: Sort by Disk Read
- `w`: Sort by Disk Write
- `d`: Sort by Network Download
- `u`: Sort by Network Upload


## Architecture

```
src/
├── main.rs         # Application loop & input handling
├── model.rs        # Data structures
├── view.rs         # Terminal rendering
├── controller.rs   # Data collection & processing
└── layout.rs       # Section layout system (collapsible sections)
```

MVC architecture with a reusable `Layout` system for defining report sections.

## Roadmap

### 🔵 Phase 1 — Cross-Platform (Linux Support)

The #1 priority. `sitrep` currently uses macOS-specific system commands (`iostat`, `netstat`, `lsof`, `sysctl`). Linux support requires platform-aware backends:

- [ ] **Platform abstraction layer** — introduce a trait-based backend so each collector (disk I/O, FDs, sockets, context switches) dispatches to OS-specific implementations at compile time via `#[cfg(target_os)]`
- [ ] **Linux: Disk I/O busy %** — read from `/proc/diskstats` or `/sys/block/*/stat` instead of `iostat`
- [ ] **Linux: File descriptors** — read `/proc/sys/fs/file-nr` for system-wide FD counts instead of `sysctl kern.maxfiles`; use `/proc/<pid>/fd` for per-process counts instead of `lsof`
- [ ] **Linux: Connection counts & socket overview** — parse `/proc/net/tcp` and `/proc/net/tcp6`, or use `ss -s` instead of `netstat`
- [ ] **Linux: Context switches** — read `/proc/<pid>/status` (`voluntary_ctxt_switches`, `nonvoluntary_ctxt_switches`) instead of `ps -eo comm,nivcsw`
- [ ] **Linux: Top bandwidth processes** — use `/proc/net/dev` + `/proc/<pid>/net/dev` or integrate `nethogs`-style accounting instead of `lsof -i`
- [ ] **CI matrix** — add GitHub Actions builds for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` alongside macOS

### 🟢 Phase 2 — Enhanced Diagnostics

- [ ] **GPU monitoring** — NVIDIA (`nvidia-smi`) and Apple Silicon GPU usage
- [ ] **Per-disk I/O breakdown** — show read/write rates per individual disk, not just aggregate busy %
- [ ] **Temperature sensors** — CPU/GPU/disk temperatures where available
- [ ] **Container awareness** — detect Docker/Podman containers and show per-container resource usage
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
