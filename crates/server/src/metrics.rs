//! Prometheus request metrics and scrape-time connection-pool measurements.
//!
//! Route templates bound label cardinality. Handler latency excludes response
//! body transfer; rates and percentiles are calculated by Prometheus.

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};
use std::sync::LazyLock;
use std::time::Instant;

use crate::state::AppState;

/// Finite latency bounds in seconds; the library adds the +Inf bucket.
const LATENCY_BUCKETS: [f64; 12] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// Preserve the existing bounded response-code labels and `other` fallback.
const STATUS_CODES: [u16; 27] = [
    100, 200, 201, 202, 204, 301, 302, 303, 304, 307, 308, 400, 401, 403, 404, 405, 409, 410, 412,
    413, 415, 422, 429, 500, 501, 502, 503,
];

/// Metrics shared by the request middleware and scrape endpoint.
static METRICS: LazyLock<Metrics> =
    LazyLock::new(|| Metrics::new().expect("valid metric definitions"));

/// Registered library collectors; no custom aggregation or exposition logic.
struct Metrics {
    /// Registry dedicated to Server metrics.
    registry: Registry,
    /// Completed requests by route template and bounded response code.
    requests: IntCounterVec,
    /// Handler response durations in seconds by route template.
    durations: HistogramVec,
    /// Executing handlers, including those whose futures are later cancelled.
    in_flight: IntGauge,
    /// Idle and used connections sampled when scraped.
    pool_connections: IntGaugeVec,
    /// Current open connections, not the configured limit.
    pool_size: IntGauge,
    /// Configured connection limit.
    pool_max: IntGauge,
    /// Server version exposed as a label with constant value one.
    build_info: IntGaugeVec,
}

impl Metrics {
    /// Register the Server's metric definitions in an independent registry.
    ///
    /// # Errors
    /// Returns an error for invalid or duplicate metric definitions.
    fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "clumsies_http_requests_total",
                "Requests handled per route and response code.",
            ),
            &["route", "status"],
        )?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "clumsies_http_request_duration_seconds",
                "Handler response latency per route; excludes response body transfer.",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["route"],
        )?;
        let in_flight = IntGauge::new(
            "clumsies_http_requests_in_flight",
            "Requests currently executing.",
        )?;
        let pool_connections = IntGaugeVec::new(
            Opts::new(
                "clumsies_db_pool_connections",
                "Database pool connections by state.",
            ),
            &["state"],
        )?;
        let pool_size = IntGauge::new(
            "clumsies_db_pool_size",
            "Current open database pool connections.",
        )?;
        let pool_max = IntGauge::new(
            "clumsies_db_pool_max_connections",
            "Configured maximum database pool connections.",
        )?;
        let build_info = IntGaugeVec::new(
            Opts::new("clumsies_build_info", "Build information."),
            &["version"],
        )?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(durations.clone()))?;
        registry.register(Box::new(in_flight.clone()))?;
        registry.register(Box::new(pool_connections.clone()))?;
        registry.register(Box::new(pool_size.clone()))?;
        registry.register(Box::new(pool_max.clone()))?;
        registry.register(Box::new(build_info.clone()))?;
        Ok(Self {
            registry,
            requests,
            durations,
            in_flight,
            pool_connections,
            pool_size,
            pool_max,
            build_info,
        })
    }

    /// Sample current pool state without reporting the configured limit as usage.
    fn sample_pool(&self, pool: &sqlx::PgPool) {
        let size = pool.size();
        let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX);
        self.pool_connections
            .with_label_values(&["idle"])
            .set(i64::from(idle));
        self.pool_connections
            .with_label_values(&["used"])
            .set(i64::from(size.saturating_sub(idle)));
        self.pool_size.set(i64::from(size));
        self.pool_max
            .set(i64::from(pool.options().get_max_connections()));
    }
}

/// Decrement the gauge on completion, cancellation, or unwinding.
struct InFlightGuard(IntGauge);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.dec();
    }
}

/// Record a handler response without changing its contents or status.
pub(crate) async fn record_request(request: Request, next: Next) -> Response<Body> {
    record(&METRICS, request, next).await
}

/// Measure a handler using route templates rather than resource-specific paths.
async fn record(metrics: &Metrics, request: Request, next: Next) -> Response<Body> {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    metrics.in_flight.inc();
    let _guard = InFlightGuard(metrics.in_flight.clone());
    let started = Instant::now();
    let response = next.run(request).await;
    let code = response.status().as_u16();
    let status = if STATUS_CODES.contains(&code) {
        code.to_string()
    } else {
        "other".to_owned()
    };
    metrics.requests.with_label_values(&[&route, &status]).inc();
    metrics
        .durations
        .with_label_values(&[&route])
        .observe(started.elapsed().as_secs_f64());
    response
}

