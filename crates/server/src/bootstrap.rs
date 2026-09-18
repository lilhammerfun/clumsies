//! Process startup, ready-file ownership, and bounded graceful shutdown.

use crate::app::auth::AuthService;
use crate::app::build_app;
use crate::app::installation::InstallationService;
use crate::config::ServerConfig;
use crate::infra::database::{DatabaseConfig, connect, run_migrations};
use crate::maintenance::project_authority::{MigrationMode, migrate_project_authority};
use crate::telemetry;
use std::future;
use std::future::{Future, IntoFuture};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(4);
const SERVER_READY_FILE_ENV: &str = "CLUMSIES_SERVER_READY_FILE";

#[derive(Debug, PartialEq, Eq)]
enum DrainResult<T> {
    Finished(T),
    TimedOut,
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init()?;
    let ServerConfig {
        database_url,
        listen_addr,
        public_origin,
    } = ServerConfig::from_env()?;
    let mut ready_file = ServerReadyFile::from_env()?;

    let pool = connect(&DatabaseConfig::from_url(database_url)).await?;
    run_migrations(&pool).await?;
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let listen_addr = listener.local_addr()?;
    let public_origin = match public_origin {
        Some(public_origin) => public_origin,
        None => crate::config::PublicOrigin::for_loopback(listen_addr)?,
    };
    let installation = InstallationService::from_env(pool.clone(), public_origin.secure_cookies())?;
    let auth = AuthService::from_env(pool.clone(), &public_origin).await?;
    let app = build_app(pool, auth, installation);
    ready_file.publish(listen_addr, &public_origin)?;

    tracing::info!(%listen_addr, "clumsies server started");
    let (graceful_shutdown, graceful_shutdown_received) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = graceful_shutdown_received.await;
        })
        .into_future();
    if let DrainResult::Finished(result) = run_with_shutdown_limit(
        server,
        shutdown_signal(),
        graceful_shutdown,
        SHUTDOWN_DRAIN_TIMEOUT,
    )
    .await
    {
        result?;
    }
    tracing::info!("clumsies server stopped");
    Ok(())
}

#[derive(Debug)]
struct ServerReadyFile {
    path: Option<PathBuf>,
    owner_token: String,
    published_file: Option<std::fs::File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadyFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl ServerReadyFile {
    fn from_env() -> Result<Self, std::io::Error> {
        let path = std::env::var_os(SERVER_READY_FILE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::new(path)
    }

    fn new(path: Option<PathBuf>) -> Result<Self, std::io::Error> {
        if let Some(path) = &path {
            if !path.is_absolute() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{SERVER_READY_FILE_ENV} must be an absolute path"),
                ));
            }
            match std::fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!(
                            "refusing to replace existing Server ready file {}",
                            path.display()
                        ),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self {
            path,
            owner_token: uuid::Uuid::new_v4().to_string(),
            published_file: None,
        })
    }

    fn publish(
        &mut self,
        listen_addr: std::net::SocketAddr,
        public_origin: &crate::config::PublicOrigin,
    ) -> Result<(), std::io::Error> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = ready_temporary_path(path, &self.owner_token);
        let mut temporary_created = false;
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            temporary_created = true;
            let mut contents = serde_json::to_vec(&serde_json::json!({
                "listen_addr": listen_addr.to_string(),
                "public_origin": public_origin.as_str(),
                "owner_token": self.owner_token.as_str(),
            }))
            .map_err(std::io::Error::other)?;
            contents.push(b'\n');
            file.write_all(&contents)?;
            file.sync_all()?;

            // A hard link publishes atomically without ever replacing an
            // existing target. Both paths are in the same directory.
            std::fs::hard_link(&temporary, path)?;
            // Keep the inode open so an unlinked replacement cannot reuse its
            // identity before Drop decides whether to remove it.
            self.published_file = Some(file);
            std::fs::remove_file(&temporary)?;
            Ok(())
        })();
        if result.is_err() && temporary_created {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

impl Drop for ServerReadyFile {
    fn drop(&mut self) {
        if let (Some(path), Some(published_file)) = (&self.path, &self.published_file)
            && let Ok(published_metadata) = published_file.metadata()
            && std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                ready_file_identity(&metadata) == ready_file_identity(&published_metadata)
            })
            && ready_file_owner(path).as_deref() == Some(self.owner_token.as_str())
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn ready_file_identity(metadata: &std::fs::Metadata) -> ReadyFileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ReadyFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        ReadyFileIdentity {}
    }
}

