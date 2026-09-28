# Retrieval Runs and Evaluation

This document defines the local retrieval-history and evaluation contract used
by the Rust daemon and native macOS Diagnostics. It complements the ranking
pipeline described in `docs/architecture.md`; it does not change the
agent-facing MCP `activate` request or response.

## Boundary

Every valid `activate_memory` request creates one Retrieval Run in the daemon's
central local SQLite database. A Run records the exact ranking inputs and
outputs needed to explain and evaluate that activation:

- Project, query, activation-state fingerprint, Effective Memory hash, Index
  Revision, parser, chunker, model, and ranking-profile revisions.
- Exact/BM25, dense-vector, RRF, reranker, and final ranks and scores for every
  candidate in the fused recall set.
- The stable exclusion reason for every unselected candidate and the
  `add`/`replace`/`reuse` delta action for every selected candidate.
- Stage latency, returned fragment/token counts, and a bounded error stage,
  code, and summary for failed Runs.
- The complete Effective Memory resource manifest and content-addressed
  resource blobs used by the Run.

The MCP response and persisted trace are assembled from the same in-memory
ranked candidates. Retrieval history is diagnostic and evaluative local state,
not Server telemetry. The Server exposes no Retrieval Run endpoint. The daemon
exports no telemetry unless an operator explicitly configures an OTLP trace
endpoint.

## Optional OTLP/OpenInference export

The resident daemon can send one OpenInference `RETRIEVER` span per
`activate_memory` call to any OTLP/HTTP protobuf collector. Phoenix is a
compatible collector, not a required Clumsies component. To enable a
self-hosted Phoenix instance, install or reinstall the LaunchAgent with:

```sh
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://localhost:6006/v1/traces \
  clumsiesd --install-launch-agent
```

The default span contains the opaque Project and Retrieval Run IDs, corpus and
model versions, result counts, stage latency, error code, and selected-document
content hashes and scores. It does not contain the query, Memory text, unit key,
path, heading, resource ID, Prompt, evaluation judgments, or frozen corpus.

An operator may explicitly include the query and selected candidate excerpts:

```sh
OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://localhost:6006/v1/traces \
CLUMSIES_OTEL_CAPTURE_CONTENT=true \
  clumsiesd --install-launch-agent
```

Treat that mode as production-data export and apply the organization's access,
retention, and residency controls to the collector. Export uses a bounded
background queue and a three-second request timeout. Queue pressure, collector
failure, and exporter rejection are logged locally but never change the
agent-facing retrieval result. Clumsies Retrieval History and Evaluation Cases
remain the durable product records; OTLP traces are optional observability
copies.

## Storage

Central `local.db` owns:

| Table | Purpose |
| --- | --- |
| `retrieval_runs` | Run identity, versions, status, latency, result counts, and failure |
| `retrieval_run_candidates` | One row per ranked unit with all stage values and disposition |
| `retrieval_run_resources` | Ordered Effective Memory corpus used by a Run |
| `retrieval_corpus_blobs` | Metadata for content-addressed corpus blobs |
| `evaluation_corpora` | Immutable, deduplicated corpus identity |
| `evaluation_corpus_resources` | Frozen resource manifest for a corpus |
| `evaluation_cases` | Versioned query and corpus pair pinned from one Run |
| `evaluation_judgments` | Human relevance 0–3 and missed-evidence judgments |

Resource bodies are stored under
`evaluation-corpora/blobs/<prefix>/<sha256>` beneath daemon Application
Support. Directories and files use owner-only permissions. Project Local
Storage remains limited to rebuildable Commit generations and search indexes;
changing or clearing that location does not delete Retrieval Runs or Evaluation
Cases.

Unpinned history is retained per Project and pruned after 500 Runs. A Run
referenced by an Evaluation Case is pinned. Clear History deletes only unpinned
Runs and garbage-collects content blobs no longer referenced by either a Run or
an Evaluation Corpus.

## Status and recovery

A Run is `running`, `succeeded`, or `failed`.

1. Daemon inserts `running` after validating that Project and query are present.
2. The ranking pipeline fills one completion trace.
3. Daemon atomically inserts candidates/resources and marks the Run terminal.
4. On restart, any remaining `running` Run becomes `failed` with
   `retrieval_interrupted`.

Failure to write diagnostic history is logged and does not change the
agent-facing retrieval result. A search or model failure still becomes a
terminal failed Run when the central database remains writable.

## Evaluation Case

Adding a successful Run to the Evaluation Set freezes:

```text
query
+ project_id
+ Effective Memory resource manifest and content blobs
+ source Run candidate trace
= immutable Evaluation Corpus and versioned judgments
```

Judgments identify a retrieved unit or a missed corpus resource and use
relevance levels 0–3. Replacing the judgment set requires
`expected_judgment_version`; a stale editor receives
`evaluation_judgment_conflict`. Export produces a self-contained versioned JSON
fixture with frozen resource bodies, source traces, judgments, and the current
benchmark report.

## Benchmark variants

The report evaluates the same source trace four ways:

| Variant | Rank |
| --- | --- |
| `b1_bm25` | Exact identity rank when present, otherwise BM25 rank |
| `b2_dense_vector` | Dense-vector rank |
| `b3_hybrid_rrf` | RRF rank |
| `b4_reranked` | Reranker rank |

Each variant reports Recall@20, nDCG@10, MRR, Resource Diversity, Scope
Violation, Stale Result, and warm p50/p95 stage latency. Scope Violation means a
ranked resource is absent from the frozen corpus. Stale Result means the
resource exists but the candidate's recorded full-resource hash differs from
the frozen corpus hash. Candidate-unit hashes and full-resource hashes are
stored separately.

These calculations make the evaluation pipeline executable; they do not claim
production retrieval quality. A representative query set and relevance labels
must come from real organization and project use and be reviewed by a human
before they become a quality gate.

## Local API

Native clients use typed XPC methods:

- `list_retrieval_runs`
- `get_retrieval_run`
- `create_evaluation_case`
- `replace_evaluation_judgments`
- `clear_retrieval_runs`
- `export_evaluation_set`

The canonical schemas are in
`packages/api-contract/openapi/clumsies.daemon.v1.yaml`. Desktop Diagnostics has
separate Runtime and Retrieval pages. Retrieval shows recent Runs, stage ranks
and scores, final disposition, relevance controls, missed evidence, benchmark
metrics, export, and clear-unpinned-history actions.

## Verification

Automated coverage includes:

- schema 17 to 18 migration and interrupted-Run recovery;
- successful and failed activation recording through the real daemon entry;
- identity between returned fragments and selected trace candidates;
- frozen corpus creation, relevance and missed-evidence replacement, and CAS;
- B1–B4 export, pinned-Run preservation, history clearing, and restart reuse;
- independent Scope Violation and Stale Result semantics;
- daemon OpenAPI generation and native Swift decoding.

The production-quality gate remains open until representative, human-reviewed
Evaluation Cases are collected and the exported B1–B4 baseline is reviewed.
