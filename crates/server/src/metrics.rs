//! Prometheus exposition for request throughput, latency, and pool state.
//!
//! The registry records only route templates and status classes, so label
//! cardinality stays bounded no matter how many resources an organization
//! creates. Derived series such as request rate or latency percentiles belong
//! to PromQL and are deliberately not stored here.

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use crate::state::AppState;

/// Upper bounds of the latency histogram in seconds; the last bucket is +Inf.
const LATENCY_BUCKETS: [f64; 9] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0];

/// Index of the +Inf bucket, which collects requests above every finite bound.
const OVERFLOW_BUCKET: usize = LATENCY_BUCKETS.len();

/// Names of the recorded status classes, covering 1xx through 5xx.
const STATUS_CLASS_NAMES: [&str; 5] = ["1xx", "2xx", "3xx", "4xx", "5xx"];

/// Process-wide request metrics shared by the middleware and the scrape route.
static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::default);

/// Request counters and latency samples for one route template.
#[derive(Default)]
struct RouteMetrics {
    /// Completed requests per status class.
    requests: [AtomicU64; STATUS_CLASS_NAMES.len()],
    /// Completed requests per latency bucket, accumulated while rendering.
    buckets: [AtomicU64; OVERFLOW_BUCKET + 1],
    /// Sum of observed request durations in microseconds.
    duration_micros: AtomicU64,
    /// Requests currently executing for this route.
    in_flight: AtomicI64,
}

impl RouteMetrics {
    /// Record one completed request.
    fn observe(&self, status: StatusCode, elapsed: Duration) {
        if let Some(class) = status_class(status) {
            self.requests[class].fetch_add(1, Ordering::Relaxed);
        }
        let seconds = elapsed.as_secs_f64();
        let bucket = LATENCY_BUCKETS
            .iter()
            .position(|bound| seconds <= *bound)
            .unwrap_or(OVERFLOW_BUCKET);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.duration_micros.fetch_add(micros, Ordering::Relaxed);
    }
}

/// Request metrics for every route the process has served.
#[derive(Default)]
struct Registry {
    /// Per-route metrics, keyed by the registered route template.
    routes: RwLock<HashMap<String, Arc<RouteMetrics>>>,
    /// Requests currently executing across all routes.
    in_flight: AtomicI64,
}

/// Resolve the metrics slot for a route, creating it on first use.
fn route_metrics(route: &str) -> Arc<RouteMetrics> {
    if let Some(existing) = REGISTRY
        .routes
        .read()
        .expect("metrics lock is not poisoned")
        .get(route)
    {
        return Arc::clone(existing);
    }
    let mut routes = REGISTRY
        .routes
        .write()
        .expect("metrics lock is not poisoned");
    Arc::clone(routes.entry(route.to_owned()).or_default())
}

/// Map an HTTP status to its recorded class index.
fn status_class(status: StatusCode) -> Option<usize> {
    let code = status.as_u16();
    (100..600)
        .contains(&code)
        .then_some(usize::from(code / 100 - 1))
}

