# Why These Data Structures & Algorithms?

> Design decisions behind sitrep's internals — what we chose, why we chose it, and how each choice affects real-world performance.

---

## 1. VecDeque as a Ring Buffer (Log Lines)

**Where:** `LogViewState.lines` and `ServiceLogState.lines` in `model.rs`

**What it is:** `VecDeque<String>` is a double-ended queue backed by a growable ring buffer. We cap it at 5,000 lines (container logs) or 10,000 lines (service logs). When the cap is reached, we `pop_front()` the oldest line before pushing a new one to the back.

**Why not `Vec<String>`?**
A `Vec` stores elements contiguously. Removing from the front (`vec.remove(0)`) is O(n) because every remaining element must be shifted left by one slot. With 10,000 log lines arriving continuously, that means up to 10,000 memcpys on every single new line — a quadratic cost that would visibly stall the TUI.

`VecDeque` is backed by a circular buffer. Both `push_back()` and `pop_front()` are O(1) amortized — no shifting required. The internal pointer simply wraps around.

**Why not `LinkedList`?**
A linked list also offers O(1) push/pop at both ends, but each node is a separate heap allocation with a pointer chase to reach it. Iterating 10,000 nodes scattered across the heap destroys CPU cache locality. `VecDeque`'s contiguous (or at most two-segment) memory layout means the CPU prefetcher can stream data into cache efficiently — critical when we re-render the log view at 10 fps.

**Performance impact:**
- **Before (hypothetical Vec):** Each new log line triggers an O(n) shift of up to 10,000 elements. At ~100 lines/sec, that's ~1,000,000 pointer moves per second just for log bookkeeping.
- **After (VecDeque):** Each new log line is O(1). Total overhead is negligible — a pointer bump and an optional wrap.

---

## 2. HashSet for Expanded/Collapsed State

**Where:** `UIState.expanded_pids`, `ContainerUIState.expanded_ids`, `SwarmUIState.expanded_ids` in `model.rs`

**What it is:** `HashSet<Pid>` or `HashSet<String>` storing which rows the user has expanded in the tree view.

**Why HashSet?**
Every render frame, for every visible row, we ask: "Is this row expanded?" That's a membership test on the hot path. `HashSet::contains()` is O(1) average. A `Vec` would require O(n) linear scan per row, and with 50+ rows visible and up to hundreds of expandable items, that cost adds up inside a render loop that must complete in under 16ms.

**Why not `BTreeSet`?**
`BTreeSet` gives O(log n) lookup and keeps elements sorted. We never need sorted iteration over expanded IDs — we only need "is X in the set?" — so the extra overhead of tree balancing buys us nothing.

**Performance impact:**
On a Swarm cluster with 50 nodes and 100+ services, the overview renderer checks expansion state for every row. With `HashSet`, this is ~150 O(1) lookups per frame. With a `Vec`, it would be ~150 * O(k) where k is the number of expanded items — potentially thousands of comparisons per frame.

---

## 3. HashMap for Stack Grouping

**Where:** `SwarmMonitor::build_stacks()` in `swarm_controller.rs`

**What it is:** A `HashMap<String, Vec<usize>>` that groups services by their stack name in a single pass over the services list.

**Why HashMap?**
We need to go from a flat list of services (each with a `.stack` field) to a grouped structure (stacks containing their services). A HashMap gives us:
- O(1) amortized lookup to find or create a stack bucket.
- O(n) total time for n services — one pass through the list.

The naive alternative — for each unique stack name, scan all services to find matches — would be O(s * n) where s is the number of stacks. With 10 stacks and 100 services, that's 1,000 iterations vs. 100.

**Why not sort + group-by?**
Sorting is O(n log n) and would require the services to be in a sortable order by stack name. The HashMap approach is O(n) and doesn't require modifying the input order. Since we sort the resulting stacks list afterward (a much smaller list), the total cost is O(n + s log s) which is dominated by O(n).

**Performance impact:**
This runs every 3 seconds on a tick. For 100 services across 10 stacks, HashMap grouping does ~100 hash lookups. The sort-then-group alternative would do ~700 comparisons (100 * log2(100)). The HashMap approach is faster and simpler, though for this input size the difference is microseconds. The real win is code clarity.

---

## 4. Vec\<usize\> Indices vs. Cloned Structs (SwarmStackInfo)

**Where:** `SwarmStackInfo.service_indices` in `model.rs`, used throughout `view.rs` and `main.rs`

**What it is:** Instead of `SwarmStackInfo` owning a `Vec<SwarmServiceInfo>` (full clones of each service), it stores `Vec<usize>` — indices into the canonical `SwarmMonitor.services` array.

