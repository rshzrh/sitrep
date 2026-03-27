use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "sitrep", version, about = "Real-time terminal diagnostic tool for server triage")]
pub struct Cli {
    /// Refresh interval in seconds
    #[arg(long, default_value = "3")]
    pub refresh_rate: u64,

    /// Disable Docker container monitoring
    #[arg(long)]
    pub no_docker: bool,

    /// Log file path (default: ~/.sitrep/sitrep.log)
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Log level: error, warn, info, debug, trace
    #[arg(long, default_value = "info")]
    pub log_level: String,
}
