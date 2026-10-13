# Clumsies Local HTTP Load Test Report

For Server maintainers and performance reviewers, this report records a reproducible local project-metadata baseline and checks HTTP telemetry against complete client operations. The [detailed report](/zh/performance/observability-load-report) contains all six distributions, environment details, reproduction commands and evidence hashes.

## Measured result

Six independent 20-second runs at 10, 100 and 500 planned business operations/s completed all **24,400 operations and 27,450 HTTP requests**. Every response was HTTP 200 and passed content checks; no operation, transport, queue or execution timeout failed. Temporary fixtures were cleaned.

| Profile at 500 operations/s | Complete-operation p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| Read | 1.71 ms | 2.69 ms | 3.67 ms |
| Read/write | 1.68 ms | 23.00 ms | 115.90 ms |

Read/write updates perform GET plus conditional PATCH, so 500 operations/s schedules 625 HTTP requests/s. Its client-queue p99 was 78.29 ms and transport p99 was 72.74 ms. These quantiles describe different samples and cannot be added or subtracted to assign a cause.

## Conditions and statistical boundaries

The generator and release Server ran on one Apple M4 Mac mini, with local Docker PostgreSQL, 64 fixed projects, 16 client workers and a 5-second budget from planned arrival. Server used four Tokio workers and a pool limit of ten. Each rate was measured once without a separate steady-state warmup.

Complete-operation time includes scheduling, project locks, client queue, HTTP and decoding. All planned operations remain in the denominator, including work that starts or finishes after the scheduling window. Quantiles use nearest-rank raw samples; they are not Server histogram estimates. Before/after completed-response, started, response-terminated and histogram counts matched every HTTP response; business in-flight ended at zero.

## Evidence and interpretation

[Download all six raw results (JSON gzip)](/reports/observability-http-load.json.gz). The evidence includes every operation, collection snapshots, source/binary fingerprints, environment and cleanup state, without production data or authentication tokens. Source baseline is `6266c8a289f33f8624f7e213194b4b45518508ef`, with the instrumentation changes identified by the recorded fingerprints.

The test establishes this local baseline and count consistency. It does not identify a sustained capacity limit or demonstrate production capacity, long-term SLO, Caddy/TLS behavior, local retrieval, sync freshness or review-transaction pool-acquisition performance. Queue tails cannot be attributed to the database without the same-event resource and stage evidence. Use the [observability design](/performance/observability-plan) and [operations runbook](/performance/observability-runbook) to interpret those boundaries.