**Why indices?**
Each `SwarmServiceInfo` contains 7 `String` fields (id, name, mode, replicas, image, ports, stack). Cloning one means 7 heap allocations for the string data plus copying bytes. For 100 services grouped into 10 stacks, that's 700 string clones every 3 seconds — purely wasted work since the data already exists in `SwarmMonitor.services`.

With indices, we store one `usize` (8 bytes) per service per stack. Zero heap allocations, zero string copies.

**Trade-off:**
Accessing a service now requires `services[idx]` instead of direct field access on the stack's own `Vec`. This is a trivial indirection — one pointer dereference into a contiguous `Vec` — and the CPU prefetcher handles it transparently.

**Performance impact:**
- **Before:** 100 services * 7 strings * ~50 bytes avg = ~35 KB of heap allocation + copy per tick.
- **After:** 100 * 8 bytes = 800 bytes of stack-allocated indices per tick. That's a ~44x reduction in allocation pressure on every refresh cycle.

---

## 5. join_all for Concurrent Futures (CPU Stats)

**Where:** `DockerClient::get_all_cpu_percents()` in `docker.rs`, called from `DockerMonitor::update()` in `docker_controller.rs`

**What it is:** `futures_util::future::join_all` runs all CPU-stat requests concurrently on the tokio runtime, rather than awaiting them one by one.

**Why concurrent?**
Each `get_cpu_percent()` call makes an HTTP request to the Docker daemon over the Unix socket, waits for a stats snapshot, and returns. This is I/O-bound — the CPU is idle while waiting for Docker to respond. Sequential execution means the total wall-clock time is `n * latency_per_container`. With `join_all`, all requests are in-flight simultaneously, and total time is approximately `max(latency_per_container)` — bounded by the slowest single response, not the sum.

**Why not `tokio::spawn` per task?**
`join_all` is simpler and avoids spawning N separate tasks on the runtime. Since we need all results before proceeding (we zip them into the container list), `join_all` gives us exactly the right semantics: "run all, wait for all, return all results in order."

**Performance impact:**
- **Before (sequential):** 20 containers * ~50ms per stats call = ~1,000ms blocking the main loop every 3 seconds. The TUI visibly freezes for 1 second on every tick.
- **After (concurrent):** 20 containers in parallel = ~50–80ms total. The main loop blocks for under 100ms — imperceptible to the user.

---

## 6. Batch CLI Calls vs. N+1 Subprocess Spawns (Stack Labels)

**Where:** `batch_get_stack_labels()` in `swarm.rs`

**What it is:** Instead of calling `docker service inspect <id>` once per service to get its stack label, we pass all service IDs to a single invocation: `docker service inspect --format '...' id1 id2 id3 ...`.

**Why batch?**
Each `Command::new("docker").spawn()` forks the current process, execs the docker binary, sets up pipes, waits for the child, and collects output. On Linux, `fork()` + `exec()` costs ~1–5ms depending on process memory size. For 50 services, that's 50 forks = 50–250ms of pure overhead before Docker even processes the request.

A single call with all IDs does one fork, one exec, and Docker inspects all services internally in one pass over its data store.

**Why not use bollard (Docker API) instead of CLI?**
Bollard 0.18 doesn't expose a batch inspect API for services. We'd need to make 50 individual HTTP requests through bollard's async API — faster than 50 subprocesses, but still 50 round-trips. The single CLI call is the simplest path to O(1) process spawns.

**Performance impact:**
- **Before:** 50 services = 50 subprocess spawns = ~150ms of fork/exec overhead + 50 Docker API calls.
- **After:** 1 subprocess spawn = ~3ms + 1 Docker API call handling 50 services internally.

---

## 7. Typed Deserialization vs. serde_json::Value

**Where:** `DockerInfoPartial` / `DockerInfoSwarm` structs in `swarm.rs`, replacing the previous `serde_json::Value` parsing of `docker info` output.

**What it is:** Instead of parsing `docker info --format '{{json .}}'` into a generic `serde_json::Value` tree and then navigating it with `.get("Swarm")?.get("NodeID")?.as_str()?`, we deserialize directly into typed Rust structs.

**Why typed structs?**
1. **Avoids intermediate allocations.** `serde_json::Value` must allocate a `HashMap<String, Value>` for every JSON object and a `Vec<Value>` for every array — even fields we don't care about. Docker's `info` output is a large JSON blob (~5–10 KB). Typed deserialization with `#[serde(rename)]` only allocates for the fields we declare; serde skips unknown fields entirely.
2. **No dynamic lookups.** `.get("Swarm")` on a `Value::Object` is a HashMap lookup (hash + compare). With typed structs, field access is a direct memory offset — zero-cost.
3. **Compile-time safety.** Typos like `.get("Swarrm")` silently return `None` with `Value`. With structs, the compiler catches field name errors.

