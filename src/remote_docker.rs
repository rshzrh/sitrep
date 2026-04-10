//! Docker container data collection over SSH.
//!
//! Pattern:
//!   1. Send `docker ps --format '{{json .}}'` etc. over a russh session
//!      channel (one command per channel — russh supports many
//!      concurrent channels per session).
//!   2. Parse each stdout line as a JSON object via `serde_json`.
//!   3. Merge `ps` + `stats` + `inspect` streams by container ID into
//!      the existing `DockerContainerInfo` model struct.
//!
//! The parsers are pure functions that take `&str` and return
//! `Vec<DockerContainerInfo>` (or partial structs). All I/O happens in
//! the async wrapper functions at the bottom of this file, which are
//! covered by the integration test in `tests/integration_fleet.rs`.
//!
//! Design constraint: NEVER interpolate a container ID into a command
//! string without passing it through `shell_escape::escape`. See the
//! `shell_escape_id` helper + its tests.

use crate::collectors::remote::{ClientHandler, RemoteError, run_remote_script};
use crate::model::DockerContainerInfo;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

// ─── JSON row shapes (one per command) ──────────────────────────────────

/// Row from `docker ps --format '{{json .}}'`.
#[derive(Debug, Clone, Deserialize)]
pub struct DockerPsRow {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Names")]
    pub names: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "State")]
    #[serde(default)]
    pub state: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "RunningFor")]
    #[serde(default)]
    pub running_for: String,
    #[serde(rename = "Ports")]
    #[serde(default)]
    pub ports: String,
}

/// Row from `docker stats --no-stream --format '{{json .}}'`.
#[derive(Debug, Clone, Deserialize)]
pub struct DockerStatsRow {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CPUPerc")]
    pub cpu_perc: String,
}

/// Relevant subset of `docker inspect <id>` output (array of objects).
/// We only care about the network IP address for now — Ports/Image come
/// from the ps row.
#[derive(Debug, Clone, Deserialize)]
pub struct DockerInspectEntry {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "NetworkSettings")]
    #[serde(default)]
    pub network_settings: DockerInspectNetwork,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DockerInspectNetwork {
    #[serde(rename = "Networks")]
    #[serde(default)]
    pub networks: HashMap<String, DockerInspectNetworkDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DockerInspectNetworkDetails {
    #[serde(rename = "IPAddress")]
    #[serde(default)]
    pub ip_address: String,
}

// ─── pure parsers ───────────────────────────────────────────────────────

/// Parse the output of `docker ps --format '{{json .}}'` — one JSON
/// object per line. Returns the parsed rows, silently skipping lines
/// that fail to parse (logged via tracing).
pub fn parse_docker_ps(stdout: &str) -> Vec<DockerPsRow> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<DockerPsRow>(line) {
            Ok(row) => rows.push(row),
            Err(e) => {
                tracing::warn!(line = %line, error = %e, "docker ps JSON parse failed");
            }
        }
    }
    rows
}

/// Parse `docker stats --no-stream --format '{{json .}}'` — same format
/// as ps (one JSON object per line).
pub fn parse_docker_stats(stdout: &str) -> Vec<DockerStatsRow> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<DockerStatsRow>(line) {
            rows.push(row);
        }
    }
    rows
}

/// Parse `docker inspect <ids...>` — a JSON array of inspection objects.
pub fn parse_docker_inspect(stdout: &str) -> Vec<DockerInspectEntry> {
    serde_json::from_str::<Vec<DockerInspectEntry>>(stdout.trim()).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "docker inspect JSON parse failed");
        Vec::new()
    })
}

/// Parse a `CPUPerc` string like `"12.34%"` into a float. Returns 0.0
/// on malformed input.
pub fn parse_cpu_perc(s: &str) -> f64 {
    s.trim_end_matches('%').trim().parse().unwrap_or(0.0)
}