fn ready_file_owner(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let value: serde_json::Value = serde_json::from_reader(file).ok()?;
    value.get("owner_token")?.as_str().map(str::to_owned)
}

fn ready_temporary_path(path: &Path, owner_token: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("server-ready.json");
    path.with_file_name(format!(
        ".{file_name}.{}.{owner_token}.tmp",
        std::process::id()
    ))
}

pub async fn run_project_authority_migration(
    expected_plan_hash: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = connect(&DatabaseConfig::from_url(database_url)).await?;
    if expected_plan_hash.is_some() {
        run_migrations(&pool).await?;
    }
    let mode = expected_plan_hash.map_or(MigrationMode::DryRun, |expected_plan_hash| {
        MigrationMode::Apply { expected_plan_hash }
    });
    let report = migrate_project_authority(&pool, mode).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn run_with_shutdown_limit<Server, Shutdown, Output>(
    server: Server,
    shutdown: Shutdown,
    graceful_shutdown: tokio::sync::oneshot::Sender<()>,
    drain_timeout: Duration,
) -> DrainResult<Output>
where
    Server: Future<Output = Output>,
    Shutdown: Future<Output = ()>,
{
    tokio::pin!(server);
    tokio::pin!(shutdown);

    tokio::select! {
        output = &mut server => DrainResult::Finished(output),
        _ = &mut shutdown => {
            tracing::info!(
                drain_timeout_seconds = drain_timeout.as_secs(),
                "shutdown signal received; draining active requests"
            );
            let _ = graceful_shutdown.send(());
            match tokio::time::timeout(drain_timeout, &mut server).await {
                Ok(output) => DrainResult::Finished(output),
                Err(_) => {
                    tracing::warn!(
                        drain_timeout_seconds = drain_timeout.as_secs(),
                        "graceful shutdown drain timed out; closing remaining connections"
                    );
                    DrainResult::TimedOut
                }
            }
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    tokio::select! {
        _ = ctrl_c_signal() => {}
        _ = terminate_signal() => {}
    }

    #[cfg(not(unix))]
    ctrl_c_signal().await;
}

async fn ctrl_c_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for Ctrl-C");
        future::pending::<()>().await;
    }
}

#[cfg(unix)]
async fn terminate_signal() {
    let mut signal = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        Ok(signal) => signal,
        Err(error) => {
            tracing::error!(%error, "failed to listen for SIGTERM");
            future::pending::<()>().await;
            return;
        }
    };
    if signal.recv().await.is_none() {
        tracing::error!("SIGTERM signal stream closed unexpectedly");
        future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tokio::time::Instant;

    use super::*;

    #[test]
    fn ready_file_is_private_atomic_json_and_removed_with_its_owner() {
        let root = std::env::temp_dir().join(format!(
            "clumsies-server-ready-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("server-ready.json");
        let mut ready = ServerReadyFile::new(Some(path.clone())).unwrap();
        let origin = crate::config::PublicOrigin::parse("http://127.0.0.1:49152").unwrap();

        ready
            .publish("127.0.0.1:49152".parse().unwrap(), &origin)
            .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["listen_addr"], "127.0.0.1:49152");
        assert_eq!(value["public_origin"], "http://127.0.0.1:49152");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));

        drop(ready);
        assert!(!path.exists());
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn ready_file_refuses_an_existing_target() {
        let root = temporary_ready_test_root("existing");
        let path = root.join("server-ready.json");
        fs::write(&path, b"do not replace\n").unwrap();

        let error = ServerReadyFile::new(Some(path.clone())).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"do not replace\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ready_file_publish_refuses_a_target_created_after_validation() {
        let root = temporary_ready_test_root("publish-race");
        let path = root.join("server-ready.json");
        let mut ready = ServerReadyFile::new(Some(path.clone())).unwrap();
        let origin = crate::config::PublicOrigin::parse("http://127.0.0.1:49152").unwrap();
        fs::write(&path, b"do not replace after validation\n").unwrap();

        let error = ready
            .publish("127.0.0.1:49152".parse().unwrap(), &origin)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"do not replace after validation\n"
        );
        drop(ready);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"do not replace after validation\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ready_file_publish_does_not_delete_an_unowned_temporary_file() {
        let root = temporary_ready_test_root("temporary-race");
        let path = root.join("server-ready.json");
        let mut ready = ServerReadyFile::new(Some(path.clone())).unwrap();
        let temporary = ready_temporary_path(&path, &ready.owner_token);
        let origin = crate::config::PublicOrigin::parse("http://127.0.0.1:49152").unwrap();
        fs::write(&temporary, b"unowned temporary\n").unwrap();

        let error = ready
            .publish("127.0.0.1:49152".parse().unwrap(), &origin)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&temporary).unwrap(), b"unowned temporary\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ready_file_drop_preserves_a_replacement_with_a_different_owner() {
        let root = temporary_ready_test_root("replacement");
        let path = root.join("server-ready.json");
        let mut ready = ServerReadyFile::new(Some(path.clone())).unwrap();
        let origin = crate::config::PublicOrigin::parse("http://127.0.0.1:49152").unwrap();
        ready
            .publish("127.0.0.1:49152".parse().unwrap(), &origin)
            .unwrap();
        fs::remove_file(&path).unwrap();
        let replacement = serde_json::to_vec(&serde_json::json!({
            "owner_token": "replacement-owner"
        }))
        .unwrap();
        fs::write(&path, &replacement).unwrap();

        drop(ready);

        assert_eq!(fs::read(&path).unwrap(), replacement);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ready_file_drop_preserves_a_different_file_with_the_same_owner_token() {
        let root = temporary_ready_test_root("replacement-identity");
        let path = root.join("server-ready.json");
        let mut ready = ServerReadyFile::new(Some(path.clone())).unwrap();
        let origin = crate::config::PublicOrigin::parse("http://127.0.0.1:49152").unwrap();
        ready
            .publish("127.0.0.1:49152".parse().unwrap(), &origin)
            .unwrap();
        let replacement = serde_json::to_vec(&serde_json::json!({
            "owner_token": ready.owner_token.as_str()
        }))
        .unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, &replacement).unwrap();

        drop(ready);

        assert_eq!(fs::read(&path).unwrap(), replacement);
        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_ready_test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "clumsies-server-ready-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_allows_requests_to_drain_within_deadline() {
        let (graceful_shutdown, graceful_shutdown_received) = tokio::sync::oneshot::channel();
        let server = async move {
            let _ = graceful_shutdown_received.await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            "clean"
        };
        let started = Instant::now();

        let result = run_with_shutdown_limit(
            server,
            future::ready(()),
            graceful_shutdown,
            SHUTDOWN_DRAIN_TIMEOUT,
        )
        .await;

        assert_eq!(result, DrainResult::Finished("clean"));
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_waiting_at_drain_deadline() {
        let (graceful_shutdown, graceful_shutdown_received) = tokio::sync::oneshot::channel();
        let server = async move {
            let _ = graceful_shutdown_received.await;
            future::pending::<()>().await;
        };
        let started = Instant::now();

        let result = run_with_shutdown_limit(
            server,
            future::ready(()),
            graceful_shutdown,
            SHUTDOWN_DRAIN_TIMEOUT,
        )
        .await;

        assert_eq!(result, DrainResult::TimedOut);
        assert_eq!(started.elapsed(), SHUTDOWN_DRAIN_TIMEOUT);
    }
}