**Performance impact:**
The `docker info` JSON blob typically contains 100+ fields across nested objects. With `Value`, all of them are parsed and heap-allocated. With typed structs, only the 6 fields we need (`local_node_state`, `node_id`, `node_addr`, `control_available`, `managers`, `nodes`) are extracted. Estimated reduction: ~80% fewer heap allocations during parsing.

---

## 8. mpsc Channels for Log Streaming

**Where:** `tokio::sync::mpsc` in `docker.rs` (container logs), `std::sync::mpsc` in `swarm.rs` (service logs)

**What it is:** Log lines are produced by a background thread/task reading from Docker's streaming API and sent through a multi-producer, single-consumer channel to the main TUI loop.

**Why channels?**
The TUI main loop runs synchronously (polling `crossterm::event` and rendering). Docker log streams are inherently asynchronous and blocking (the stream waits for new lines indefinitely). We need to decouple these two:

- The **producer** blocks on the Docker stream and pushes lines as they arrive.
- The **consumer** (main loop) drains available lines non-blockingly via `try_recv()` on every iteration (~100ms).

Channels give us exactly this: thread-safe, bounded (for tokio mpsc) or unbounded (for std mpsc) queues with zero shared mutable state.

**Why not `Arc<Mutex<Vec<String>>>`?**
A shared mutex would work but introduces lock contention: the producer holds the lock while pushing, and the consumer holds it while draining. If the producer is pushing lines faster than the consumer drains them, the consumer must wait for each push to complete. Channels internally use lock-free or fine-grained locking algorithms optimized for this exact producer/consumer pattern.

**Why tokio mpsc for containers but std mpsc for Swarm?**
Container logs use bollard's async streaming API, which runs on the tokio runtime — so `tokio::sync::mpsc` is the natural fit (it's `Send`-compatible and wakes the receiver task). Swarm service logs use `std::process::Command` with `BufReader`, which runs in a standard OS thread — so `std::sync::mpsc` avoids pulling in async machinery for a synchronous producer.

**Performance impact:**
The bounded channel (`tokio::sync::mpsc::channel(256)`) provides backpressure: if the consumer falls behind, the producer pauses instead of consuming unlimited memory. The `try_recv()` drain loop (up to 100–200 lines per poll) ensures the TUI stays responsive even under heavy log throughput.

---

## 9. Arc\<Runtime\> for Shared Tokio Runtime

**Where:** `Arc<tokio::runtime::Runtime>` created in `main.rs`, shared with `DockerMonitor`

**What it is:** A single tokio multi-threaded runtime (2 worker threads) wrapped in an `Arc` so it can be stored in `DockerMonitor` while the main function retains a reference.

**Why one shared runtime?**
Creating a tokio runtime is expensive — it spawns OS threads, sets up I/O drivers, and allocates internal scheduler state. If each component created its own runtime, we'd waste threads and memory. A single runtime with 2 worker threads is sufficient for our workload (Docker API calls are I/O-bound, not CPU-bound).

**Why `Arc` and not just passing ownership?**
`DockerMonitor::new()` needs the runtime handle to check Docker availability (`rt.block_on(c.is_available())`), and the main loop may need the runtime reference in the future. `Arc` allows shared ownership without lifetime gymnastics.

**Why `block_on` instead of making main async?**
The TUI main loop must be synchronous — `crossterm::event::poll()` and `event::read()` are blocking calls that don't play well inside a tokio task. Using `rt.block_on()` at the boundary lets us call async Docker APIs from synchronous code. The `block_on` calls are always short-lived (a single API call), so they don't starve the TUI event loop.

**Performance impact:**
2 worker threads handle all Docker API calls concurrently. The thread pool is reused across every tick, avoiding the cost of thread creation/destruction. Total memory overhead: ~200 KB for the runtime + 2 OS threads (~16 KB stack each).

---

## 10. Tick Counter for Rate-Limiting Expensive Probes

**Where:** `tick_counter` in the main loop (`main.rs`), used as `tick_counter % 10 == 0` to gate `recheck_swarm()`

**What it is:** A simple `u64` counter incremented every 3-second tick. Modular arithmetic determines when to run infrequent operations.

**Why a counter instead of a separate `Instant` timer?**
Using `Instant::now()` and `duration_since()` requires a syscall (`clock_gettime`) on every check. A counter is a register increment and a modulo — pure arithmetic, no syscall. When the check is "every 10th tick," a counter is both simpler and cheaper.

