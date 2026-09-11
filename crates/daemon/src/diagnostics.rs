//! Bounded local logs and safe request metadata. Never record bodies or credentials.
use crate::DaemonError;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

tokio::task_local! { pub(crate) static REQUEST_ID: String; }

pub(crate) fn request_id() -> String {
    REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| new_request_id())
}

pub(crate) fn new_request_id() -> String {
    format!("req_{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn validated_request_id(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.'))
    .then(|| value.to_owned())
}

// Only the API resource family is logged: nested paths can contain Memory paths or user IDs.
pub(crate) fn route(path: &str) -> String {
    let resource = path
        .split('?')
        .next()
        .unwrap_or("")
        .split('/')
        .nth(3)
        .unwrap_or("");
    match resource {
        "reviews" | "drafts" | "projects" | "auth" | "me" | "health" | "setup" | "memories"
        | "users" | "organizations" | "commits" => format!("/api/v1/{resource}"),
        _ => "/api/v1/:resource".to_owned(),
    }
}

pub(crate) fn error_kind(error: &DaemonError) -> &'static str {
    match error {
        DaemonError::Reqwest(e) if e.is_timeout() => "timeout",
        DaemonError::Reqwest(e) if e.is_connect() => "connect",
        DaemonError::Reqwest(e) if e.is_decode() => "decode",
        DaemonError::Reqwest(_) => "transport",
        DaemonError::Io(_) => "io",
        DaemonError::Sqlx(_) => "database",
        DaemonError::SerdeJson(_) => "json",
        DaemonError::ServerResponse { .. } => "http_status",
        DaemonError::CredentialStore(_) => "credentials",
        DaemonError::Ipc(_) | DaemonError::Remote(_) => "ipc",
        _ => "application",
    }
}

pub(crate) fn transport_details(error: &reqwest::Error) -> Value {
    use std::error::Error;
    let mut causes = Vec::new();
    let mut source = error.source();
    // Source Display strings can contain URLs. Retain causal categories and OS codes only.
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<io::Error>() {
            causes.push(json!({"kind": format!("{:?}", io.kind()), "os_code": io.raw_os_error()}));
        } else {
            let description = cause.to_string().to_ascii_lowercase();
            let kind = if description.contains("timed out") || description.contains("timeout") {
                "timeout"
            } else if description.contains("dns") || description.contains("resolve") {
                "dns"
            } else if description.contains("tls") || description.contains("certificate") {
                "tls"
            } else if description.contains("connection") || description.contains("connect") {
                "connection"
            } else if description.contains("body") {
                "body"
            } else {
                "transport"
            };
            causes.push(json!({"kind": kind}));
        }
        if causes.len() == 8 {
            break;
        }
        source = cause.source();
    }
    json!({"timeout": error.is_timeout(), "connect": error.is_connect(), "body": error.is_body(), "decode": error.is_decode(), "causes": causes})
}

pub(crate) fn http_error(error: reqwest::Error, stage: &'static str) -> DaemonError {
    tracing::warn!(event = "http_failed", request_id = %request_id(), stage, details = %transport_details(&error));
    DaemonError::Reqwest(error.without_url())
}

pub(crate) fn error_details(error: &DaemonError) -> Value {
    match error {
        DaemonError::Reqwest(error) => transport_details(error),
        DaemonError::ServerResponse { status, .. } => json!({"status": status}),
        DaemonError::Io(error) => {
            json!({"kind": format!("{:?}", error.kind()), "os_code": error.raw_os_error()})
        }
        DaemonError::Sqlx(sqlx::Error::Database(error)) => json!({"database_code": error.code()}),
        DaemonError::Sqlx(sqlx::Error::PoolTimedOut) => json!({"kind": "pool_timeout"}),
        DaemonError::Sqlx(sqlx::Error::PoolClosed) => json!({"kind": "pool_closed"}),
        DaemonError::State { code, .. } => json!({"code": code}),
        _ => json!({}),
    }
}

pub(crate) fn worker_result<T>(
    worker: &'static str,
    failure: &mut (u64, String),
    result: &Result<T, DaemonError>,
) {
    match result {
        Err(error) => {
            let kind = error_kind(error);
            let details = error_details(error);
            let signature = format!("{kind}:{details}");
            if failure.1 != signature {
                *failure = (0, signature);
            }
            failure.0 = failure.0.saturating_add(1);
            if failure.0.is_power_of_two() {
                tracing::warn!(event = "worker_failed", worker, failures = failure.0,
                    request_id = %request_id(), kind, %details);
            }
        }
        Ok(_) if failure.0 > 0 => {
            tracing::info!(event = "worker_recovered", worker, failures = failure.0, request_id = %request_id());
            *failure = (0, String::new());
        }
        Ok(_) => {}
    }
}

