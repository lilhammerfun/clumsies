//! Standalone runtime discovery and bounded startup, shared by CLI and MCP.

use std::path::PathBuf;
#[cfg(not(target_os = "macos"))]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{DaemonConfig, DaemonError, DaemonIpcClient};

/// Finds the resident shipped with this executable, or the App-owned macOS resident.
///
/// # Errors
/// Returns an error when the executable location or resident cannot be resolved.
pub fn binary() -> Result<PathBuf, DaemonError> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    {
        let sibling = executable.with_file_name("clumsiesd");
        if sibling.ends_with("Contents/Resources/clumsiesd") && sibling.is_file() {
            return Ok(sibling);
        }
        for app in [
            PathBuf::from("/Applications/Clumsies.app"),
            crate::util::home_dir()?.join("Applications/Clumsies.app"),
        ] {
            let daemon = app.join("Contents/Resources/clumsiesd");
            if daemon.is_file() {
                return Ok(daemon);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let sibling = executable.with_file_name(if cfg!(windows) {
            "clumsiesd.exe"
        } else {
            "clumsiesd"
        });
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Err(DaemonError::InvalidConfig(
        "Install clumsies and clumsiesd from the same package (macOS requires Clumsies.app)"
            .to_owned(),
    ))
}

/// Reuses a compatible resident or starts the standalone user runtime once.
/// macOS delegates startup to the App's existing launch agent.
///
/// # Errors
/// Fails on incompatible protocol, missing installation, or startup timeout.
pub fn ensure_running() -> Result<DaemonIpcClient, DaemonError> {
    // Official installers keep this lock outside the atomically replaced runtime directory.
    // Refuse a new startup while either executable is being upgraded.
    #[cfg(not(target_os = "macos"))]
    let _installation_lock = {
        let path = binary()?
            .parent()
            .and_then(|directory| directory.parent())
            .map(|directory| directory.join(".install.lock"));
        if let Some(path) = path.filter(|path| path.exists()) {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?;
            file.try_lock_shared().map_err(|_| {
                DaemonError::InvalidRequest(
                    "Clumsies is being installed or upgraded; retry when installation completes"
                        .to_owned(),
                )
            })?;
            Some(file)
        } else {
            None
        }
    };
    let config = DaemonConfig::from_env()?;
    let client = DaemonIpcClient::new(config.mach_service_name.clone());
    let probe = client.clone().with_timeout(Duration::from_secs(1));
    if let Ok(health) = probe.health() {
        crate::agent_runtime::validate_identity(&health.agent_runtime)?;
        return Ok(client);
    }
    #[cfg(target_os = "macos")]
    {
        let agent = crate::LaunchAgentConfig::from_daemon_config(&config, binary()?)?;
        crate::LaunchAgentController::for_current_user(agent)?.reconcile()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut command = Command::new(binary()?);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window and no inherited parent console lifetime.
            command.creation_flags(0x08000000 | 0x00000008);
        }
        let mut child = command.spawn()?;
        // Reap a losing concurrent starter without tying resident lifetime to the CLI.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(health) = probe.health() {
            crate::agent_runtime::validate_identity(&health.agent_runtime)?;
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(DaemonError::InvalidConfig(format!(
                "Daemon did not become ready; inspect {}",
                config.log_dir.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stops the standalone resident and waits until its root lock is released.
///
/// # Errors
/// Returns an error when the App owns the runtime or shutdown fails or times out.
pub fn stop() -> Result<(), DaemonError> {
    if cfg!(target_os = "macos") {
        return Err(DaemonError::InvalidRequest(
            "The macOS App manages daemon lifecycle".to_owned(),
        ));
    }
    let config = DaemonConfig::from_env()?;
    let client =
        DaemonIpcClient::new(config.mach_service_name.clone()).with_timeout(Duration::from_secs(2));
    let lock_path = config.root_dir.join("daemon.lock");
    if !lock_path.exists() {
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)?;
    if lock.try_lock().is_ok() {
        return Ok(());
    }
    client
        .call(crate::DaemonIpcRequest::empty("daemon_shutdown"))?
        .into_payload::<serde_json::Value>()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while lock.try_lock().is_err() {
        if Instant::now() >= deadline {
            return Err(DaemonError::InvalidRequest(
                "Daemon is still running; installation was not changed".to_owned(),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
