use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use clap::Parser;

use sitrep::app;
use sitrep::cli::Cli;

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Set up logging — _guard must live for the entire program
    let _guard = setup_logging(&cli);

    tracing::info!(
        "sitrep starting, refresh_rate={}s, no_docker={}",
        cli.refresh_rate,
        cli.no_docker
    );

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
    tracing::info!("sitrep exiting");
    result
}

fn setup_logging(cli: &Cli) -> tracing_appender::non_blocking::WorkerGuard {
    let log_path = cli.log_file.clone().unwrap_or_else(|| {
        let mut p = dirs_or_home();
        p.push(".sitrep");
        p.push("sitrep.log");
        p
    });

    let log_dir = log_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let log_filename = log_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("sitrep.log"));

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