/// Four files of at most 4 MiB each, including the active file. One resident writer.
/// ponytail: synchronous local writes; use a bounded writer queue if logging blocks request work.
pub struct RotatingLog {
    path: PathBuf,
    file: File,
    bytes: u64,
    limit: u64,
    reported_failure: bool,
}

impl RotatingLog {
    pub fn new(path: &Path) -> io::Result<Self> {
        Self::with_limit(path, 4 * 1024 * 1024)
    }

    fn with_limit(path: &Path, limit: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        for index in 0..=3 {
            let old_path = if index == 0 {
                path.to_owned()
            } else {
                path.with_extension(format!("log.{index}"))
            };
            if let Ok(metadata) = std::fs::metadata(&old_path)
                && metadata.len() > limit
            {
                let mut old = OpenOptions::new().read(true).write(true).open(&old_path)?;
                old.seek(SeekFrom::End(-(limit as i64)))?;
                let mut tail = Vec::with_capacity(limit as usize);
                old.read_to_end(&mut tail)?;
                old.set_len(0)?;
                old.seek(SeekFrom::Start(0))?;
                old.write_all(&tail)?;
            }
        }
        let file = Self::open(path)?;
        let bytes = file.metadata()?.len();
        let mut writer = Self {
            path: path.to_owned(),
            file,
            bytes,
            limit,
            reported_failure: false,
        };
        if bytes >= limit {
            writer.rotate()?;
        }
        Ok(writer)
    }

    fn open(path: &Path) -> io::Result<File> {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        options.open(path)
    }

    fn rotate(&mut self) -> io::Result<()> {
        for index in (1..=3).rev() {
            let destination = self.path.with_extension(format!("log.{index}"));
            let source = if index == 1 {
                self.path.clone()
            } else {
                self.path.with_extension(format!("log.{}", index - 1))
            };
            match std::fs::rename(source, destination) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        self.file = Self::open(&self.path)?;
        self.bytes = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() as u64 > self.limit {
            // Discard an oversized event, never a partial JSON/line containing an unbounded payload.
            let marker = b"diagnostic event exceeded log size limit\n";
            if marker.len() as u64 <= self.limit {
                self.write_all(marker)?;
            }
            return Ok(buffer.len());
        }
        let result = (|| {
            if self.bytes + buffer.len() as u64 > self.limit {
                self.rotate()?;
            }
            self.file.write_all(buffer)?;
            self.bytes += buffer.len() as u64;
            Ok(buffer.len())
        })();
        if let Err(ref error) = result {
            if !self.reported_failure {
                eprintln!(
                    "diagnostic_log_write_failed kind={:?}",
                    io::Error::kind(error)
                );
            }
            self.reported_failure = true;
        } else {
            self.reported_failure = false;
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_errors_are_bounded_and_recovery_is_visible() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("worker.log");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(File::create(&path).unwrap())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut failures = (0, String::new());
        for _ in 0..9 {
            worker_result::<()>(
                "test",
                &mut failures,
                &Err(DaemonError::Server("SECRET_BODY".to_owned())),
            );
        }
        worker_result::<()>(
            "test",
            &mut failures,
            &Err(DaemonError::Io(io::Error::from_raw_os_error(28))),
        );
        worker_result("test", &mut failures, &Ok(()));
        let logs = std::fs::read_to_string(path).unwrap();
        assert_eq!(logs.matches("worker_failed").count(), 5);
        assert!(logs.contains("worker_recovered"));
        assert!(!logs.contains("SECRET_BODY"));
        assert_eq!(failures.0, 0);
        assert!(logs.contains("os_code"));
    }

    #[test]
    fn rotation_bounds_retention_and_route_hides_user_input() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("test.log");
        std::fs::write(&path, vec![b'x'; 100]).unwrap();
        std::fs::write(path.with_extension("log.3"), vec![b'x'; 100]).unwrap();
        let mut log = RotatingLog::with_limit(&path, 32).unwrap();
        assert!(
            std::fs::read_dir(root.path())
                .unwrap()
                .all(|f| f.unwrap().metadata().unwrap().len() <= 32)
        );
        log.write_all(&[b'x'; 100]).unwrap();
        for _ in 0..20 {
            log.write_all(b"01234567890123456789\n").unwrap();
        }
        let files: Vec<_> = std::fs::read_dir(root.path()).unwrap().collect();
        assert_eq!(files.len(), 4);
        assert!(
            files
                .iter()
                .all(|f| f.as_ref().unwrap().metadata().unwrap().len() <= 32)
        );
        assert_eq!(
            route("/api/v1/projects/secret/path?token=secret"),
            "/api/v1/projects"
        );
        assert_eq!(route("/api/v1/secret"), "/api/v1/:resource");
        assert!(validated_request_id("secret\nheader").is_none());
    }
}
