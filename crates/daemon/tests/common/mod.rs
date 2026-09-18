use daemon::{
    CredentialStore, CredentialStoreError, DaemonConfig, DaemonIpcService, DaemonState,
    ServerCredentials,
};
use server::app::auth::OidcIdentity;
use server::app::installation::dto::ReplaceSetupConfigurationRequest;
use server::app::installation::{InitializedInstallation, InstallationService};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tempfile::TempDir;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[allow(dead_code)]
pub struct TestPostgres {
    _permit: OwnedSemaphorePermit,
    _container: ContainerAsync<Postgres>,
    pub database_url: String,
}

fn postgres_slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4)))
}

#[allow(dead_code)]
pub async fn start_postgres() -> TestPostgres {
    let permit = postgres_slots().clone().acquire_owned().await.unwrap();
    let container = Postgres::default().start().await.unwrap();
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    TestPostgres {
        _permit: permit,
        _container: container,
        database_url: format!("postgres://postgres:postgres@{host}:{port}/postgres"),
    }
}

#[allow(dead_code)]
pub async fn initialize_installation(
    pool: sqlx::PgPool,
    project_name: &str,
) -> InitializedInstallation {
    const TEST_SETUP_CODE: &str = "clumsies-test-setup-code-00000001";

    let installation =
        InstallationService::new(pool.clone(), Some(TEST_SETUP_CODE), false).unwrap();
    let credentials = installation.create_session(TEST_SETUP_CODE).await.unwrap();
    installation
        .replace_configuration(
            &credentials.token,
            &credentials.session.csrf_token,
            ReplaceSetupConfigurationRequest {
                org_name: "Acme Memory".to_owned(),
                default_project_name: project_name.to_owned(),
                allowed_email_domains: Vec::new(),
            },
        )
        .await
        .unwrap();
    let session_id = installation
        .authorize_oidc(&credentials.token, &credentials.session.csrf_token)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let initialized = installation
        .initialize_with_oidc(
            &mut tx,
            &session_id,
            &OidcIdentity {
                issuer: "https://identity.example.test".to_owned(),
                subject: "oidc-subject-owner".to_owned(),
                email: "owner@example.com".to_owned(),
                email_verified: true,
                display_name: Some("Owner".to_owned()),
                avatar_url: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    initialized
}

#[derive(Clone, Default)]
pub struct TestCredentialStore {
    credentials: Arc<Mutex<Option<ServerCredentials>>>,
    fail_writes: Arc<AtomicBool>,
}

impl TestCredentialStore {
    pub fn new(credentials: Option<ServerCredentials>) -> Self {
        Self {
            credentials: Arc::new(Mutex::new(credentials)),
            fail_writes: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn credentials(&self) -> Option<ServerCredentials> {
        self.credentials.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::Release);
    }
}

impl CredentialStore for TestCredentialStore {
    fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
        Ok(self.credentials())
    }

    fn replace(&self, credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(CredentialStoreError::new(
                "injected credential write failure",
            ));
        }
        *self.credentials.lock().unwrap() = Some(credentials.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), CredentialStoreError> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(CredentialStoreError::new(
                "injected credential write failure",
            ));
        }
        *self.credentials.lock().unwrap() = None;
        Ok(())
    }
}

pub async fn initialize_daemon(
    config: DaemonConfig,
    credential_store: TestCredentialStore,
) -> DaemonState {
    DaemonState::initialize_with_credential_store(config, Arc::new(credential_store))
        .await
        .unwrap()
}

pub async fn initialize_authenticated_daemon(
    config: DaemonConfig,
    access_token: impl Into<String>,
    refresh_token: Option<String>,
) -> (DaemonState, TestCredentialStore) {
    let credential_store = TestCredentialStore::new(Some(ServerCredentials {
        server_url: config.project.server_url.clone(),
        access_token: access_token.into(),
        refresh_token,
    }));
    let state = initialize_daemon(config, credential_store.clone()).await;
    (state, credential_store)
}

#[allow(dead_code)]
pub async fn test_daemon() -> (TempDir, DaemonState, DaemonIpcService) {
    let root = tempfile::tempdir().unwrap();
    let state = initialize_daemon(
        DaemonConfig::for_root(root.path()),
        TestCredentialStore::default(),
    )
    .await;
    let service = DaemonIpcService::new(state.clone());
    (root, state, service)
}

/// Construct the production server with unconfigured authentication for TCP scenarios.
#[allow(dead_code)]
pub fn server_app(pool: sqlx::PgPool) -> axum::Router {
    let auth = server::app::auth::AuthService::unconfigured(pool.clone());
    let installation = InstallationService::new(pool.clone(), None, true).unwrap();
    server::build_app(pool, auth, installation)
}

/// A TCP scenario owns the server task, its pool, and its PostgreSQL container.
#[allow(dead_code)]
pub struct TestServer {
    pub address: std::net::SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    pool: sqlx::PgPool,
    postgres: Option<TestPostgres>,
}

#[allow(dead_code)]
impl TestServer {
    /// Bind a fresh port and serve the production Router with the scenario's pool.
    pub async fn start(pool: sqlx::PgPool, postgres: TestPostgres) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = server_app(pool.clone());
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .expect("serve test application");
        });
        Self {
            address,
            shutdown: Some(shutdown),
            task,
            pool,
            postgres: Some(postgres),
        }
    }

    /// Drain HTTP tasks before closing the pool and removing the database.
    pub async fn shutdown(mut self) {
        self.shutdown
            .take()
            .unwrap()
            .send(())
            .expect("signal test server shutdown");
        tokio::time::timeout(std::time::Duration::from_secs(10), &mut self.task)
            .await
            .expect("test server shutdown deadline")
            .expect("join test server");
        self.pool.close().await;
        self.postgres
            .take()
            .unwrap()
            ._container
            .rm()
            .await
            .expect("remove test database");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Panic-path fallback; successful scenarios await shutdown explicitly.
        self.task.abort();
    }
}