/// Merge `ps`, `stats`, and `inspect` rows into `DockerContainerInfo`.
/// Containers present in `ps` but missing from stats/inspect are still
/// included (they just have zero CPU% and empty IP).
pub fn merge_docker_rows(
    ps: &[DockerPsRow],
    stats: &[DockerStatsRow],
    inspect: &[DockerInspectEntry],
) -> Vec<DockerContainerInfo> {
    let stats_by_id: HashMap<&str, &DockerStatsRow> =
        stats.iter().map(|r| (r.id.as_str(), r)).collect();

    // For inspect, match on short-id prefix since `docker ps` returns the
    // 12-char short ID and `docker inspect` returns the full 64-char ID.
    let inspect_by_prefix: HashMap<String, &DockerInspectEntry> = inspect
        .iter()
        .map(|e| {
            let short = if e.id.len() >= 12 { &e.id[..12] } else { e.id.as_str() };
            (short.to_string(), e)
        })
        .collect();

    ps.iter()
        .map(|p| {
            let short_id = if p.id.len() >= 12 { &p.id[..12] } else { p.id.as_str() };
            let cpu_percent = stats_by_id
                .get(p.id.as_str())
                .map(|s| parse_cpu_perc(&s.cpu_perc))
                .unwrap_or(0.0);
            let ip_address = inspect_by_prefix
                .get(short_id)
                .and_then(|e| {
                    e.network_settings
                        .networks
                        .values()
                        .find(|n| !n.ip_address.is_empty())
                        .map(|n| n.ip_address.clone())
                })
                .unwrap_or_default();
            // `docker ps` Names column may have multiple entries separated
            // by commas; take the first and strip the leading slash.
            let name = p
                .names
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .trim_start_matches('/')
                .to_string();
            DockerContainerInfo {
                id: short_id.to_string(),
                name,
                image: p.image.clone(),
                status: p.status.clone(),
                state: p.state.clone(),
                uptime: p.running_for.clone(),
                cpu_percent,
                ports: p.ports.clone(),
                ip_address,
            }
        })
        .collect()
}

// ─── shell-escaping (defense against injection) ─────────────────────────

/// Quote a container or service ID for safe interpolation into a shell
/// command string. Uses `shell_escape::escape` for POSIX quoting rules.
/// ALWAYS call this before embedding any user-controllable value into a
/// `docker ...` command.
pub fn shell_escape_id(id: &str) -> String {
    shell_escape::unix::escape(id.into()).into_owned()
}

// ─── async SSH wrappers ─────────────────────────────────────────────────

/// Run a single command over a fresh russh session channel and return
/// its stdout as a String. Errors on channel/I/O failure. Non-zero exit
/// is NOT treated as an error — the caller inspects stdout/stderr.
pub async fn run_remote_command(
    session: &russh::client::Handle<ClientHandler>,
    cmd: &str,
) -> Result<String, RemoteError> {
    // `run_remote_script` in `collectors::remote` already handles the
    // `sh -s` + stdin dance. We reuse it by wrapping the command in a
    // one-line sh script.
    let script = format!("{}\n", cmd);
    run_remote_script(session, &script).await
}

