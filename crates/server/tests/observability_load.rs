//! Explicit local HTTP fixture for dev/load-http.py; excluded from normal tests.

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use url::Url;

mod common;

/// Serve the production application only against the wrapper's disposable database.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Run through python3 dev/load-http.py; creates and cleans up its own local fixture"]
async fn serve_load_fixture() {
    let settings: Value = serde_json::from_str(
        &std::env::var("CLUMSIES_LOAD_SETTINGS").expect("use dev/load-http.py"),
    )
    .unwrap();
    let database_url = std::env::var("CLUMSIES_LOAD_DATABASE_URL").expect("wrapper database");
    let database = Url::parse(&database_url).unwrap();
    assert_eq!(database.scheme(), "postgres");
    assert_eq!(database.host_str(), Some("127.0.0.1"));
    assert_eq!(database.username(), "load_fixture");
    assert_eq!(database.path(), "/load_fixture");
    let container = settings["container"].as_str().unwrap();
    let run_id = settings["run_id"].as_str().unwrap();
    assert!(container.len() == 64 && container.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(run_id.len() == 32 && run_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let owner = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{index .Config.Labels \"clumsies.load.fixture\"}}",
            container,
        ])
        .output()
        .unwrap();
    assert!(
        owner.status.success(),
        "fixture ownership inspection failed"
    );
    assert_eq!(String::from_utf8(owner.stdout).unwrap().trim(), run_id);
    let binding = Command::new("docker")
        .args(["port", container, "5432/tcp"])
        .output()
        .unwrap();
    assert!(binding.status.success(), "fixture port inspection failed");
    assert_eq!(
        String::from_utf8(binding.stdout).unwrap().trim(),
        format!("127.0.0.1:{}", database.port().unwrap())
    );

    let pool = server::infra::database::connect(
        &server::infra::database::DatabaseConfig::from_url(database_url),
    )
    .await
    .unwrap();
    server::infra::database::run_migrations(&pool)
        .await
        .unwrap();
    let installation = common::initialize_installation(
        pool.clone(),
        "Local HTTP Load",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Load 000",
    )
    .await;
    let principal = common::owner_principal(&pool).await;
    let description = "S".repeat(256);
    sqlx::query("UPDATE projects SET description = $1 WHERE project_id = $2")
        .bind(&description)
        .bind(&installation.project_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut projects = vec![installation.project_id];
    for index in 1..64 {
        projects.push(
            server::app::project::create_project(
                &pool,
                &principal,
                &format!("Load {index:03}"),
                &description,
            )
            .await
            .unwrap(),
        );
    }
    let (_, token) = common::authenticated_router(pool.clone()).await;
    // The load uses a real Bearer header; the helper's auto-auth layer is unused.
    let app = common::setup_router(pool.clone(), "owner@example.com", "oidc-subject-owner");
    let postgres_version: String = sqlx::query_scalar("SELECT version()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = oneshot::channel();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                stopped.await.expect("fixture shutdown sender disappeared");
            })
            .await
    });
    let fixture = json!({
        "address": format!("http://{address}"),
        "access_token": token.access_token,
        "projects": projects,
        "project_count": 64,
        "description_bytes": 256,
        "database_max_connections": 10,
        "server_workers": 4,
        "server_version": env!("CARGO_PKG_VERSION"),
        "postgres_version": postgres_version,
        "run_id": run_id,
        "settings": settings,
    });
    let driver = async {
        let mut child = Command::new(settings["python"].as_str().unwrap())
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/load-http.py"))
            .arg("--driver")
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .expect("piped fixture input")
            .write_all(serde_json::to_string(&fixture).unwrap().as_bytes())?;
        loop {
            if let Some(status) = child.try_wait()? {
                break Ok::<_, std::io::Error>(status);
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                signal = tokio::signal::ctrl_c() => {
                    child.kill()?;
                    child.wait()?;
                    signal?;
                    break Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "load interrupted"));
                }
            }
        }
    }
    .await;
    let signalled = shutdown.send(()).is_ok();
    let serving = tokio::time::timeout(Duration::from_secs(10), &mut server).await;
    let server_clean = matches!(serving, Ok(Ok(Ok(()))));
    if !server_clean {
        server.abort();
    }
    let database_clean = tokio::time::timeout(Duration::from_secs(10), pool.close())
        .await
        .is_ok();
    assert!(
        signalled && server_clean,
        "HTTP fixture did not shut down cleanly"
    );
    assert!(
        database_clean,
        "fixture database pool did not close cleanly"
    );
    assert!(
        driver.expect("load driver failed").success(),
        "load recorded unsuccessful operations"
    );
    // The Python wrapper removes its exact owned container even if this test fails.
}
