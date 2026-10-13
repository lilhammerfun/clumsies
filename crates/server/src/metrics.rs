//! Prometheus request lifecycle, scoped pool acquisitions, and scrape-time pool measurements.
//!
//! Route templates bound label cardinality. Handler latency excludes response
//! body transfer; rates and percentiles are calculated by Prometheus.

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode, header};
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

/// Metrics shared by the request middleware and scrape endpoint.
static METRICS: LazyLock<Metrics> =
    LazyLock::new(|| Metrics::new().expect("valid metric definitions"));

/// Registered library collectors; no custom aggregation or exposition logic.
struct Metrics {
    /// Registry dedicated to Server metrics.
    registry: Registry,
    /// Completed responses by route template, bounded method, and response code.
    requests: IntCounterVec,
    /// Handler response durations in seconds by route, method, and status class.
    durations: HistogramVec,
    /// Executing handlers across all routes, including health and metrics.
    in_flight: IntGauge,
    /// Requests entering non-health, non-metrics handlers.
    business_started: IntCounterVec,
    /// Executing handlers in the same scope as the business start counter.
    business_in_flight: IntGaugeVec,
    /// Business handler futures ending with a response or without one.
    business_terminated: IntCounterVec,
    /// Idle and used connections sampled when scraped.
    pool_connections: IntGaugeVec,
    /// Current open connections, not the configured limit.
    pool_size: IntGauge,
    /// Configured connection limit.
    pool_max: IntGauge,
    /// Completed acquisition attempts on the explicitly instrumented operations.
    pool_acquires: IntCounterVec,
    /// Acquisition durations including queueing, connection creation, and validation.
    pool_acquire_durations: HistogramVec,
    /// In-progress acquisitions, including connection creation and validation.
    pool_acquires_in_flight: IntGaugeVec,
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
                "Completed handler responses per route, method, and response code.",
            ),
            &["route", "method", "status"],
        )?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "clumsies_http_request_duration_seconds",
                "Handler response latency per route; excludes response body transfer.",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["route", "method", "status_class"],
        )?;
        let in_flight = IntGauge::new(
            "clumsies_http_requests_in_flight",
            "Requests currently executing across all routes, including health and metrics.",
        )?;
        let business_started = IntCounterVec::new(
            Opts::new(
                "clumsies_http_business_requests_started_total",
                "Requests entering handlers, excluding health and metrics routes.",
            ),
            &["route", "method"],
        )?;
        let business_in_flight = IntGaugeVec::new(
            Opts::new(
                "clumsies_http_business_requests_in_flight",
                "Executing handlers, excluding health and metrics routes.",
            ),
            &["route", "method"],
        )?;
        let business_terminated = IntCounterVec::new(
            Opts::new(
                "clumsies_http_business_requests_terminated_total",
                "Handler futures ending with a response or dropped without one; excludes health and metrics.",
            ),
            &["route", "method", "outcome"],
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
        let pool_acquires = IntCounterVec::new(
            Opts::new(
                "clumsies_db_pool_acquires_total",
                "Completed pool acquisitions for review_create and review_merge transactions only.",
            ),
            &["operation", "outcome"],
        )?;
        let pool_acquire_durations = HistogramVec::new(
            HistogramOpts::new(
                "clumsies_db_pool_acquire_duration_seconds",
                "Pool acquisition latency for review_create and review_merge only; excludes BEGIN and transaction execution.",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["operation", "outcome"],
        )?;
        let pool_acquires_in_flight = IntGaugeVec::new(
            Opts::new(
                "clumsies_db_pool_acquires_in_flight",
                "In-progress pool acquisitions for review_create and review_merge only; not pure queue waiters.",
            ),
            &["operation"],
        )?;
        let build_info = IntGaugeVec::new(
            Opts::new("clumsies_build_info", "Build information."),
            &["version"],
        )?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(durations.clone()))?;
        registry.register(Box::new(in_flight.clone()))?;
        registry.register(Box::new(business_started.clone()))?;
        registry.register(Box::new(business_in_flight.clone()))?;
        registry.register(Box::new(business_terminated.clone()))?;
        registry.register(Box::new(pool_connections.clone()))?;
        registry.register(Box::new(pool_size.clone()))?;
        registry.register(Box::new(pool_max.clone()))?;
        registry.register(Box::new(pool_acquires.clone()))?;
        registry.register(Box::new(pool_acquire_durations.clone()))?;
        registry.register(Box::new(pool_acquires_in_flight.clone()))?;
        registry.register(Box::new(build_info.clone()))?;
        Ok(Self {
            registry,
            requests,
            durations,
            in_flight,
            business_started,
            business_in_flight,
            business_terminated,
            pool_connections,
            pool_size,
            pool_max,
            pool_acquires,
            pool_acquire_durations,
            pool_acquires_in_flight,
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

/// Balance request lifecycle metrics when a handler returns or its future is dropped.
struct RequestGuard<'a> {
    /// Collectors belonging to this request's registry.
    metrics: &'a Metrics,
    /// Matched route template, never the resource-specific request path.
    route: &'a str,
    /// Bounded standard method or `other`.
    method: &'a str,
    /// Whether this request is outside the health and metrics endpoints.
    business: bool,
    /// Whether the handler produced a response before this guard was dropped.
    completed: bool,
}

impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.metrics.in_flight.dec();
        if self.business {
            self.metrics
                .business_in_flight
                .with_label_values(&[self.route, self.method])
                .dec();
            let outcome = if self.completed {
                "response"
            } else {
                // Dropping a future cannot distinguish caller cancellation, task abort, or unwinding.
                "cancelled_or_aborted"
            };
            self.metrics
                .business_terminated
                .with_label_values(&[self.route, self.method, outcome])
                .inc();
        }
    }
}

