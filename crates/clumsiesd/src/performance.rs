//! Bounded process-local performance summaries; no payloads, resource IDs, or network export.
//!
//! Counters reset with the process. Durations use a monotonic clock and are separated by
//! outcome; an unfinished guard records interruption when its future is dropped.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// Fixed operation names accepted by the local registry; unknown names collapse to `other`.
const OPERATIONS: &[&str] = &[
    "activate",
    "search_lock_wait",
    "storage_read_wait",
    "history_lock_wait",
    "history_persistence",
    "bm25",
    "vector",
    "fusion",
    "assembly",
    "model_gate_wait",
    "model_blocking_queue",
    "model_embedding",
    "model_rerank",
    "model_metadata",
    "model_build_units",
    "sync_lock_wait",
    "sync_cycle",
    "sync_execute",
    "commit_check",
    "commit_check_record",
    "commit_install",
    "commit_download",
    "commit_validate",
    "draft_attempt",
    "draft_server_ack",
    "http_json",
    "http_decode",
    "index_claim",
    "index_build",
    "index_publish",
    "index_maintenance",
    "index_failure_persistence",
];
/// Fixed terminal outcomes; interruption is reserved for scopes that never report a result.
const OUTCOMES: &[&str] = &[
    "success",
    "failed",
    "interrupted",
    "skipped",
    "partial",
    "converged",
    "no_change",
    "updated",
    "published",
    "superseded",
    "already_ready",
];
/// Finite cumulative histogram bounds in milliseconds; the terminal count is the +Inf bucket.
const BUCKETS_MS: [u64; 15] = [
    5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000, 60_000, 120_000, 300_000,
];
/// One registry for this resident process, including work continuing after caller cancellation.
static PERFORMANCE: LazyLock<Registry> = LazyLock::new(Registry::new);

/// Safe local diagnostic snapshot; not uploaded to the Server.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PerformanceSnapshot {
    /// Process owning these counters; a new process begins a new statistical window.
    pub process_id: u32,
    /// UTC time when this process initialized collection, not operating-system boot time.
    pub collection_started_at: String,
    /// UTC time of this snapshot, distinct from the last business event.
    pub snapshot_at: String,
    /// Monotonic length of this collection window, unaffected by wall-clock adjustments.
    pub collection_elapsed_ms: u64,
    /// Shared finite upper bounds, in milliseconds, for every cumulative histogram.
    pub bucket_upper_bounds_ms: Vec<u64>,
    /// Fixed operation names with at least one observed start in this process.
    pub operations: BTreeMap<String, OperationPerformance>,
}

/// Starts, current concurrency, and terminal distributions for one fixed operation.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperationPerformance {
    /// Observed starts, including work that later fails or is interrupted.
    pub started: u64,
    /// Scopes that have started but have not reported or dropped their terminal result.
    pub in_flight: u64,
    /// Fixed outcomes; absent outcomes mean no such event has been observed in this process.
    pub outcomes: BTreeMap<String, PerformanceDistribution>,
}

/// Completed duration samples for one outcome, including the time spent before interruption.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PerformanceDistribution {
    /// Terminal observations; also the cumulative +Inf bucket.
    pub count: u64,
    /// Sum of monotonic elapsed microseconds for these observations.
    pub total_us: u64,
    /// Cumulative counts at the snapshot's shared finite millisecond bounds.
    pub buckets: Vec<u64>,
}

/// Owns a bounded map of counters and their collection epoch.
struct Registry {
    /// Collection epoch initialized before resident work begins.
    started_at: String,
    /// Monotonic collection epoch used to report the statistical window's length.
    started: Instant,
    /// Small, synchronous updates; no business I/O occurs while this mutex is held.
    operations: Mutex<BTreeMap<String, OperationPerformance>>,
}

impl Registry {
    /// Start an empty process-local statistical window.
    fn new() -> Self {
        Self {
            started_at: chrono::Utc::now().to_rfc3339(),
            started: Instant::now(),
            operations: Mutex::new(BTreeMap::new()),
        }
    }

    /// Begin one bounded observation without allowing a diagnostics failure to fail business work.
    fn start(&self, operation: &'static str) -> Observation<'_> {
        let operation = if OPERATIONS.contains(&operation) {
            operation
        } else {
            "other"
        };
        if let Ok(mut operations) = self.operations.lock() {
            let stats = operations.entry(operation.to_owned()).or_default();
            stats.started = stats.started.saturating_add(1);
            stats.in_flight = stats.in_flight.saturating_add(1);
        }
        Observation {
            registry: self,
            operation,
            started: Instant::now(),
            outcome: "interrupted",
            terminal_elapsed_us: None,
        }
    }

    /// Read a coherent snapshot; unavailable collection is represented by absence, not zero.
    fn snapshot(&self) -> Option<PerformanceSnapshot> {
        self.operations
            .lock()
            .ok()
            .map(|operations| PerformanceSnapshot {
                process_id: std::process::id(),
                collection_started_at: self.started_at.clone(),
                snapshot_at: chrono::Utc::now().to_rfc3339(),
                collection_elapsed_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX))
                    as u64,
                bucket_upper_bounds_ms: BUCKETS_MS.to_vec(),
                operations: operations.clone(),
            })
    }
}

/// A business scope whose terminal metrics are balanced by ownership.
pub(crate) struct Observation<'a> {
    /// Registry owning this scope's collection window.
    registry: &'a Registry,
    /// Normalized fixed operation name.
    operation: &'static str,
    /// Monotonic start used for completed and interrupted observations.
    started: Instant,
    /// Explicit result, or `interrupted` when the owner never reports one.
    outcome: &'static str,
    /// Explicit terminal boundary, shared by completion logs and the recorded distribution.
    terminal_elapsed_us: Option<u64>,
}