/// Fetch + merge containers on the remote. Runs ps, stats, and inspect
/// concurrently via three channels (session supports parallel channels).
pub async fn list_containers_remote(
    session: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<Vec<DockerContainerInfo>, RemoteError> {
    // Kick off all three commands concurrently.
    let ps_fut = run_remote_command(
        session,
        "docker ps --format '{{json .}}' 2>/dev/null",
    );
    let stats_fut = run_remote_command(
        session,
        "docker stats --no-stream --format '{{json .}}' 2>/dev/null",
    );
    // `docker inspect $(docker ps -q)` — running containers only. If
    // there are zero the subshell expands to nothing and `docker inspect`
    // would choke, so guard with an empty-array fallback.
    let inspect_fut = run_remote_command(
        session,
        "ids=$(docker ps -q 2>/dev/null); if [ -n \"$ids\" ]; then docker inspect $ids 2>/dev/null; else echo '[]'; fi",
    );

    let (ps_out, stats_out, inspect_out) = tokio::join!(ps_fut, stats_fut, inspect_fut);
    let ps = parse_docker_ps(&ps_out?);
    let stats = parse_docker_stats(&stats_out?);
    let inspect = parse_docker_inspect(&inspect_out?);

    Ok(merge_docker_rows(&ps, &stats, &inspect))
}

// ─── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/docker/{}", name)).unwrap()
    }

    #[test]
    fn parse_docker_ps_happy_path() {
        let rows = parse_docker_ps(&fixture("docker_ps.jsonl"));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].names, "prod-db-1");
        assert_eq!(rows[0].image, "postgres:15");
        assert_eq!(rows[0].state, "running");
        assert!(rows[0].ports.contains("5432"));
    }

    #[test]
    fn parse_docker_ps_skips_malformed_lines() {
        let input = "{\"ID\":\"abc\",\"Names\":\"x\",\"Image\":\"y\",\"Status\":\"up\"}\n\
                     not json at all\n\
                     {\"ID\":\"def\",\"Names\":\"q\",\"Image\":\"r\",\"Status\":\"up\"}\n";
        let rows = parse_docker_ps(input);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn parse_docker_ps_empty_input_returns_empty() {
        assert!(parse_docker_ps("").is_empty());
    }

    #[test]
    fn parse_docker_stats_happy_path() {
        let rows = parse_docker_stats(&fixture("docker_stats.jsonl"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "abc123def456");
        assert_eq!(rows[0].cpu_perc, "12.34%");
    }

    #[test]
    fn parse_cpu_perc_handles_formatted_strings() {
        assert_eq!(parse_cpu_perc("12.34%"), 12.34);
        assert_eq!(parse_cpu_perc("0.00%"), 0.0);
        assert_eq!(parse_cpu_perc("100%"), 100.0);
        assert_eq!(parse_cpu_perc(""), 0.0);
        assert_eq!(parse_cpu_perc("garbage"), 0.0);
    }

    #[test]
    fn parse_docker_inspect_extracts_ip_addresses() {
        let entries = parse_docker_inspect(&fixture("docker_inspect.json"));
        assert_eq!(entries.len(), 2);
        let prod_db = entries.iter().find(|e| e.id.starts_with("abc123")).unwrap();
        let ip = prod_db
            .network_settings
            .networks
            .values()
            .next()
            .map(|n| n.ip_address.clone())
            .unwrap_or_default();
        assert_eq!(ip, "10.0.1.15");
    }

    #[test]
    fn parse_docker_inspect_empty_array_is_ok() {
        assert_eq!(parse_docker_inspect("[]").len(), 0);
        assert_eq!(parse_docker_inspect("").len(), 0);
    }

    #[test]
    fn merge_docker_rows_happy_path() {
        let ps = parse_docker_ps(&fixture("docker_ps.jsonl"));
        let stats = parse_docker_stats(&fixture("docker_stats.jsonl"));
        let inspect = parse_docker_inspect(&fixture("docker_inspect.json"));
        let merged = merge_docker_rows(&ps, &stats, &inspect);
        assert_eq!(merged.len(), 3);
        let db = merged.iter().find(|c| c.name == "prod-db-1").unwrap();
        assert_eq!(db.id, "abc123def456");
        assert_eq!(db.image, "postgres:15");
        assert!(db.cpu_percent > 12.0 && db.cpu_percent < 13.0);
        assert_eq!(db.ip_address, "10.0.1.15");
        assert!(db.ports.contains("5432"));
    }

    #[test]
    fn merge_docker_rows_container_without_stats_is_still_listed() {
        // The `debug` container (exited) has no stats row — should still
        // appear in the merged output with cpu_percent=0.
        let ps = parse_docker_ps(&fixture("docker_ps.jsonl"));
        let stats = parse_docker_stats(&fixture("docker_stats.jsonl"));
        let merged = merge_docker_rows(&ps, &stats, &[]);
        let debug = merged.iter().find(|c| c.name == "debug").unwrap();
        assert_eq!(debug.cpu_percent, 0.0);
        assert_eq!(debug.state, "exited");
    }

    #[test]
    fn merge_docker_rows_strips_leading_slash_from_names() {
        // docker ps Names column rarely has a leading slash (that's inspect).
        // But defensive: our merger should strip it if present.
        let ps = vec![DockerPsRow {
            id: "abc".into(),
            names: "/my-container".into(),
            image: "x".into(),
            state: "running".into(),
            status: "up".into(),
            running_for: "".into(),
            ports: "".into(),
        }];
        let merged = merge_docker_rows(&ps, &[], &[]);
        assert_eq!(merged[0].name, "my-container");
    }

    #[test]
    fn merge_docker_rows_handles_multi_name_column() {
        let ps = vec![DockerPsRow {
            id: "abc".into(),
            names: "main,alias1,alias2".into(),
            image: "x".into(),
            state: "running".into(),
            status: "up".into(),
            running_for: "".into(),
            ports: "".into(),
        }];
        let merged = merge_docker_rows(&ps, &[], &[]);
        assert_eq!(merged[0].name, "main");
    }

    #[test]
    fn shell_escape_id_quotes_dangerous_input() {
        // A malicious container ID trying to inject a second command.
        let dangerous = "x; rm -rf /";
        let escaped = shell_escape_id(dangerous);
        // Must be quoted (not literal). Actual quoting is single-quote
        // based in POSIX.
        assert!(escaped.contains('\''));
        assert_ne!(escaped, dangerous);
        // When embedded in a command, the whole thing stays inside quotes.
        let cmd = format!("docker restart {}", escaped);
        assert!(cmd.starts_with("docker restart '"));
    }

    #[test]
    fn shell_escape_id_leaves_safe_input_minimally_changed() {
        // A normal short container ID has no special chars — escaped form
        // should be a no-op or trivial wrapping.
        let safe = "abc123def456";
        let escaped = shell_escape_id(safe);
        // Either exactly equal or wrapped — both are acceptable.
        assert!(escaped == safe || escaped == format!("'{}'", safe));
    }
}