/// Bound arbitrary extension methods without changing the request sent to its handler.
fn method_label(method: &Method) -> &str {
    match method.as_str() {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "CONNECT" | "TRACE" => {
            method.as_str()
        }
        _ => "other",
    }
}

/// Classify the actual response even when its exact code uses the `other` label.
fn status_class(code: u16) -> &'static str {
    match code {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
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
    let method = method_label(request.method()).to_owned();
    let business = !matches!(route.as_str(), "/metrics" | "/api/v1/admin/health");
    metrics.in_flight.inc();
    if business {
        metrics
            .business_started
            .with_label_values(&[&route, &method])
            .inc();
        metrics
            .business_in_flight
            .with_label_values(&[&route, &method])
            .inc();
    }
    let mut guard = RequestGuard {
        metrics,
        route: &route,
        method: &method,
        business,
        completed: false,
    };
    let started = Instant::now();
    let response = next.run(request).await;
    guard.completed = true;
    let code = response.status().as_u16();
    let status = if (100..=599).contains(&code) {
        code.to_string()
    } else {
        "other".to_owned()
    };
    metrics
        .requests
        .with_label_values(&[&route, &method, &status])
        .inc();
    metrics
        .durations
        .with_label_values(&[route.as_str(), method.as_str(), status_class(code)])
        .observe(started.elapsed().as_secs_f64());
    response
}

/// Record the outcome and duration of a real pooled connection acquisition.
struct PoolAcquireGuard<'a> {
    /// Collectors belonging to this acquisition's registry.
    metrics: &'a Metrics,
    /// Fixed operation name supplied by the instrumented call site.
    operation: &'static str,
    /// Monotonic acquisition start, before waiting for the pool.
    started: Instant,
    /// Terminal result, or cancellation if the acquisition future is dropped.
    outcome: &'static str,
}

impl Drop for PoolAcquireGuard<'_> {
    fn drop(&mut self) {
        self.metrics
            .pool_acquires_in_flight
            .with_label_values(&[self.operation])
            .dec();
        self.metrics
            .pool_acquires
            .with_label_values(&[self.operation, self.outcome])
            .inc();
        self.metrics
            .pool_acquire_durations
            .with_label_values(&[self.operation, self.outcome])
            .observe(self.started.elapsed().as_secs_f64());
    }
}

/// Acquire a connection for an explicitly instrumented review transaction.
///
/// Timing includes pool queueing, validation, and connection creation, but excludes BEGIN and
/// transaction execution. Other Server acquisitions are outside this metric's coverage.
///
/// # Errors
/// Preserves the pool's acquisition timeout, closed-pool, and connection errors unchanged.
pub(crate) async fn acquire_review_connection(
    pool: &sqlx::PgPool,
    operation: &'static str,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    acquire_connection(&METRICS, pool, operation).await
}

