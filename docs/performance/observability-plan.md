# Clumsies Observability Design

For engineers maintaining a Clumsies installation, this design organizes performance evidence around throughput, latency, errors and saturation. The [detailed design](/zh/performance/observability-plan) defines the metric contracts, existing service objectives, all 40 panel migrations and acceptance checks.

## Collection and measurement boundaries

Prometheus collects Caddy, Server, dependency and host metrics; Alloy delivers container logs to Loki; Grafana joins their views; Alertmanager sends notifications. The existing host collector provides five-minute database and operational snapshots. These services observe one installation.

Server latency ends when the handler returns a Response, before full body transmission. Successful 2xx/3xx distributions and failed distributions have separate samples. Legacy route-only histograms remain explicitly labeled as mixed results. Business started, terminated and in-flight metrics exclude health and metrics routes. Pool acquisition timing covers only review creation and merge transactions, including acquisition wait, validation and connection creation, excluding BEGIN and transaction execution.

Client HTTP, sync, index and retrieval evidence stays local. Health.performance contains process-cumulative counts and outcome-specific duration buckets; interval analysis requires two snapshots from the same collection epoch. Index backlog and publication times, original queued operation times and successful Commit checks are distinct completion boundaries. Commit last_checked_success_at is project-scoped: select a project to read it; unscoped status and the Draft channel intentionally return None rather than an invented global aggregate.

## Dashboard responsibilities

| Dashboard | Decision supported |
| --- | --- |
| Overview | Confirm affected traffic, successful latency, errors and evidence confidence |
| Business API | Narrow by route/method, compare samples and correlate one request |
| Website Traffic | Investigate site entrance behavior, slow transmission and access logs |
| Infrastructure | Check dependencies, finite pool-acquisition coverage, resources and operational evidence |

Product record counts remain in the API page's collapsed secondary section. They are neither request throughput nor sync backlog. Cross-page links preserve time and applicable filters; one API request-correlation panel serves both entrance and backend logs.

## Data quality and service objectives

Expected targets distinguish up, failed scrape and missing. Database snapshot values require a successful database collection and host/database ages between 0 and 900 seconds; re-scraping an old textfile does not refresh its evidence. Idle traffic has no latency sample. Missing or unsupported observations remain unknown rather than zero. Negative ages from future past-event timestamps or clock rollback are invalid, never zero-age success. Backup/restore/release receipts remain visible as neutral recorded facts; current age or confidence separately requires trustworthy host sampling. A present future exit timestamp is invalid; a known startup failure may lack an exit timestamp. Backup failures retain the previous successful timestamp and show the latest task result separately. Certificate expiration is a future deadline and has a different meaning.

The existing availability, entrance-latency, error, RPO and RTO objectives remain. The 99% five-minute client-write objective still lacks a complete durable SLI: eligibility, first-ACK deduplication and expired unfinished operations must be defined and measured before reporting compliance. Same-host monitoring requires an independent observer to detect whole-host loss.

Use the [operations runbook](/performance/observability-runbook) for fault investigation and the [local load report](/performance/observability-load-report) for the measured HTTP baseline.
