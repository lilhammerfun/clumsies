//! Where the client says what it is doing.
//!
//! The window is the product, not a debug console, so the client keeps its own
//! words out of the UI and puts them here instead: a line per event, on stderr
//! — which the dev instance captures into its own logs — and in a file when one
//! is named. Every line carries the time since the client started, which is
//! what makes a log readable when a reader is following one action.
//!
//! The file is CLUMSIES_DESKTOP_LOG when that is set, and otherwise the file
//! beside the daemon this client talks to: the two halves' logs belong in the
//! same place. With neither, stderr is the whole story.
//!
//! A panic is logged before the process goes, so a crash leaves a reason behind
//! rather than a window that closed.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Log {
    started: Instant,
    file: Option<Mutex<File>>,
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Starts the log and hooks panics into it. Call once, before the window.
pub fn init() {
    let log = Log {
        started: Instant::now(),
        file: log_file().map(Mutex::new),
    };
    let _ = LOG.set(log);

    std::panic::set_hook(Box::new(|panic| {
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panicked without a message");
        let place = panic
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "an unknown place".to_owned());
        error(&format!("panic at {place}: {message}"));
    }));
}

pub fn info(message: &str) {
    write("info", message);
}

pub fn error(message: &str) {
    write("error", message);
}

fn write(level: &str, message: &str) {
    let line = match LOG.get() {
        Some(log) => format!(
            "clumsies-desktop +{:.3}s {level:5} {message}",
            log.started.elapsed().as_secs_f64()
        ),
        None => format!("clumsies-desktop {level:5} {message}"),
    };
    eprintln!("{line}");
    if let Some(Some(file)) = LOG.get().map(|log| log.file.as_ref())
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// The file this client logs to, if it has one.
fn log_file() -> Option<File> {
    let path = match std::env::var("CLUMSIES_DESKTOP_LOG") {
        Ok(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => {
            let root = std::env::var("CLUMSIES_DAEMON_ROOT").ok()?;
            std::path::Path::new(&root).join("desktop.log")
        }
    };
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}
