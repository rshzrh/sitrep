use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use sitrep::app;

fn main() -> io::Result<()> {
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

    app::run(should_quit)
}
