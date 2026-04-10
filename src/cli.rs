use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "flotop",
    version,
    about = "Agentless multi-host TUI for triaging your fleet of VPSes when something breaks at 2am"
)]
pub struct Cli {
    /// Hosts to monitor over SSH. Each is `[user@]host[:port]`. If empty,
    /// flotop runs in single-host (local) mode like the legacy sitrep.
    #[arg(value_name = "HOST")]
    pub hosts: Vec<String>,

    /// Default SSH user when a host arg doesn't specify one.
    #[arg(long, default_value = "root")]
    pub user: String,

    /// Path to an SSH private key. Defaults to ~/.ssh/id_ed25519,
    /// then ~/.ssh/id_rsa.
    #[arg(long)]
    pub ssh_key: Option<PathBuf>,

    /// Print a Markdown snapshot of the system (or fleet) and exit.
    /// Use with `--anonymize` to redact hostnames before sharing.
    #[arg(long)]
    pub snapshot: bool,

    /// Anonymize hostnames in --snapshot output (host1, host2, ...).
    #[arg(long)]
    pub anonymize: bool,

    /// Refresh interval in seconds for the LOCAL collector. Remote
    /// hosts use a separate (slower) cadence — see --remote-refresh.
    #[arg(long, default_value = "3")]
    pub refresh_rate: u64,

    /// Refresh interval in seconds for REMOTE hosts. Default 5s to keep
    /// SSH bandwidth low.
    #[arg(long, default_value = "5")]
    pub remote_refresh: u64,

    /// Disable Docker container monitoring
    #[arg(long)]
    pub no_docker: bool,

    /// Log file path (default: ~/.flotop/flotop.log)
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Log level: error, warn, info, debug, trace
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

impl Cli {
    /// Resolve the SSH key path (single). Kept for callers that only
    /// want one key. Prefer `resolve_ssh_keys` for multi-key fallback.
    pub fn resolve_ssh_key(&self) -> Option<PathBuf> {
        self.resolve_ssh_keys().into_iter().next()
    }

    /// Resolve all candidate SSH key paths to try, in preference order.
    ///
    /// * If `--ssh-key` is explicitly set, that's the only candidate.
    /// * Otherwise, returns every `~/.ssh/id_*` private key that exists,
    ///   in order: ed25519, ecdsa, rsa. flotop tries each until one
    ///   authenticates successfully. This matches the behavior of the
    ///   `ssh` command when it tries all identities.
    pub fn resolve_ssh_keys(&self) -> Vec<PathBuf> {
        if let Some(p) = &self.ssh_key {
            return vec![p.clone()];
        }
        let home = match std::env::var("HOME").ok().map(PathBuf::from) {
            Some(h) => h,
            None => return Vec::new(),
        };
        let ssh_dir = home.join(".ssh");
        ["id_ed25519", "id_ecdsa", "id_rsa"]
            .iter()
            .map(|name| ssh_dir.join(name))
            .filter(|p| p.exists())
            .collect()
    }
}
