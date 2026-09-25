//! Request correlation and HTTP tracing.

use axum::body::Body;
use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderValue, Response};
use axum::middleware::Next;
use axum::{Router, middleware};
use std::io;
use std::time::Duration;
use tower_http::trace::TraceLayer;
use tracing::{Span, info_span};
use tracing_subscriber::EnvFilter;

tokio::task_local! {
    static CURRENT_REQUEST_ID: String;
}

/// Validated correlation identity propagated through request logs and error responses.
#[derive(Clone)]
struct RequestId(String);

/// Initialize structured tracing once using the deployment log filter.
///
/// # Errors
/// Propagates failure to install the process-wide tracing subscriber.
pub(crate) fn init() -> io::Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("server=info"));
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_env_filter(filter)
        .with_target(false)
        .try_init()
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Attach request correlation and tracing to the constructed application.
pub(crate) fn instrument(app: Router) -> Router {
    app.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &axum::http::Request<Body>| {
                let route = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map(MatchedPath::as_str)
                    .unwrap_or("unmatched");
                let request_id = request
                    .extensions()
                    .get::<RequestId>()
                    .map(|request_id| request_id.0.as_str())
                    .unwrap_or("missing");
                let client_request_id = request
                    .headers()
                    .get("x-clumsies-request-id")
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| valid_request_id(value))
                    .unwrap_or("missing");
                info_span!(
                    "http_request",
                    method = %request.method(),
                    route,
                    request_id,
                    client_request_id,
                )
            })
            .on_request(|_request: &Request, span: &Span| {
                tracing::info!(parent: span, "http request received");
            })
            .on_response(
                |response: &Response<Body>, latency: Duration, span: &Span| {
                    tracing::info!(
                        parent: span,
                        status = response.status().as_u16(),
                        duration_ms = latency.as_secs_f64() * 1_000.0,
                        "http request completed"
                    );
                },
            )
            .on_failure(()),
    )
    // Inside the request context so the span, response, and metrics share one
    // matched route and one correlation identity.
    .layer(middleware::from_fn(crate::metrics::record_request))
    .layer(middleware::from_fn(request_context))
}

/// Read the correlation identity of the current request task.
pub(crate) fn current_request_id() -> String {
    CURRENT_REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| new_request_id())
}

/// Bind one request ID to response headers, logs, and the handler's task scope.
async fn request_context(mut request: Request, next: Next) -> Response<Body> {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    CURRENT_REQUEST_ID
        .scope(request_id.clone(), async move {
            let mut response = next.run(request).await;
            if let Ok(value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert("x-request-id", value);
            }
            response
        })
        .await
}

/// Accept only bounded, safe characters from an upstream correlation header.
fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Generate a fresh correlation identity when no valid upstream value is supplied.
fn new_request_id() -> String {
    format!("req_{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test(flavor = "current_thread")]
    async fn ingress_logs_link_client_and_server_ids_without_query_or_body() {
        // Tracing caches callsite interest across threads. Run alone so another
        // router test without a subscriber cannot register these calls as disabled.
        const CHILD_ENV: &str = "CLUMSIES_TELEMETRY_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "telemetry::tests::ingress_logs_link_client_and_server_ids_without_query_or_body",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed;"),
                "isolated telemetry test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }
        let path =
            std::env::temp_dir().join(format!("clumsies-telemetry-{}.log", uuid::Uuid::new_v4()));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(std::fs::File::create(&path).unwrap())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let app = instrument(Router::new().route("/probe", axum::routing::post(|| async { "ok" })));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/probe?token=SECRET_QUERY")
                    .header("x-request-id", "req_proxy")
                    .header("x-clumsies-request-id", "req_client")
                    .body(Body::from("SECRET_BODY"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-request-id"], "req_proxy");
        let logs = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        for expected in [
            "http request received",
            "http request completed",
            "req_client",
            "req_proxy",
        ] {
            assert!(logs.contains(expected), "missing {expected}: {logs}");
        }
        assert!(!logs.contains("SECRET"));
    }
}