**Why not just run detect_swarm() every tick?**
`detect_swarm()` spawns `docker info --format '{{json .}}'`, which forks a process, execs docker, serializes the entire daemon state to JSON, pipes it back, and deserializes it. This takes 20–100ms. Running it every 3 seconds when we're in Standalone mode (meaning Docker Swarm isn't even active) wastes CPU and I/O for no benefit.

Running it every 30 seconds (10 ticks) is sufficient to detect when a user initializes Swarm mode.

**Performance impact:**
- **Before:** `detect_swarm()` ran every 3 seconds = ~33 subprocess spawns per minute, even when Swarm wasn't active.
- **After:** Runs every 30 seconds = ~2 spawns per minute. A 16x reduction in unnecessary process spawns.

---

## 11. Linear Scan for Virtual List Resolution

**Where:** `resolve_swarm_overview_item()` in `main.rs`

**What it is:** Given a selected row index in the Swarm overview (which is a virtual list of headers, nodes, stacks, and services), we walk through the list structure counting rows until we reach the selected index, then return what semantic item is at that position.

**Why linear scan?**
The virtual list is small — typically under 200 rows even for a large cluster (50 nodes + 10 stacks + 100 services). A linear scan of 200 items takes microseconds. Building and maintaining a lookup table (e.g., `Vec<SwarmOverviewItem>`) would require regenerating it on every expand/collapse action and every data refresh — more code, more allocations, and no measurable speedup.

**When would this need to change?**
If the list grew to thousands of items (e.g., monitoring 1,000+ services), a pre-built index would be worthwhile. The crossover point where a `Vec<SwarmOverviewItem>` lookup table pays for itself is roughly when the scan takes longer than the allocation + rebuild cost — likely around 5,000+ rows.

**Performance impact:**
At 200 rows, the scan takes ~0.2 microseconds. This runs once per keypress, not per frame. The cost is entirely negligible for the foreseeable scale of Swarm clusters (20–50 nodes).

---

## 12. Active-View-Only Refresh

**Where:** The main tick handler in `main.rs`

**What it is:** On each 3-second tick, only the monitor corresponding to the currently active tab (System, Containers, or Swarm) performs its data refresh. The other monitors are skipped.

**Why selective refresh?**
Each monitor's `update()` has non-trivial cost:
- **System monitor:** Reads `/proc` (Linux) or calls `sysctl`/`nettop` (macOS) — ~50ms.
- **Docker monitor:** Makes HTTP calls to the Docker daemon for container list + stats — ~50–100ms.
- **Swarm monitor:** Spawns 2–3 `docker` subprocesses (node ls, service ls, service ps) — ~100–300ms.

Running all three every 3 seconds means ~200–450ms of blocking I/O per tick — nearly half a second of freeze in a 3-second cycle. The user only sees one tab at a time, so refreshing invisible tabs is pure waste.

**Trade-off:**
When the user switches tabs, the first render shows slightly stale data (up to 3 seconds old). The next tick immediately refreshes the new active view. In practice, this is invisible — the user sees fresh data within one tick after switching.

**Performance impact:**
- **Before:** ~200–450ms of I/O per 3-second tick.
- **After:** ~50–150ms per tick (only the active monitor). This is a 2–3x reduction in per-tick blocking time, directly translating to a more responsive TUI.

---

## Summary Table

| Decision | Data Structure / Algorithm | Complexity | Key Benefit |
|---|---|---|---|
| Log buffer | `VecDeque` (ring buffer) | O(1) push/pop | No shifting, cache-friendly iteration |
| Expansion tracking | `HashSet` | O(1) contains | Fast per-row membership test in render loop |
| Stack grouping | `HashMap` | O(n) single pass | Linear grouping, no re-scanning |
| Stack references | `Vec<usize>` indices | O(1) access | Zero-clone, ~44x less allocation |
| CPU stats | `join_all` concurrent | O(max latency) | ~20x faster than sequential for 20 containers |
| Stack labels | Batch CLI call | O(1) process spawn | ~50x fewer fork/exec calls |
| Docker info parsing | Typed `Deserialize` | O(fields needed) | ~80% fewer heap allocations |
| Log streaming | `mpsc` channels | O(1) send/recv | Lock-free decoupling, bounded backpressure |
| Async runtime | `Arc<Runtime>` | Shared 2 threads | Single runtime, zero per-call overhead |
| Slow probe gating | Tick counter + modulo | O(1) arithmetic | 16x fewer subprocess spawns when idle |
| Virtual list lookup | Linear scan | O(rows) | Simple, fast enough for <200 rows |
| Selective refresh | Active-view-only | Skips 2 of 3 monitors | 2–3x less blocking I/O per tick |