impl Observation<'_> {
    /// Complete with one fixed outcome and return the elapsed microseconds for safe event logs.
    pub(crate) fn finish(mut self, outcome: &'static str) -> u64 {
        self.outcome = if OUTCOMES.contains(&outcome) {
            outcome
        } else {
            "other"
        };
        let elapsed_us = self.elapsed_us();
        self.terminal_elapsed_us = Some(elapsed_us);
        elapsed_us
    }

    /// Complete according to the actual result without copying or changing its error.
    pub(crate) fn finish_result<T, E>(self, result: &Result<T, E>) -> u64 {
        self.finish(if result.is_ok() { "success" } else { "failed" })
    }

    /// Elapsed monotonic microseconds, saturated only at the wire format's integer limit.
    pub(crate) fn elapsed_us(&self) -> u64 {
        self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        let elapsed_us = self
            .terminal_elapsed_us
            .unwrap_or_else(|| self.elapsed_us());
        if let Ok(mut operations) = self.registry.operations.lock() {
            let stats = operations.entry(self.operation.to_owned()).or_default();
            stats.in_flight = stats.in_flight.saturating_sub(1);
            let distribution = stats
                .outcomes
                .entry(self.outcome.to_owned())
                .or_insert_with(|| PerformanceDistribution {
                    count: 0,
                    total_us: 0,
                    buckets: vec![0; BUCKETS_MS.len()],
                });
            distribution.count = distribution.count.saturating_add(1);
            distribution.total_us = distribution.total_us.saturating_add(elapsed_us);
            for (count, bound_ms) in distribution.buckets.iter_mut().zip(BUCKETS_MS) {
                if elapsed_us <= bound_ms * 1_000 {
                    *count = count.saturating_add(1);
                }
            }
        }
    }
}

/// Initialize collection before the resident state's startup and background work.
pub(crate) fn initialize() {
    LazyLock::force(&PERFORMANCE);
}

/// Begin a local observation; only fixed operation names can create distinct series.
pub(crate) fn start(operation: &'static str) -> Observation<'static> {
    PERFORMANCE.start(operation)
}

/// Capture safe process-local counters, or absence if collection is unavailable.
pub(crate) fn snapshot() -> Option<PerformanceSnapshot> {
    PERFORMANCE.snapshot()
}

/// Measure a fallible operation through its real return, preserving errors and cancellation.
/// Heap-pin the action so nested instrumentation does not enlarge the caller's async stack frame.
///
/// # Errors
/// Returns the supplied operation's error unchanged; dropping this future records interruption.
pub(crate) fn observe<T, E>(
    operation: &'static str,
    action: impl Future<Output = Result<T, E>>,
) -> impl Future<Output = Result<T, E>> {
    let action = Box::pin(action);
    async move {
        let observation = start(operation);
        let result = action.await;
        observation.finish_result(&result);
        result
    }
}

/// Measure waiting for an infallible synchronization primitive without changing ownership.
/// Heap-pin the action to keep the wrapper's future independent of the action's frame size.
pub(crate) fn wait<T>(
    operation: &'static str,
    action: impl Future<Output = T>,
) -> impl Future<Output = T> {
    let action = Box::pin(action);
    async move {
        let observation = start(operation);
        let result = action.await;
        observation.finish("success");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_wrappers_do_not_inline_large_operation_frames() {
        let payload = [0_u8; 65_536];
        let fallible = async move {
            std::future::pending::<()>().await;
            std::hint::black_box(payload);
            Ok::<(), ()>(())
        };
        let infallible = async move {
            std::future::pending::<()>().await;
            std::hint::black_box(payload);
        };
        assert!(std::mem::size_of_val(&observe("activate", fallible)) < 512);
        assert!(std::mem::size_of_val(&wait("search_lock_wait", infallible)) < 512);
    }

    #[test]
    fn snapshot_balances_terminal_outcomes_and_bounds_names() {
        let registry = Registry::new();
        let interrupted = registry.start("activate");
        registry.start("activate").finish("success");
        registry.start("activate").finish("failed");
        assert_eq!(
            registry.snapshot().unwrap().operations["activate"].in_flight,
            1
        );
        drop(interrupted);
        registry
            .start("PRIVATE_PATH_OR_QUERY")
            .finish("PRIVATE_ERROR");
        let snapshot = registry.snapshot().unwrap();
        let stats = &snapshot.operations["activate"];
        assert_eq!(stats.started, 3);
        assert_eq!(stats.in_flight, 0);
        assert_eq!(
            stats
                .outcomes
                .values()
                .map(|value| value.count)
                .sum::<u64>(),
            3
        );
        assert_eq!(stats.outcomes["interrupted"].count, 1);
        for distribution in stats.outcomes.values() {
            assert_eq!(
                distribution.buckets.len(),
                snapshot.bucket_upper_bounds_ms.len()
            );
            assert!(
                distribution
                    .buckets
                    .windows(2)
                    .all(|pair| pair[0] <= pair[1])
            );
            assert!(
                distribution
                    .buckets
                    .iter()
                    .all(|value| *value <= distribution.count)
            );
        }
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("PRIVATE"));
        assert_eq!(
            serde_json::from_str::<PerformanceSnapshot>(&json).unwrap(),
            snapshot
        );
    }
}
