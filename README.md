# sitrep 🎖️

**Situation Report** — A real-time terminal diagnostic tool for server triage.

When your server is busy and not responding, `sitrep` gives you the full picture at a glance: CPU, memory, disk, network, file descriptors, context switches, and socket health — all in one interactive terminal UI.

## Features

- **Load Average** — 1/5/15 min with core count, red when overloaded
- **Disk Space Warnings** — Alerts for drives with < 10% free space
- **Memory Overview** — RAM & Swap usage with visual progress bars
- **Top 5 CPU Processes** — 60-second sliding window average, grouped by parent
- **Top 5 Disk I/O Processes** — Read/write throughput per process group
- **Network & Bandwidth** — Per-interface bandwidth + connection counts
- **Open File Descriptors** — System usage vs kernel limit + top 5 processes
- **Context Switches** — Involuntary context switch count + top 5 offenders
- **TCP/Socket Overview** — Connection state breakdown with warnings for leaks

### Interactive UI

- **↑↓** Navigate between rows
- **→** Expand section or process children
- **←** Collapse section or process children
- **Q / Esc / Ctrl+C** Quit

All sections are **collapsible** — collapse what you don't need, focus on what matters.

Data freezes for expanded sections so you can inspect process details without the display jumping around.

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

That's it. No flags, no config files. Just run it and get your situation report.

## Requirements

- **macOS** (primary target — uses macOS-specific system commands)
- Linux support planned for a future release

## How It Works

`sitrep` uses:
- [`sysinfo`](https://crates.io/crates/sysinfo) for CPU, memory, disk, and network data
- [`crossterm`](https://crates.io/crates/crossterm) for the terminal UI
- System commands (`lsof`, `netstat`, `ps`, `iostat`, `sysctl`) for deeper diagnostics

Data is refreshed every 3 seconds. Process CPU usage is averaged over a 60-second sliding window for stable readings.

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

## License

[MIT](LICENSE)
