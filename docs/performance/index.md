# End-to-end latency optimization

This section records the performance model, implementation changes, and dated evidence across Server, network, daemon, and macOS boundaries.

## Reading order

1. [Latency model and diagnosis](/performance/latency-model)
2. [Server hot path](/performance/server-hot-path)
3. [macOS first-ready](/performance/macos-first-ready)
4. [Cross-region gzip experiment](/performance/gzip-experiment)
5. [Evidence ledger](/performance/evidence-ledger)
6. [Client loading and request audit](/performance/client-loading-audit) — September 17 loading states, request scheduling, and remaining work.

7. [Client error feedback audit](/performance/client-error-feedback-audit) — failure presentation, recovery, and regression evidence.

The [signing appendix](/performance/signing-boundary) separates local identity failures from latency regressions. Measurements are snapshots tied to a revision, environment, request shape, and completion definition—not permanent SLOs.
