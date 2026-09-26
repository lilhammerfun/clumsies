mod common;

use std::time::{Duration, Instant};

use clumsiesd::{DaemonConfig, DaemonIpcRequest, DaemonIpcService};
use serde_json::json;
use tokio::io::AsyncReadExt;

// Regression: timeout evidence must survive dispatch and preserve causal information.
// Uses a temporary database, fake credentials and a loopback server only.
#[tokio::test(flavor = "current_thread")]
async fn batch_timeout_preserves_cause_correlation_and_safe_logs() {
    let evidence = tempfile::tempdir().unwrap();
    let evidence = evidence.path();
    let log_path = evidence.join("timeout-daemon.log");
    let log_file = std::fs::File::create(&log_path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_ansi(false)
        .with_writer(log_file)
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);

    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        std::future::pending::<()>().await;
        drop(stream);
    });
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.sync.enabled = false;
    let (state, _) =
        common::initialize_authenticated_daemon(config, "audit-fake-token", None).await;
    let service = DaemonIpcService::new(state);
    let body = json!({
        "drafts": (0..118).map(|i| json!({"draft_id": format!("audit-{i}"), "expected_draft_version": 1})).collect::<Vec<_>>(),
        "title": "SECRET_MEMORY_BODY"
    }).to_string();
    let request = DaemonIpcRequest::new(
        "server_request",
        json!({
            "method": "POST", "path": "/api/v1/reviews?secret=SECRET_QUERY",
            "headers": {"content-type": "application/json"}, "body": body
        }),
    );
    let request_id = request.request_id.clone();
    let log_before = std::fs::read_to_string(&log_path).unwrap().len();
    let started = Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(35), service.dispatch(request))
        .await
        .expect("the daemon HTTP timeout must be bounded");
    let elapsed = started.elapsed();
    server.abort();
    let error = response
        .error
        .as_ref()
        .expect("the hanging server must fail");
    let logs = std::fs::read_to_string(&log_path).unwrap();
    let request_logs = &logs[log_before..];
    std::fs::write(
        evidence.join("timeout-response.json"),
        serde_json::to_vec_pretty(&response).unwrap(),
    )
    .unwrap();
    println!(
        "elapsed_ms={} code={} message={} details={} request_log_bytes={}",
        elapsed.as_millis(),
        error.code,
        error.message,
        error.details,
        request_logs.len()
    );
    assert!(elapsed >= Duration::from_secs(29));
    assert_eq!(error.code, "server_request_failed");
    assert!(error.message.contains("timed out"));
    assert_eq!(error.request_id, request_id);
    assert_eq!(error.details["timeout"], true);
    assert!(!error.details["causes"].as_array().unwrap().is_empty());
    for expected in [
        "http_started",
        "draft_count=118",
        "http_failed",
        "ipc_failed",
        "elapsed_ms=",
        &request_id,
    ] {
        assert!(
            request_logs.contains(expected),
            "missing {expected}: {request_logs}"
        );
    }
    for secret in ["SECRET_MEMORY_BODY", "SECRET_QUERY", "audit-fake-token"] {
        assert!(!request_logs.contains(secret));
        assert!(!error.message.contains(secret));
    }
    let preserved = response.into_payload::<serde_json::Value>().unwrap_err();
    match preserved {
        clumsiesd::DaemonError::Remote(error) => {
            assert_eq!(error.request_id, request_id);
            assert_eq!(error.details["timeout"], true);
        }
        error => panic!("flattened IPC error: {error}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_status_and_truncated_body_preserve_response_correlation() {
    use tokio::io::AsyncWriteExt;
    let root = tempfile::tempdir().unwrap();
    let log_path = root.path().join("http.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(std::fs::File::create(&log_path).unwrap())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (status, length) in [(503, 6), (200, 100)] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let bytes = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..bytes]);
            assert!(request.contains("x-clumsies-request-id: req_response_test"));
            let response = format!(
                "HTTP/1.1 {status} Test\r\nX-Request-ID: req_server_{status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\nSECRET"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let mut config = DaemonConfig::for_root(root.path());
    config.project.server_url = format!("http://{address}");
    config.sync.enabled = false;
    let (state, _) = common::initialize_authenticated_daemon(config, "SECRET_TOKEN", None).await;
    let service = DaemonIpcService::new(state);
    for status in [503, 200] {
        let mut request = DaemonIpcRequest::new(
            "server_request",
            json!({"method":"POST", "path":"/api/v1/reviews", "headers":{}}),
        );
        request.request_id = "req_response_test".to_owned();
        let response = service.dispatch(request).await;
        if status == 503 {
            assert!(response.ok);
            assert_eq!(response.payload["status"], 503);
            assert_eq!(
                response.payload["headers"]["x-request-id"],
                "req_server_503"
            );
        } else {
            let error = response.error.unwrap();
            assert_eq!(error.request_id, "req_response_test");
            assert!(!error.details["causes"].as_array().unwrap().is_empty());
        }
    }
    server.await.unwrap();
    let logs = std::fs::read_to_string(log_path).unwrap();
    for expected in [
        "req_response_test",
        "req_server_503",
        "req_server_200",
        "http_failed",
        "stage=\"body\"",
    ] {
        assert!(logs.contains(expected), "missing {expected}: {logs}");
    }
    assert!(!logs.contains("SECRET"));
}