/// Encode library collectors using the Prometheus text format and matching MIME type.
pub(crate) async fn render(State(state): State<AppState>) -> Response {
    METRICS.sample_pool(&state.pool);
    METRICS
        .build_info
        .with_label_values(&[state.version])
        .set(1);
    let encoder = TextEncoder::new();
    let mut body = Vec::new();
    match encoder.encode(&METRICS.registry.gather(), &mut body) {
        Ok(()) => ([(header::CONTENT_TYPE, encoder.format_type())], body).into_response(),
        Err(error) => {
            tracing::error!(%error, "metrics encoding failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(metrics: Arc<Metrics>) -> Router {
        Router::new()
            .route("/items/{id}", get(|| async { StatusCode::CONFLICT }))
            .route(
                "/unknown",
                get(|| async { StatusCode::from_u16(599).unwrap() }),
            )
            .layer(axum::middleware::from_fn(move |request, next| {
                let metrics = metrics.clone();
                async move { record(&metrics, request, next).await }
            }))
    }

    #[tokio::test]
    async fn middleware_preserves_labels_counts_and_response() {
        let metrics = Arc::new(Metrics::new().unwrap());
        for (path, status) in [
            ("/items/private-id", 409),
            ("/unknown", 599),
            ("/not-found", 404),
        ] {
            let response = app(metrics.clone())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
        }
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/items/{id}", "409"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/unknown", "other"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["unmatched", "404"])
                .get(),
            1
        );
        assert_eq!(metrics.in_flight.get(), 0);
        let text = TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        assert!(!text.contains("private-id"));
    }

    #[test]
    fn exposition_keeps_histogram_contract_and_escapes_labels() {
        let metrics = Metrics::new().unwrap();
        let histogram = metrics.durations.with_label_values(&["/items/{id}"]);
        for value in [0.003, 0.04, 2.0, 30.0, 31.0] {
            histogram.observe(value);
        }
        metrics.build_info.with_label_values(&["a\"b\\c\nd"]).set(1);
        let text = TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        for (bound, count) in [
            ("0.005", 1),
            ("0.05", 2),
            ("2.5", 3),
            ("30", 4),
            ("+Inf", 5),
        ] {
            assert!(text.contains(&format!("clumsies_http_request_duration_seconds_bucket{{route=\"/items/{{id}}\",le=\"{bound}\"}} {count}")), "{text}");
        }
        assert!(
            text.contains("clumsies_http_request_duration_seconds_count{route=\"/items/{id}\"} 5")
        );
        assert!((histogram.get_sample_sum() - 63.043).abs() < 1e-9);
        assert!(text.contains(r#"version="a\"b\\c\nd""#));
    }

    #[tokio::test]
    async fn pool_size_and_configured_limit_are_distinct() {
        // Lazy pool never connects to a real database.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(17)
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let metrics = Metrics::new().unwrap();
        metrics.sample_pool(&pool);
        assert_eq!(metrics.pool_size.get(), 0);
        assert_eq!(metrics.pool_max.get(), 17);
        assert_eq!(
            metrics.pool_connections.with_label_values(&["used"]).get(),
            0
        );
    }

    #[tokio::test]
    async fn cancelled_request_releases_in_flight_without_counting_a_response() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let entered = Arc::new(tokio::sync::Notify::new());
        let app = Router::new()
            .route(
                "/pending",
                get({
                    let entered = entered.clone();
                    move || {
                        let entered = entered.clone();
                        async move {
                            entered.notify_one();
                            std::future::pending::<()>().await;
                        }
                    }
                }),
            )
            .layer(axum::middleware::from_fn({
                let metrics = metrics.clone();
                move |request, next| {
                    let metrics = metrics.clone();
                    async move { record(&metrics, request, next).await }
                }
            }));
        let task = tokio::spawn(
            app.oneshot(
                Request::builder()
                    .uri("/pending")
                    .body(Body::empty())
                    .unwrap(),
            ),
        );
        entered.notified().await;
        assert_eq!(metrics.in_flight.get(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(metrics.in_flight.get(), 0);
        assert_eq!(
            metrics
                .durations
                .with_label_values(&["/pending"])
                .get_sample_count(),
            0
        );
    }
}