/// Observe a real acquisition without changing its timeout or cancellation behavior.
///
/// # Errors
/// Returns the underlying pool error; dropping this future drops its acquisition future.
async fn acquire_connection(
    metrics: &Metrics,
    pool: &sqlx::PgPool,
    operation: &'static str,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    metrics
        .pool_acquires_in_flight
        .with_label_values(&[operation])
        .inc();
    let mut guard = PoolAcquireGuard {
        metrics,
        operation,
        started: Instant::now(),
        outcome: "cancelled_or_aborted",
    };
    let result = pool.acquire().await;
    guard.outcome = match &result {
        Ok(_) => "acquired",
        Err(sqlx::Error::PoolTimedOut) => "timeout",
        Err(_) => "error",
    };
    result
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
    use axum::{
        Router,
        routing::{any, get},
    };
    use sqlx::postgres::PgPoolOptions;
    use std::sync::Arc;
    use std::time::Duration;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    use tower::ServiceExt;

    fn app(metrics: Arc<Metrics>) -> Router {
        Router::new()
            .route(
                "/items/{id}",
                get(|| async { StatusCode::CONFLICT }).post(|| async { StatusCode::CREATED }),
            )
            .route(
                "/unknown",
                get(|| async { StatusCode::from_u16(699).unwrap() }),
            )
            .route("/timeout", get(|| async { StatusCode::GATEWAY_TIMEOUT }))
            .route(
                "/server-error",
                get(|| async { StatusCode::from_u16(599).unwrap() }),
            )
            .route(
                "/extension",
                any(|method: Method| async move { method.to_string() }),
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
            ("/unknown", 699),
            ("/timeout", 504),
            ("/server-error", 599),
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
                .with_label_values(&["/items/{id}", "GET", "409"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/unknown", "GET", "other"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["unmatched", "GET", "404"])
                .get(),
            1
        );
        assert_eq!(metrics.in_flight.get(), 0);
        for (route, status) in [("/timeout", "504"), ("/server-error", "599")] {
            assert_eq!(
                metrics
                    .requests
                    .with_label_values(&[route, "GET", status])
                    .get(),
                1
            );
        }
        for (route, class) in [
            ("/items/{id}", "4xx"),
            ("/unknown", "other"),
            ("/timeout", "5xx"),
            ("/server-error", "5xx"),
            ("unmatched", "4xx"),
        ] {
            assert_eq!(
                metrics
                    .business_started
                    .with_label_values(&[route, "GET"])
                    .get(),
                1
            );
            assert_eq!(
                metrics
                    .business_in_flight
                    .with_label_values(&[route, "GET"])
                    .get(),
                0
            );
            assert_eq!(
                metrics
                    .business_terminated
                    .with_label_values(&[route, "GET", "response"])
                    .get(),
                1
            );
            assert_eq!(
                metrics
                    .durations
                    .with_label_values(&[route, "GET", class])
                    .get_sample_count(),
                1
            );
        }
        let text = TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        assert!(!text.contains("private-id"));
    }

    #[tokio::test]
    async fn middleware_separates_methods_and_bounds_extension_methods() {
        let metrics = Arc::new(Metrics::new().unwrap());
        for (method, path, status) in [
            ("GET", "/items/one", 409),
            ("POST", "/items/one", 201),
            ("PRIVATE_METHOD", "/extension", 200),
        ] {
            let response = app(metrics.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status);
            if method == "PRIVATE_METHOD" {
                let body = axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap();
                assert_eq!(&body[..], b"PRIVATE_METHOD");
            }
        }
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/items/{id}", "POST", "201"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .durations
                .with_label_values(&["/items/{id}", "POST", "2xx"])
                .get_sample_count(),
            1
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/extension", "other", "200"])
                .get(),
            1
        );
        let text = TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        assert!(!text.contains("PRIVATE_METHOD"));
        assert!(!text.contains("/items/one"));
    }

    #[tokio::test]
    async fn health_and_metrics_are_excluded_from_every_business_lifecycle_metric() {
        let metrics = Arc::new(Metrics::new().unwrap());
        let app = Router::new()
            .route("/metrics", get(|| async { StatusCode::OK }))
            .route("/api/v1/admin/health", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn({
                let metrics = metrics.clone();
                move |request, next| {
                    let metrics = metrics.clone();
                    async move { record(&metrics, request, next).await }
                }
            }));
        for path in ["/metrics", "/api/v1/admin/health"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                metrics
                    .requests
                    .with_label_values(&[path, "GET", "200"])
                    .get(),
                1
            );
        }
        assert_eq!(metrics.in_flight.get(), 0);
        let text = TextEncoder::new()
            .encode_to_string(&metrics.registry.gather())
            .unwrap();
        assert!(!text.contains("clumsies_http_business_requests"), "{text}");
    }

    #[test]
    fn exposition_keeps_histogram_contract_and_escapes_labels() {
        let metrics = Metrics::new().unwrap();
        let histogram = metrics
            .durations
            .with_label_values(&["/items/{id}", "GET", "2xx"]);
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
            assert!(text.contains(&format!("clumsies_http_request_duration_seconds_bucket{{method=\"GET\",route=\"/items/{{id}}\",status_class=\"2xx\",le=\"{bound}\"}} {count}")), "{text}");
        }
        assert!(
            text.contains("clumsies_http_request_duration_seconds_count{method=\"GET\",route=\"/items/{id}\",status_class=\"2xx\"} 5")
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
    async fn real_pool_acquisition_tracks_results_without_timing_connection_use() {
        let container = Postgres::default().start().await.unwrap();
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(5432).await.unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(150))
            .connect(&format!(
                "postgres://postgres:postgres@{host}:{port}/postgres"
            ))
            .await
            .unwrap();
        let metrics = Arc::new(Metrics::new().unwrap());
        let mut connection = acquire_connection(&metrics, &pool, "review_create")
            .await
            .unwrap();
        let histogram = metrics
            .pool_acquire_durations
            .with_label_values(&["review_create", "acquired"]);
        let acquired_seconds = histogram.get_sample_sum();
        assert_eq!(histogram.get_sample_count(), 1);
        assert_eq!(
            metrics
                .pool_acquires_in_flight
                .with_label_values(&["review_create"])
                .get(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT 42")
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
            42
        );
        sqlx::query("SELECT pg_sleep(0.02)")
            .execute(&mut *connection)
            .await
            .unwrap();
        assert_eq!(histogram.get_sample_sum(), acquired_seconds);

        let result = acquire_connection(&metrics, &pool, "review_create").await;
        assert!(matches!(result, Err(sqlx::Error::PoolTimedOut)));
        assert_eq!(
            metrics
                .pool_acquires
                .with_label_values(&["review_create", "timeout"])
                .get(),
            1
        );
        assert!(
            metrics
                .pool_acquire_durations
                .with_label_values(&["review_create", "timeout"])
                .get_sample_sum()
                >= 0.1
        );

        let task = tokio::spawn({
            let metrics = metrics.clone();
            let pool = pool.clone();
            async move { acquire_connection(&metrics, &pool, "review_merge").await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while metrics
                .pool_acquires_in_flight
                .with_label_values(&["review_merge"])
                .get()
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            metrics
                .pool_acquires_in_flight
                .with_label_values(&["review_merge"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .pool_acquires
                .with_label_values(&["review_merge", "cancelled_or_aborted"])
                .get(),
            1
        );

        drop(connection);
        pool.close().await;
        assert!(matches!(
            acquire_connection(&metrics, &pool, "review_create").await,
            Err(sqlx::Error::PoolClosed)
        ));
        assert_eq!(
            metrics
                .pool_acquires
                .with_label_values(&["review_create", "error"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .pool_acquires
                .with_label_values(&["review_create", "acquired"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .pool_acquires_in_flight
                .with_label_values(&["review_create"])
                .get(),
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
        assert_eq!(
            metrics
                .business_started
                .with_label_values(&["/pending", "GET"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .business_in_flight
                .with_label_values(&["/pending", "GET"])
                .get(),
            1
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(metrics.in_flight.get(), 0);
        assert_eq!(
            metrics
                .business_in_flight
                .with_label_values(&["/pending", "GET"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .business_terminated
                .with_label_values(&["/pending", "GET", "cancelled_or_aborted"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .business_terminated
                .with_label_values(&["/pending", "GET", "response"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .requests
                .with_label_values(&["/pending", "GET", "200"])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .durations
                .with_label_values(&["/pending", "GET", "2xx"])
                .get_sample_count(),
            0
        );
    }
}
