//! The macOS diagnostic export contract: bounded log files and build metadata.
use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const LOG_LIMIT: u64 = 4 * 1024 * 1024;

pub fn export_diagnostics(
    destination: &Path,
    log_directory: Option<PathBuf>,
) -> Result<(), String> {
    let health = client().health().ok();
    let directory = log_directory.or_else(|| health.as_ref().map(|h| PathBuf::from(&h.log_dir)));
    let mut files = Vec::new();
    let mut manifest = serde_json::json!({
        "app_version":env!("CARGO_PKG_VERSION"),
        "os":std::env::consts::OS,
        "arch":std::env::consts::ARCH,
        "app_pid":std::process::id(),
        "collected_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs(),
        "daemon_version":health.as_ref().map(|h| &h.daemon_version),
        "daemon_build":health.as_ref().map(|h| &h.agent_runtime.build_id),
        "files":{}
    });
    if let Some(path) = crate::logging::path() {
        append_log(&path, "client.log", &mut files, &mut manifest);
    }
    if let Some(directory) = directory {
        for family in [
            "daemon.log",
            "clumsiesd.crash.log",
            "clumsiesd.err.log",
            "clumsiesd.out.log",
        ] {
            for index in 0..=3 {
                let name = if index == 0 {
                    family.to_owned()
                } else {
                    format!("{family}.{index}")
                };
                append_log(&directory.join(&name), &name, &mut files, &mut manifest);
            }
        }
    }
    use sha2::{Digest, Sha256};
    if let Ok(executable) = std::env::current_exe().and_then(std::fs::File::open) {
        let mut hash = Sha256::new();
        let mut reader = std::io::BufReader::new(executable);
        let mut buffer = [0u8; 65536];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        manifest["app_executable_sha256"] = format!("{:x}", hash.finalize()).into();
    }
    files.push((
        "manifest.json".into(),
        false,
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    ));
    export_memory(&files, destination)
}

fn append_log(
    path: &Path,
    name: &str,
    files: &mut Vec<(String, bool, String)>,
    manifest: &mut serde_json::Value,
) {
    let result = (|| -> std::io::Result<(String, bool)> {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(std::io::Error::other("Not a regular log file"));
        }
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(meta.len().saturating_sub(LOG_LIMIT)))?;
        let mut bytes = Vec::new();
        file.take(LOG_LIMIT).read_to_end(&mut bytes)?;
        Ok((
            String::from_utf8_lossy(&bytes).into_owned(),
            meta.len() > LOG_LIMIT,
        ))
    })();
    match result {
        Ok((text, truncated)) => {
            files.push((name.into(), false, text));
            manifest["files"][name] = if truncated { "last 4 MiB" } else { "included" }.into();
        }
        Err(error) => {
            manifest["files"][name] = if error.kind() == std::io::ErrorKind::NotFound {
                "missing"
            } else {
                "unreadable"
            }
            .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_bounds_logs_and_excludes_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("daemon.log");
        std::fs::write(&path, vec![b'x'; LOG_LIMIT as usize + 10]).unwrap();
        let mut files = Vec::new();
        let mut manifest = serde_json::json!({"files":{}});
        append_log(&path, "daemon.log", &mut files, &mut manifest);
        assert_eq!(files[0].2.len(), LOG_LIMIT as usize);
        assert_eq!(manifest["files"]["daemon.log"], "last 4 MiB");
        #[cfg(unix)]
        {
            let link = root.path().join("link.log");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            append_log(&link, "link.log", &mut files, &mut manifest);
            assert_eq!(files.len(), 1);
            assert_eq!(manifest["files"]["link.log"], "unreadable");
        }
    }
}