/// Decrement the in-flight gauges however the request future ends.
struct InFlightGuard {
    /// Route slot whose gauge to decrement.
    route: Arc<RouteMetrics>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.route.in_flight.fetch_sub(1, Ordering::Relaxed);
        REGISTRY.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Record one request without changing its response.
pub(crate) async fn record_request(request: Request, next: Next) -> Response<Body> {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let metrics = route_metrics(&route);
    metrics.in_flight.fetch_add(1, Ordering::Relaxed);
    REGISTRY.in_flight.fetch_add(1, Ordering::Relaxed);
    let _guard = InFlightGuard {
        route: Arc::clone(&metrics),
    };
    let started = Instant::now();

    let response = next.run(request).await;

    metrics.observe(response.status(), started.elapsed());
    response
}

/// Render the current metrics in the Prometheus text exposition format.
pub(crate) async fn render(State(state): State<AppState>) -> Response {
    let routes = snapshot();
    let mut body = String::with_capacity(4_096);

    render_requests(&mut body, &routes);
    render_durations(&mut body, &routes);
    render_in_flight(&mut body);
    render_pool(&mut body, &state);

    body.push_str("# HELP clumsies_build_info Build information.\n");
    body.push_str("# TYPE clumsies_build_info gauge\n");
    let version = escape(state.version);
    let _ = writeln!(body, "clumsies_build_info{{version=\"{version}\"}} 1");

    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

/// Render per-route request counters.
fn render_requests(body: &mut String, routes: &[(String, Arc<RouteMetrics>)]) {
    body.push_str("# HELP clumsies_http_requests_total Requests handled per route and status class.\n");
    body.push_str("# TYPE clumsies_http_requests_total counter\n");
    for (route, metrics) in routes {
        let route = escape(route);
        for (index, class) in STATUS_CLASS_NAMES.iter().enumerate() {
            let count = metrics.requests[index].load(Ordering::Relaxed);
            if count > 0 {
                let _ = writeln!(
                    body,
                    "clumsies_http_requests_total{{route=\"{route}\",status=\"{class}\"}} {count}",
                );
            }
        }
    }
}

/// Render the per-route latency histogram with cumulative buckets.
fn render_durations(body: &mut String, routes: &[(String, Arc<RouteMetrics>)]) {
    body.push_str("# HELP clumsies_http_request_duration_seconds Request duration per route.\n");
    body.push_str("# TYPE clumsies_http_request_duration_seconds histogram\n");
    for (route, metrics) in routes {
        let route = escape(route);
        let mut cumulative = 0_u64;
        for (index, bound) in LATENCY_BUCKETS.iter().enumerate() {
            cumulative += metrics.buckets[index].load(Ordering::Relaxed);
            let _ = writeln!(
                body,
                "clumsies_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"{bound}\"}} {cumulative}",
            );
        }
        cumulative += metrics.buckets[OVERFLOW_BUCKET].load(Ordering::Relaxed);
        let _ = writeln!(
            body,
            "clumsies_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {cumulative}",
        );
        let seconds = metrics.duration_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let _ = writeln!(
            body,
            "clumsies_http_request_duration_seconds_sum{{route=\"{route}\"}} {seconds}",
        );
        let _ = writeln!(
            body,
            "clumsies_http_request_duration_seconds_count{{route=\"{route}\"}} {cumulative}",
        );
    }
}

/// Render the process-wide in-flight gauge.
fn render_in_flight(body: &mut String) {
    body.push_str("# HELP clumsies_http_requests_in_flight Requests currently executing.\n");
    body.push_str("# TYPE clumsies_http_requests_in_flight gauge\n");
    let in_flight = REGISTRY.in_flight.load(Ordering::Relaxed).max(0);
    let _ = writeln!(body, "clumsies_http_requests_in_flight {in_flight}");
}

/// Render connection-pool gauges sampled at scrape time.
fn render_pool(body: &mut String, state: &AppState) {
    let idle = u32::try_from(state.pool.num_idle()).unwrap_or(u32::MAX);
    let size = state.pool.size();
    body.push_str("# HELP clumsies_db_pool_connections Database pool connections by state.\n");
    body.push_str("# TYPE clumsies_db_pool_connections gauge\n");
    let _ = writeln!(body, "clumsies_db_pool_connections{{state=\"idle\"}} {idle}");
    let used = size.saturating_sub(idle);
    let _ = writeln!(body, "clumsies_db_pool_connections{{state=\"used\"}} {used}");
    body.push_str("# HELP clumsies_db_pool_size Configured maximum pool connections.\n");
    body.push_str("# TYPE clumsies_db_pool_size gauge\n");
    let _ = writeln!(body, "clumsies_db_pool_size {size}");
}

/// Copy the current route metrics so rendering never holds the lock.
fn snapshot() -> Vec<(String, Arc<RouteMetrics>)> {
    REGISTRY
        .routes
        .read()
        .expect("metrics lock is not poisoned")
        .iter()
        .map(|(route, metrics)| (route.clone(), Arc::clone(metrics)))
        .collect()
}

/// Escape a Prometheus label value.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '"' => escaped.push_str("\\\""),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use tower::ServiceExt;

    #[test]
    fn status_classes_cover_informational_through_server_errors() {
        assert_eq!(status_class(StatusCode::CONTINUE), Some(0));
        assert_eq!(status_class(StatusCode::OK), Some(1));
        assert_eq!(status_class(StatusCode::SEE_OTHER), Some(2));
        assert_eq!(status_class(StatusCode::NOT_FOUND), Some(3));
        assert_eq!(status_class(StatusCode::SERVICE_UNAVAILABLE), Some(4));
        assert_eq!(status_class(StatusCode::from_u16(999).unwrap()), None);
    }

    #[test]
    fn durations_render_cumulative_buckets() {
        let metrics = Arc::new(RouteMetrics::default());
        metrics.observe(StatusCode::OK, Duration::from_millis(3));
        metrics.observe(StatusCode::OK, Duration::from_millis(40));
        metrics.observe(StatusCode::INTERNAL_SERVER_ERROR, Duration::from_secs(30));

        let routes = vec![("/items/{id}".to_owned(), Arc::clone(&metrics))];
        let mut body = String::new();
        render_requests(&mut body, &routes);
        render_durations(&mut body, &routes);

        assert!(body.contains(r#"clumsies_http_requests_total{route="/items/{id}",status="2xx"} 2"#));
        assert!(body.contains(r#"clumsies_http_requests_total{route="/items/{id}",status="5xx"} 1"#));
        assert!(body.contains(r#"clumsies_http_request_duration_seconds_bucket{route="/items/{id}",le="0.005"} 1"#));
        assert!(body.contains(r#"clumsies_http_request_duration_seconds_bucket{route="/items/{id}",le="0.05"} 2"#));
        assert!(body.contains(r#"clumsies_http_request_duration_seconds_bucket{route="/items/{id}",le="+Inf"} 3"#));
        assert!(body.contains(r#"clumsies_http_request_duration_seconds_count{route="/items/{id}"} 3"#));
    }

    #[test]
    fn label_values_escape_quotes_backslashes_and_newlines() {
        assert_eq!(escape("a\"b\\c\nd"), r#"a\"b\\c\nd"#);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_records_the_route_template_not_the_request_path() {
        let app = Router::new()
            .route("/metrics-test/{id}", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(record_request));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics-test/abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let mut body = String::new();
        render_requests(&mut body, &snapshot());
        assert!(body.contains(r#"route="/metrics-test/{id}",status="2xx""#));
        assert!(!body.contains("/metrics-test/abc123"));
    }
}

