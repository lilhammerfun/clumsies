//! Start the packaged daemon when no instance is reachable.
use clumsiesd::{DaemonConfig, DaemonIpcClient};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn ensure_daemon() -> Result<(), String> {
    let client =
        DaemonIpcClient::new("ai.clumsies.daemon").with_timeout(Duration::from_millis(500));
    if client.health().is_ok() {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let daemon = executable.with_file_name(if cfg!(windows) {
        "clumsiesd.exe"
    } else {
        "clumsiesd"
    });
    if !daemon.is_file() {
        return Err("The bundled clumsiesd engine is missing.".into());
    }
    let config = DaemonConfig::from_env().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&config.log_dir).map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config.log_dir.join("clumsiesd.err.log"))
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(daemon);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start the engine: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if client.health().is_ok() {
            // The daemon stays alive for Agent sessions after the UI closes.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            // A simultaneous client may have acquired the daemon lock first.
            if client.health().is_ok() {
                return Ok(());
            }
            return Err(format!(
                "The engine exited during startup ({status}). See its logs."
            ));
        }
        if Instant::now() >= deadline {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Err("The engine did not become ready within 15 seconds. See its logs.".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
