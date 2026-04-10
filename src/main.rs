use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use clap::Parser;

use flotop::app;
use flotop::cli::Cli;

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Set up logging — _guard must live for the entire program
    let _guard = setup_logging(&cli);

    tracing::info!(
        "flotop starting, refresh_rate={}s, no_docker={}, hosts={}, snapshot={}",
        cli.refresh_rate,
        cli.no_docker,
        cli.hosts.len(),
        cli.snapshot
    );

    // ── --snapshot mode: generate Markdown and exit, don't enter TUI ──
    if cli.snapshot {
        return run_snapshot_mode(&cli);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        app::restore_terminal();
        default_hook(info);
    }));

    let should_quit = Arc::new(AtomicBool::new(false));
    {
        let quit_flag = Arc::clone(&should_quit);
        let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, quit_flag);
    }
    {
        let quit_flag = Arc::clone(&should_quit);
        let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, quit_flag);
    }

    let result = app::run(should_quit, &cli);
    tracing::info!("flotop exiting");
    result
}

/// `flotop --snapshot [--anonymize] host1 host2 …` — connect to each host,
/// fetch one snapshot, render Markdown to stdout, exit.
fn run_snapshot_mode(cli: &Cli) -> io::Result<()> {
    use flotop::collectors::remote::{RemoteLinuxCollector, SshAuth};
    use flotop::snapshot::{HostSnapshotInput, render_snapshot_markdown};
    use std::time::Duration;

    if cli.hosts.is_empty() {
        eprintln!("--snapshot requires at least one host argument (v0.1).");
        eprintln!("Local-host snapshot is a v0.2 feature. Run:");
        eprintln!("  flotop --snapshot user@host1 user@host2 > report.md");
        std::process::exit(2);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");

    let keys = cli.resolve_ssh_keys();
    let user = cli.user.clone();
    let hosts = cli.hosts.clone();

    let inputs: Vec<HostSnapshotInput> = rt.block_on(async {
        let mut out = Vec::with_capacity(hosts.len());
        for raw in hosts {
            let mut auth = SshAuth::parse(&raw, &user);
            auth.key_candidates = keys.clone();
            let display = auth.display();
            let mut collector = RemoteLinuxCollector::new(auth);
            let result = async {
                tokio::time::timeout(Duration::from_secs(15), collector.connect())
                    .await
                    .map_err(|_| "connect timeout".to_string())?
                    .map_err(|e| e.to_string())?;
                tokio::time::timeout(Duration::from_secs(15), collector.collect_snapshot())
                    .await
                    .map_err(|_| "snapshot timeout".to_string())?
                    .map_err(|e| e.to_string())
            }
            .await;
            out.push(HostSnapshotInput {
                display_name: display,
                data: result,
            });
        }
        out
    });

    let md = render_snapshot_markdown(&inputs, cli.anonymize);
    print!("{}", md);
    Ok(())
}

fn setup_logging(cli: &Cli) -> tracing_appender::non_blocking::WorkerGuard {
    let log_path = cli.log_file.clone().unwrap_or_else(|| {
        let mut p = dirs_or_home();
        p.push(".flotop");
        p.push("flotop.log");
        p
    });

    let log_dir = log_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let log_filename = log_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("flotop.log"));

    // Create log directory if it doesn't exist
    let _ = std::fs::create_dir_all(log_dir);

    let file_appender = tracing_appender::rolling::daily(log_dir, log_filename);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = cli
        .log_level
        .parse::<tracing_subscriber::filter::LevelFilter>()
        .unwrap_or(tracing_subscriber::filter::LevelFilter::INFO);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_max_level(filter)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .init();

    guard
}

fn dirs_or_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
