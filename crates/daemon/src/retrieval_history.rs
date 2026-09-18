use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use super::{DaemonError, DaemonState, MemoryKind, SourceLocator, SourceScope};

const RETRIEVAL_RUN_RETENTION_PER_PROJECT: i64 = 500;
const RETRIEVAL_EXCERPT_CHARS: usize = 1_200;
const EVALUATION_FIXTURE_VERSION: u32 = 2;
const EVALUATION_SUGGESTION_LIMIT: usize = 5;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalRunStatus {
    Running,
    Succeeded,
    Failed,
}

impl RetrievalRunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            _ => Err(history_corrupt(format!(
                "Unknown Retrieval Run status: {value}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalExclusionReason {
    Selected,
    BelowRelevance,
    Overlap,
    PerResourceLimit,
    TokenBudget,
    FragmentLimit,
    NotReranked,
}

impl RetrievalExclusionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::BelowRelevance => "below_relevance",
            Self::Overlap => "overlap",
            Self::PerResourceLimit => "per_resource_limit",
            Self::TokenBudget => "token_budget",
            Self::FragmentLimit => "fragment_limit",
            Self::NotReranked => "not_reranked",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "selected" => Ok(Self::Selected),
            "below_relevance" => Ok(Self::BelowRelevance),
            "overlap" => Ok(Self::Overlap),
            "per_resource_limit" => Ok(Self::PerResourceLimit),
            "token_budget" => Ok(Self::TokenBudget),
            "fragment_limit" => Ok(Self::FragmentLimit),
            "not_reranked" => Ok(Self::NotReranked),
            _ => Err(history_corrupt(format!(
                "Unknown retrieval exclusion reason: {value}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalDeltaAction {
    Add,
    Replace,
    Reuse,
}

impl RetrievalDeltaAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Replace => "replace",
            Self::Reuse => "reuse",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "add" => Ok(Self::Add),
            "replace" => Ok(Self::Replace),
            "reuse" => Ok(Self::Reuse),
            _ => Err(history_corrupt(format!(
                "Unknown retrieval delta action: {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetrievalStageLatencies {
    pub effective_memory_us: u64,
    pub index_ensure_us: u64,
    pub bm25_us: u64,
    pub embedding_us: u64,
    pub vector_us: u64,
    pub rrf_us: u64,
    pub rerank_us: u64,
    pub assembly_us: u64,
    pub persistence_us: u64,
    pub total_us: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetrievalRunListRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub status: Option<RetrievalRunStatus>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetrievalRunRequest {
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetrievalRunListResponse {
    pub items: Vec<RetrievalRun>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetrievalRun {
    pub run_id: String,
    pub project_id: String,
    pub query: String,
    pub activation_state_fingerprint: String,
    pub status: RetrievalRunStatus,
    pub effective_hash: Option<String>,
    pub index_revision: Option<String>,
    pub resource_count: u64,
    pub unit_count: u64,
    pub parser_version: Option<String>,
    pub chunker_version: Option<String>,
    pub model_revision: Option<String>,
    pub ranking_profile: Option<String>,
    pub latencies: RetrievalStageLatencies,
    pub returned_fragment_count: u64,
    pub returned_token_count: u64,
    pub error_stage: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub evaluation_case_id: Option<String>,
    pub evaluation_case_status: Option<EvaluationCaseStatus>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RetrievalCandidate {
    pub unit_key: String,
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub heading_path: Vec<String>,
    pub locator: SourceLocator,
    pub content_hash: String,
    pub resource_content_hash: String,
    pub token_count: u64,
    pub evidence_excerpt: String,
    pub exact_rank: Option<u64>,
    pub bm25_rank: Option<u64>,
    pub bm25_score: Option<f64>,
    pub vector_rank: Option<u64>,
    pub vector_score: Option<f64>,
    pub rrf_rank: Option<u64>,
    pub rrf_score: Option<f64>,
    pub reranker_rank: Option<u64>,
    pub reranker_logit: Option<f64>,
    pub reranker_relevance: Option<f64>,
    pub final_rank: Option<u64>,
    pub selected: bool,
    pub exclusion_reason: RetrievalExclusionReason,
    pub delta_action: Option<RetrievalDeltaAction>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RetrievalRunDetail {
    pub run: RetrievalRun,
    pub candidates: Vec<RetrievalCandidate>,
    pub evaluation_case: Option<EvaluationCase>,
    pub evidence: Vec<EvaluationEvidence>,
    pub evidence_suggestions: Vec<EvaluationEvidenceSuggestion>,
    pub report: Option<RetrievalBenchmarkReport>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateEvaluationCaseRequest {
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResolveEvaluationCaseRequest {
    pub case_id: String,
    pub expected_version: u64,
    pub evidence: Vec<EvaluationEvidenceInput>,
    #[serde(default)]
    pub none_matched: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EvaluationEvidenceInput {
    pub resource_id: String,
    #[serde(default)]
    pub unit_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationCaseStatus {
    Draft,
    NeedsEvidence,
    Ready,
}

impl EvaluationCaseStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::NeedsEvidence => "needs_evidence",
            Self::Ready => "ready",
        }
    }

    fn parse(value: &str) -> Result<Self, DaemonError> {
        match value {
            "draft" => Ok(Self::Draft),
            "needs_evidence" => Ok(Self::NeedsEvidence),
            "ready" => Ok(Self::Ready),
            _ => Err(history_corrupt(format!(
                "Unknown Evaluation Case status: {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EvaluationCase {
    pub case_id: String,
    pub source_run_id: String,
    pub corpus_id: String,
    pub project_id: String,
    pub query: String,
    pub status: EvaluationCaseStatus,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EvaluationEvidence {
    pub evidence_id: String,
    pub case_id: String,
    pub resource_id: String,
    pub unit_key: Option<String>,
    pub evidence_excerpt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct EvaluationCorpusResource {
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub title: String,
    pub content_hash: String,
    pub source_commit_id: Option<String>,
    pub draft_id: Option<String>,
    pub draft_revision: Option<String>,
    pub preview: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalFailureStage {
    Fusion,
    Reranking,
    Assembly,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EvaluationEvidenceSuggestion {
    pub resource_id: String,
    pub unit_key: String,
    pub path: String,
    pub heading_path: Vec<String>,
    pub evidence_excerpt: String,
    pub model_relevance: Option<f64>,
    pub likely_failure_stage: RetrievalFailureStage,
    pub exclusion_reason: RetrievalExclusionReason,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct EvaluationCaseDetail {
    pub evaluation_case: EvaluationCase,
    pub evidence: Vec<EvaluationEvidence>,
    pub evidence_suggestions: Vec<EvaluationEvidenceSuggestion>,
    pub report: Option<RetrievalBenchmarkReport>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClearRetrievalRunsRequest {
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClearRetrievalRunsResponse {
    pub deleted_run_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExportEvaluationSetRequest {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub case_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ExportEvaluationSetResponse {
    pub fixture_json: String,
    pub report: RetrievalBenchmarkReport,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalBenchmarkVariant {
    Bm25,
    DenseVector,
    HybridRrf,
    Reranked,
}

impl RetrievalBenchmarkVariant {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bm25 => "b1_bm25",
            Self::DenseVector => "b2_dense_vector",
            Self::HybridRrf => "b3_hybrid_rrf",
            Self::Reranked => "b4_reranked",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RetrievalBenchmarkMetrics {
    pub case_count: u64,
    pub recall_at_20: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,
    pub resource_diversity: f64,
    pub scope_violation: f64,
    pub stale_result: f64,
    pub warm_p50_us: u64,
    pub warm_p95_us: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RetrievalBenchmarkReport {
    pub variants: BTreeMap<String, RetrievalBenchmarkMetrics>,
}

#[derive(Clone, Debug)]
pub(crate) struct RetrievalCorpusResourceInput {
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub title: String,
    pub content: String,
    pub content_hash: String,
    pub source_commit_id: Option<String>,
    pub draft_id: Option<String>,
    pub draft_revision: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct RetrievalCandidateInput {
    pub unit_key: String,
    pub resource_id: String,
    pub scope: SourceScope,
    pub kind: MemoryKind,
    pub path: String,
    pub heading_path: Vec<String>,
    pub locator: SourceLocator,
    pub content_hash: String,
    pub resource_content_hash: String,
    pub token_count: usize,
    pub evidence_excerpt: String,
    pub exact_rank: Option<usize>,
    pub bm25_rank: Option<usize>,
    pub bm25_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub vector_score: Option<f32>,
    pub rrf_rank: Option<usize>,
    pub rrf_score: Option<f32>,
    pub reranker_rank: Option<usize>,
    pub reranker_logit: Option<f32>,
    pub reranker_relevance: Option<f32>,
    pub final_rank: Option<usize>,
    pub exclusion_reason: RetrievalExclusionReason,
    pub delta_action: Option<RetrievalDeltaAction>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RetrievalRunCompletion {
    pub effective_hash: Option<String>,
    pub index_revision: Option<String>,
    pub resources: Vec<RetrievalCorpusResourceInput>,
    pub candidates: Vec<RetrievalCandidateInput>,
    pub unit_count: usize,
    pub parser_version: Option<String>,
    pub chunker_version: Option<String>,
    pub model_revision: Option<String>,
    pub ranking_profile: Option<String>,
    pub latencies: RetrievalStageLatencies,
    pub returned_fragment_count: usize,
    pub returned_token_count: usize,
    pub error_stage: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RetrievalCursor {
    created_at: String,
    run_id: String,
}

#[derive(Serialize)]
struct EvaluationFixture {
    version: u32,
    exported_at: String,
    cases: Vec<EvaluationFixtureCase>,
}

#[derive(Serialize)]
struct EvaluationFixtureCase {
    evaluation_case: EvaluationCase,
    resources: Vec<EvaluationFixtureResource>,
    evidence: Vec<EvaluationEvidence>,
    source_run: RetrievalRun,
    candidates: Vec<RetrievalCandidate>,
}

#[derive(Serialize)]
struct EvaluationFixtureResource {
    metadata: EvaluationCorpusResource,
    content: String,
}

pub(super) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS retrieval_runs (
            run_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            query TEXT NOT NULL,
            activation_state_fingerprint TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
            effective_hash TEXT,
            index_revision TEXT,
            resource_count BIGINT NOT NULL DEFAULT 0 CHECK (resource_count >= 0),
            unit_count BIGINT NOT NULL DEFAULT 0 CHECK (unit_count >= 0),
            parser_version TEXT,
            chunker_version TEXT,
            model_revision TEXT,
            ranking_profile TEXT,
            effective_memory_us BIGINT NOT NULL DEFAULT 0 CHECK (effective_memory_us >= 0),
            index_ensure_us BIGINT NOT NULL DEFAULT 0 CHECK (index_ensure_us >= 0),
            bm25_us BIGINT NOT NULL DEFAULT 0 CHECK (bm25_us >= 0),
            embedding_us BIGINT NOT NULL DEFAULT 0 CHECK (embedding_us >= 0),
            vector_us BIGINT NOT NULL DEFAULT 0 CHECK (vector_us >= 0),
            rrf_us BIGINT NOT NULL DEFAULT 0 CHECK (rrf_us >= 0),
            rerank_us BIGINT NOT NULL DEFAULT 0 CHECK (rerank_us >= 0),
            assembly_us BIGINT NOT NULL DEFAULT 0 CHECK (assembly_us >= 0),
            persistence_us BIGINT NOT NULL DEFAULT 0 CHECK (persistence_us >= 0),
            total_us BIGINT NOT NULL DEFAULT 0 CHECK (total_us >= 0),
            returned_fragment_count BIGINT NOT NULL DEFAULT 0
                CHECK (returned_fragment_count >= 0),
            returned_token_count BIGINT NOT NULL DEFAULT 0 CHECK (returned_token_count >= 0),
            error_stage TEXT,
            error_code TEXT,
            error_summary TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            completed_at TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_retrieval_runs_project_created
         ON retrieval_runs (project_id, created_at DESC, run_id DESC)",
        "CREATE TABLE IF NOT EXISTS retrieval_run_candidates (
            run_id TEXT NOT NULL,
            candidate_order BIGINT NOT NULL CHECK (candidate_order >= 0),
            unit_key TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
            path TEXT NOT NULL,
            heading_path_json TEXT NOT NULL,
            locator_json TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            resource_content_hash TEXT NOT NULL,
            token_count BIGINT NOT NULL CHECK (token_count >= 0),
            evidence_excerpt TEXT NOT NULL,
            exact_rank BIGINT,
            bm25_rank BIGINT,
            bm25_score REAL,
            vector_rank BIGINT,
            vector_score REAL,
            rrf_rank BIGINT,
            rrf_score REAL,
            reranker_rank BIGINT,
            reranker_logit REAL,
            reranker_relevance REAL,
            final_rank BIGINT,
            selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
            exclusion_reason TEXT NOT NULL CHECK (exclusion_reason IN (
                'selected', 'below_relevance', 'overlap', 'per_resource_limit',
                'token_budget', 'fragment_limit', 'not_reranked'
            )),
            delta_action TEXT CHECK (delta_action IN ('add', 'replace', 'reuse')),
            PRIMARY KEY (run_id, unit_key)
        )",
        "CREATE INDEX IF NOT EXISTS idx_retrieval_candidates_run_order
         ON retrieval_run_candidates (run_id, candidate_order)",
        "CREATE TABLE IF NOT EXISTS retrieval_corpus_blobs (
            content_hash TEXT PRIMARY KEY,
            byte_length BIGINT NOT NULL CHECK (byte_length >= 0),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "CREATE TABLE IF NOT EXISTS retrieval_run_resources (
            run_id TEXT NOT NULL,
            resource_order BIGINT NOT NULL CHECK (resource_order >= 0),
            resource_id TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            content_preview TEXT NOT NULL,
            source_commit_id TEXT,
            draft_id TEXT,
            draft_revision TEXT,
            PRIMARY KEY (run_id, resource_id)
        )",
        "CREATE TABLE IF NOT EXISTS evaluation_corpora (
            corpus_id TEXT PRIMARY KEY,
            effective_hash TEXT NOT NULL,
            source_run_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "CREATE TABLE IF NOT EXISTS evaluation_corpus_resources (
            corpus_id TEXT NOT NULL,
            resource_order BIGINT NOT NULL CHECK (resource_order >= 0),
            resource_id TEXT NOT NULL,
            scope TEXT NOT NULL CHECK (scope IN ('org', 'project')),
            kind TEXT NOT NULL CHECK (kind IN ('context', 'rule', 'workflow', 'memory')),
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            content_preview TEXT NOT NULL,
            source_commit_id TEXT,
            draft_id TEXT,
            draft_revision TEXT,
            PRIMARY KEY (corpus_id, resource_id)
        )",
        "CREATE TABLE IF NOT EXISTS evaluation_cases (
            case_id TEXT PRIMARY KEY,
            source_run_id TEXT NOT NULL UNIQUE,
            corpus_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            query TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'draft' CHECK (
                status IN ('draft', 'needs_evidence', 'ready')
            ),
            version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "CREATE INDEX IF NOT EXISTS idx_evaluation_cases_project_created
         ON evaluation_cases (project_id, created_at DESC, case_id DESC)",
        "CREATE TABLE IF NOT EXISTS evaluation_evidence (
            evidence_id TEXT PRIMARY KEY,
            case_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            unit_key TEXT,
            evidence_excerpt TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            UNIQUE (case_id, resource_id, unit_key)
        )",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }
    Ok(())
}

pub(super) async fn migrate_schema_17_to_18(pool: &SqlitePool) -> Result<(), DaemonError> {
    migrate(pool).await?;
    let mut tx = pool.begin().await?;
    for statement in [
        "DROP INDEX IF EXISTS idx_evaluation_cases_project_created",
        "DROP TABLE evaluation_evidence",
        "DROP TABLE evaluation_cases",
        "CREATE TABLE evaluation_cases (
            case_id TEXT PRIMARY KEY,
            source_run_id TEXT NOT NULL UNIQUE,
            corpus_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            query TEXT NOT NULL,
            query_category TEXT,
            notes TEXT,
            judgment_version BIGINT NOT NULL DEFAULT 1 CHECK (judgment_version > 0),
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )",
        "CREATE INDEX idx_evaluation_cases_project_created
         ON evaluation_cases (project_id, created_at DESC, case_id DESC)",
        "CREATE TABLE evaluation_judgments (
            judgment_id TEXT PRIMARY KEY,
            case_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            unit_key TEXT,
            relevance BIGINT NOT NULL CHECK (relevance BETWEEN 0 AND 3),
            missed INTEGER NOT NULL CHECK (missed IN (0, 1)),
            evidence_excerpt TEXT NOT NULL,
            notes TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            UNIQUE (case_id, resource_id, unit_key, missed)
        )",
        "INSERT INTO daemon_meta (key, value)
         VALUES ('schema_version', '18')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn migrate_schema_18_to_19(pool: &SqlitePool) -> Result<(), DaemonError> {
    let mut tx = pool.begin().await?;
    for statement in [
        "DROP INDEX IF EXISTS idx_evaluation_cases_project_created",
        "ALTER TABLE evaluation_cases RENAME TO evaluation_cases_v18",
        "ALTER TABLE evaluation_judgments RENAME TO evaluation_judgments_v18",
        "CREATE TABLE evaluation_cases (
            case_id TEXT PRIMARY KEY,
            source_run_id TEXT NOT NULL UNIQUE,
            corpus_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            query TEXT NOT NULL,
            status TEXT NOT NULL CHECK (
                status IN ('draft', 'needs_evidence', 'ready')
            ),
            version BIGINT NOT NULL CHECK (version > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        "INSERT INTO evaluation_cases (
            case_id, source_run_id, corpus_id, project_id, query, status,
            version, created_at, updated_at
         )
         SELECT
            old.case_id,
            old.source_run_id,
            old.corpus_id,
            old.project_id,
            old.query,
            CASE WHEN EXISTS (
                SELECT 1
                FROM evaluation_judgments_v18 judgment
                WHERE judgment.case_id = old.case_id
                  AND judgment.relevance > 0
            ) THEN 'ready' ELSE 'draft' END,
            old.judgment_version,
            old.created_at,
            old.updated_at
         FROM evaluation_cases_v18 old",
        "CREATE INDEX idx_evaluation_cases_project_created
         ON evaluation_cases (project_id, created_at DESC, case_id DESC)",
        "CREATE TABLE evaluation_evidence (
            evidence_id TEXT PRIMARY KEY,
            case_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            unit_key TEXT,
            evidence_excerpt TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE (case_id, resource_id, unit_key)
        )",
        "INSERT INTO evaluation_evidence (
            evidence_id, case_id, resource_id, unit_key, evidence_excerpt, created_at
         )
         SELECT
            judgment_id,
            case_id,
            resource_id,
            unit_key,
            evidence_excerpt,
            created_at
         FROM evaluation_judgments_v18
         WHERE relevance > 0",
        "DROP TABLE evaluation_judgments_v18",
        "DROP TABLE evaluation_cases_v18",
        "INSERT INTO daemon_meta (key, value)
         VALUES ('schema_version', '19')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ] {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn recover_interrupted_runs(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE retrieval_runs
         SET status = 'failed',
             error_stage = COALESCE(error_stage, 'interrupted'),
             error_code = COALESCE(error_code, 'retrieval_interrupted'),
             error_summary = COALESCE(
                 error_summary,
                 'The daemon stopped before this Retrieval Run completed.'
             ),
             completed_at = COALESCE(
                 completed_at,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )
         WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) fn activation_state_fingerprint(state: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    match state {
        Some(state) => {
            hasher.update(b"present\0");
            hasher.update(state.as_bytes());
        }
        None => hasher.update(b"absent"),
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(crate) async fn start_run(
    state: &DaemonState,
    project_id: &str,
    query: &str,
    activation_state_fingerprint: &str,
) -> Result<String, DaemonError> {
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO retrieval_runs (
            run_id, project_id, query, activation_state_fingerprint, status
         ) VALUES ($1, $2, $3, $4, 'running')",
    )
    .bind(&run_id)
    .bind(project_id)
    .bind(query)
    .bind(activation_state_fingerprint)
    .execute(&state.inner.pool)
    .await?;
    Ok(run_id)
}

pub(crate) async fn finish_run(
    state: &DaemonState,
    run_id: &str,
    completion: RetrievalRunCompletion,
) -> Result<(), DaemonError> {
    let _history = state.inner.retrieval_history_lock.lock().await;
    let persistence_started = std::time::Instant::now();
    persist_run_blobs(state, &completion.resources).await?;
    let mut tx = state.inner.pool.begin().await?;
    insert_run_resources(&mut tx, run_id, &completion.resources).await?;
    insert_run_candidates(&mut tx, run_id, &completion.candidates).await?;
    let status = if completion.error_code.is_some() {
        RetrievalRunStatus::Failed
    } else {
        RetrievalRunStatus::Succeeded
    };
    let persistence_us = elapsed_us(persistence_started);
    let resource_count = usize_to_i64(completion.resources.len(), "resource_count")?;
    let unit_count = usize_to_i64(completion.unit_count, "unit_count")?;
    let returned_fragment_count = usize_to_i64(
        completion.returned_fragment_count,
        "returned_fragment_count",
    )?;
    let returned_token_count =
        usize_to_i64(completion.returned_token_count, "returned_token_count")?;
    let latencies = completion.latencies;
    let updated = sqlx::query(
        "UPDATE retrieval_runs
         SET status = $2,
             effective_hash = $3,
             index_revision = $4,
             resource_count = $5,
             unit_count = $6,
             parser_version = $7,
             chunker_version = $8,
             model_revision = $9,
             ranking_profile = $10,
             effective_memory_us = $11,
             index_ensure_us = $12,
             bm25_us = $13,
             embedding_us = $14,
             vector_us = $15,
             rrf_us = $16,
             rerank_us = $17,
             assembly_us = $18,
             persistence_us = $19,
             total_us = $20,
             returned_fragment_count = $21,
             returned_token_count = $22,
             error_stage = $23,
             error_code = $24,
             error_summary = $25,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE run_id = $1 AND status = 'running'",
    )
    .bind(run_id)
    .bind(status.as_str())
    .bind(completion.effective_hash)
    .bind(completion.index_revision)
    .bind(resource_count)
    .bind(unit_count)
    .bind(completion.parser_version)
    .bind(completion.chunker_version)
    .bind(completion.model_revision)
    .bind(completion.ranking_profile)
    .bind(u64_to_i64(
        latencies.effective_memory_us,
        "effective_memory_us",
    )?)
    .bind(u64_to_i64(latencies.index_ensure_us, "index_ensure_us")?)
    .bind(u64_to_i64(latencies.bm25_us, "bm25_us")?)
    .bind(u64_to_i64(latencies.embedding_us, "embedding_us")?)
    .bind(u64_to_i64(latencies.vector_us, "vector_us")?)
    .bind(u64_to_i64(latencies.rrf_us, "rrf_us")?)
    .bind(u64_to_i64(latencies.rerank_us, "rerank_us")?)
    .bind(u64_to_i64(latencies.assembly_us, "assembly_us")?)
    .bind(u64_to_i64(persistence_us, "persistence_us")?)
    .bind(u64_to_i64(latencies.total_us, "total_us")?)
    .bind(returned_fragment_count)
    .bind(returned_token_count)
    .bind(completion.error_stage)
    .bind(completion.error_code)
    .bind(completion.error_summary)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(history_corrupt(format!(
            "Retrieval Run {run_id} is missing or already terminal"
        )));
    }
    tx.commit().await?;
    prune_runs(
        state,
        &completion_project_id(&state.inner.pool, run_id).await?,
    )
    .await?;
    Ok(())
}

/// Mark a retrieval deadline without waiting for the history serialization
/// lock or persisting the (potentially large) corpus/candidate payload.
///
/// The activation deadline is also an API/terminal-state deadline. Running
/// the normal completion path here could wait behind another history writer
/// or synchronous blob I/O after the retrieval computation has already timed
/// out, leaving the durable Run observably `running` for an unbounded period.
pub(crate) async fn finish_deadline_run(
    state: &DaemonState,
    run_id: &str,
    error_stage: &str,
    total_us: u64,
) -> Result<(), DaemonError> {
    let updated = sqlx::query(
        "UPDATE retrieval_runs
         SET status = 'failed',
             total_us = $2,
             error_stage = $3,
             error_code = 'activation_deadline',
             error_summary = 'Activation exceeded its retrieval deadline.',
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE run_id = $1 AND status = 'running'",
    )
    .bind(run_id)
    .bind(u64_to_i64(total_us, "total_us")?)
    .bind(error_stage)
    .execute(&state.inner.pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(history_corrupt(format!(
            "Retrieval Run {run_id} is missing or already terminal"
        )));
    }
    Ok(())
}

pub(crate) async fn record_persistence_failure(
    state: &DaemonState,
    run_id: &str,
    error: &DaemonError,
) -> Result<(), DaemonError> {
    sqlx::query(
        "UPDATE retrieval_runs
         SET status = 'failed',
             error_stage = 'persistence',
             error_code = 'retrieval_history_persistence_failed',
             error_summary = $2,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE run_id = $1 AND status = 'running'",
    )
    .bind(run_id)
    .bind(truncate_excerpt(&error.to_string()))
    .execute(&state.inner.pool)
    .await?;
    Ok(())
}

async fn insert_run_resources(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    resources: &[RetrievalCorpusResourceInput],
) -> Result<(), DaemonError> {
    for (index, resource) in resources.iter().enumerate() {
        sqlx::query(
            "INSERT INTO retrieval_run_resources (
                run_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview, source_commit_id, draft_id, draft_revision
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(run_id)
        .bind(usize_to_i64(index, "resource_order")?)
        .bind(&resource.resource_id)
        .bind(resource.scope.as_str())
        .bind(resource.kind.as_str())
        .bind(&resource.path)
        .bind(&resource.title)
        .bind(&resource.content_hash)
        .bind(truncate_excerpt(&resource.content))
        .bind(&resource.source_commit_id)
        .bind(&resource.draft_id)
        .bind(&resource.draft_revision)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_run_candidates(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    candidates: &[RetrievalCandidateInput],
) -> Result<(), DaemonError> {
    for (index, candidate) in candidates.iter().enumerate() {
        sqlx::query(
            "INSERT INTO retrieval_run_candidates (
                run_id, candidate_order, unit_key, resource_id, scope, kind, path,
                heading_path_json, locator_json, content_hash, resource_content_hash,
                token_count, evidence_excerpt,
                exact_rank, bm25_rank, bm25_score, vector_rank, vector_score,
                rrf_rank, rrf_score, reranker_rank, reranker_logit,
                reranker_relevance, final_rank, selected, exclusion_reason, delta_action
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17, $18, $19, $20, $21, $22,
                $23, $24, $25, $26, $27
             )",
        )
        .bind(run_id)
        .bind(usize_to_i64(index, "candidate_order")?)
        .bind(&candidate.unit_key)
        .bind(&candidate.resource_id)
        .bind(candidate.scope.as_str())
        .bind(candidate.kind.as_str())
        .bind(&candidate.path)
        .bind(serde_json::to_string(&candidate.heading_path)?)
        .bind(serde_json::to_string(&candidate.locator)?)
        .bind(&candidate.content_hash)
        .bind(&candidate.resource_content_hash)
        .bind(usize_to_i64(candidate.token_count, "token_count")?)
        .bind(truncate_excerpt(&candidate.evidence_excerpt))
        .bind(optional_usize_to_i64(candidate.exact_rank, "exact_rank")?)
        .bind(optional_usize_to_i64(candidate.bm25_rank, "bm25_rank")?)
        .bind(candidate.bm25_score.map(f64::from))
        .bind(optional_usize_to_i64(candidate.vector_rank, "vector_rank")?)
        .bind(candidate.vector_score.map(f64::from))
        .bind(optional_usize_to_i64(candidate.rrf_rank, "rrf_rank")?)
        .bind(candidate.rrf_score.map(f64::from))
        .bind(optional_usize_to_i64(
            candidate.reranker_rank,
            "reranker_rank",
        )?)
        .bind(candidate.reranker_logit.map(f64::from))
        .bind(candidate.reranker_relevance.map(f64::from))
        .bind(optional_usize_to_i64(candidate.final_rank, "final_rank")?)
        .bind(i64::from(
            candidate.exclusion_reason == RetrievalExclusionReason::Selected,
        ))
        .bind(candidate.exclusion_reason.as_str())
        .bind(candidate.delta_action.map(RetrievalDeltaAction::as_str))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn persist_run_blobs(
    state: &DaemonState,
    resources: &[RetrievalCorpusResourceInput],
) -> Result<(), DaemonError> {
    for resource in resources {
        let computed_hash = content_hash(&resource.content);
        if computed_hash != resource.content_hash {
            return Err(history_corrupt(format!(
                "Resource {} content hash does not match its Retrieval Run corpus",
                resource.resource_id
            )));
        }
        let path = corpus_blob_path(&state.inner.config.root_dir, &resource.content_hash)?;
        let content = resource.content.as_bytes();
        let existing_matches = match fs::read(&path) {
            Ok(existing) => existing == content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !existing_matches {
            super::project_storage::write_private_file(&path, content)?;
        }
        sqlx::query(
            "INSERT INTO retrieval_corpus_blobs (content_hash, byte_length)
             VALUES ($1, $2)
             ON CONFLICT(content_hash) DO UPDATE SET byte_length = excluded.byte_length",
        )
        .bind(&resource.content_hash)
        .bind(usize_to_i64(content.len(), "blob byte length")?)
        .execute(&state.inner.pool)
        .await?;
    }
    Ok(())
}

async fn completion_project_id(pool: &SqlitePool, run_id: &str) -> Result<String, DaemonError> {
    sqlx::query_scalar("SELECT project_id FROM retrieval_runs WHERE run_id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| DaemonError::NotFound(format!("Retrieval Run {run_id}")))
}

pub(super) async fn list_retrieval_runs(
    state: &DaemonState,
    request: RetrievalRunListRequest,
) -> Result<RetrievalRunListResponse, DaemonError> {
    let limit = request.limit.unwrap_or(50).clamp(1, 100) as i64;
    let status = request.status.map(RetrievalRunStatus::as_str);
    let cursor = request.cursor.as_deref().map(decode_cursor).transpose()?;
    let rows = sqlx::query(
        "SELECT r.*, e.case_id AS evaluation_case_id,
                e.status AS evaluation_case_status
         FROM retrieval_runs r
         LEFT JOIN evaluation_cases e ON e.source_run_id = r.run_id
         WHERE ($1 IS NULL OR r.project_id = $1)
           AND ($2 IS NULL OR r.status = $2)
           AND (
                $3 IS NULL
                OR r.created_at < $3
                OR (r.created_at = $3 AND r.run_id < $4)
           )
         ORDER BY r.created_at DESC, r.run_id DESC
         LIMIT $5",
    )
    .bind(request.project_id.as_deref())
    .bind(status)
    .bind(cursor.as_ref().map(|cursor| cursor.created_at.as_str()))
    .bind(cursor.as_ref().map(|cursor| cursor.run_id.as_str()))
    .bind(limit + 1)
    .fetch_all(&state.inner.pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let mut items = rows
        .into_iter()
        .take(limit as usize)
        .map(run_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        items
            .last()
            .map(|run| {
                encode_cursor(&RetrievalCursor {
                    created_at: run.created_at.clone(),
                    run_id: run.run_id.clone(),
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(RetrievalRunListResponse {
        items: std::mem::take(&mut items),
        next_cursor,
    })
}

pub(super) async fn get_retrieval_run(
    state: &DaemonState,
    request: RetrievalRunRequest,
) -> Result<RetrievalRunDetail, DaemonError> {
    load_run_detail(&state.inner.pool, &request.run_id).await
}

/// Loads one selected candidate from the immutable corpus captured for its
/// retrieval run. Activity details must never drift to the current memory.
pub(crate) async fn load_recall_fragment_content(
    state: &DaemonState,
    project_id: &str,
    run_id: &str,
    unit_key: &str,
) -> Result<(RetrievalCandidate, String, bool), DaemonError> {
    let _history = state.inner.retrieval_history_lock.lock().await;
    let actual_project: String =
        sqlx::query_scalar("SELECT project_id FROM retrieval_runs WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(&state.inner.pool)
            .await?
            .ok_or_else(|| DaemonError::NotFound(format!("Retrieval Run {run_id}")))?;
    if actual_project != project_id {
        return Err(DaemonError::NotFound(format!("Retrieval Run {run_id}")));
    }

    let row =
        sqlx::query("SELECT * FROM retrieval_run_candidates WHERE run_id = $1 AND unit_key = $2")
            .bind(run_id)
            .bind(unit_key)
            .fetch_optional(&state.inner.pool)
            .await?
            .ok_or_else(|| DaemonError::NotFound(format!("Retrieval candidate {unit_key}")))?;
    let candidate = candidate_from_row(row)?;
    if !candidate.selected {
        return Err(DaemonError::InvalidRequest(format!(
            "Retrieval candidate {unit_key} was not returned by this run"
        )));
    }

    let frozen_hash: String = sqlx::query_scalar(
        "SELECT content_hash FROM retrieval_run_resources
         WHERE run_id = $1 AND resource_id = $2",
    )
    .bind(run_id)
    .bind(&candidate.resource_id)
    .fetch_optional(&state.inner.pool)
    .await?
    .ok_or_else(|| {
        history_corrupt(format!(
            "Candidate {unit_key} has no frozen Retrieval Run resource"
        ))
    })?;
    if frozen_hash != candidate.resource_content_hash {
        return Err(history_corrupt(format!(
            "Candidate {unit_key} does not belong to its frozen Retrieval Run resource"
        )));
    }
    let frozen_content = fs::read_to_string(corpus_blob_path(
        &state.inner.config.root_dir,
        &frozen_hash,
    )?)?;
    if content_hash(&frozen_content) != frozen_hash {
        return Err(history_corrupt(format!(
            "Frozen Retrieval Run resource for candidate {unit_key} is corrupt"
        )));
    }
    let (content, truncated) = candidate_content(&candidate, &frozen_content)?;
    Ok((candidate, content, truncated))
}

fn candidate_content(
    candidate: &RetrievalCandidate,
    frozen_content: &str,
) -> Result<(String, bool), DaemonError> {
    let SourceLocator::MarkdownSpan {
        start_byte,
        end_byte,
        ..
    } = &candidate.locator;
    if *start_byte == 0 && *end_byte == 0 && candidate.unit_key.ends_with("/description") {
        // Descriptions are indexed as synthetic units but are not part of the
        // resource body blob. Historical runs retain only their excerpt.
        return Ok((
            candidate.evidence_excerpt.clone(),
            excerpt_is_truncated(&candidate.evidence_excerpt),
        ));
    }
    let content = frozen_content.get(*start_byte..*end_byte).ok_or_else(|| {
        history_corrupt(format!(
            "Candidate {} has an invalid frozen corpus locator",
            candidate.unit_key
        ))
    })?;
    if content_hash(content) != candidate.content_hash {
        return Err(history_corrupt(format!(
            "Candidate {} does not match its frozen corpus locator",
            candidate.unit_key
        )));
    }
    Ok((content.to_owned(), false))
}

async fn load_run_detail(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<RetrievalRunDetail, DaemonError> {
    let row = sqlx::query(
        "SELECT r.*, e.case_id AS evaluation_case_id,
                e.status AS evaluation_case_status
         FROM retrieval_runs r
         LEFT JOIN evaluation_cases e ON e.source_run_id = r.run_id
         WHERE r.run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("Retrieval Run {run_id}")))?;
    let run = run_from_row(row)?;
    let candidates = load_candidates(pool, run_id).await?;
    let evaluation_case = match run.evaluation_case_id.as_deref() {
        Some(case_id) => Some(load_evaluation_case(pool, case_id).await?),
        None => None,
    };
    let (evidence, evidence_suggestions, report) = match evaluation_case.as_ref() {
        Some(case) => {
            let report = if case.status == EvaluationCaseStatus::Ready {
                Some(benchmark_report(pool, std::slice::from_ref(case)).await?)
            } else {
                None
            };
            (
                load_evidence(pool, &case.case_id).await?,
                suggest_evidence(&candidates),
                report,
            )
        }
        None => (Vec::new(), Vec::new(), None),
    };
    Ok(RetrievalRunDetail {
        run,
        candidates,
        evaluation_case,
        evidence,
        evidence_suggestions,
        report,
    })
}

async fn load_candidates(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Vec<RetrievalCandidate>, DaemonError> {
    let rows = sqlx::query(
        "SELECT *
         FROM retrieval_run_candidates
         WHERE run_id = $1
         ORDER BY candidate_order",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(candidate_from_row).collect()
}

fn run_from_row(row: sqlx::sqlite::SqliteRow) -> Result<RetrievalRun, DaemonError> {
    Ok(RetrievalRun {
        run_id: row.try_get("run_id")?,
        project_id: row.try_get("project_id")?,
        query: row.try_get("query")?,
        activation_state_fingerprint: row.try_get("activation_state_fingerprint")?,
        status: RetrievalRunStatus::parse(row.try_get("status")?)?,
        effective_hash: row.try_get("effective_hash")?,
        index_revision: row.try_get("index_revision")?,
        resource_count: non_negative_u64(row.try_get("resource_count")?, "resource_count")?,
        unit_count: non_negative_u64(row.try_get("unit_count")?, "unit_count")?,
        parser_version: row.try_get("parser_version")?,
        chunker_version: row.try_get("chunker_version")?,
        model_revision: row.try_get("model_revision")?,
        ranking_profile: row.try_get("ranking_profile")?,
        latencies: RetrievalStageLatencies {
            effective_memory_us: non_negative_u64(
                row.try_get("effective_memory_us")?,
                "effective_memory_us",
            )?,
            index_ensure_us: non_negative_u64(row.try_get("index_ensure_us")?, "index_ensure_us")?,
            bm25_us: non_negative_u64(row.try_get("bm25_us")?, "bm25_us")?,
            embedding_us: non_negative_u64(row.try_get("embedding_us")?, "embedding_us")?,
            vector_us: non_negative_u64(row.try_get("vector_us")?, "vector_us")?,
            rrf_us: non_negative_u64(row.try_get("rrf_us")?, "rrf_us")?,
            rerank_us: non_negative_u64(row.try_get("rerank_us")?, "rerank_us")?,
            assembly_us: non_negative_u64(row.try_get("assembly_us")?, "assembly_us")?,
            persistence_us: non_negative_u64(row.try_get("persistence_us")?, "persistence_us")?,
            total_us: non_negative_u64(row.try_get("total_us")?, "total_us")?,
        },
        returned_fragment_count: non_negative_u64(
            row.try_get("returned_fragment_count")?,
            "returned_fragment_count",
        )?,
        returned_token_count: non_negative_u64(
            row.try_get("returned_token_count")?,
            "returned_token_count",
        )?,
        error_stage: row.try_get("error_stage")?,
        error_code: row.try_get("error_code")?,
        error_summary: row.try_get("error_summary")?,
        created_at: row.try_get("created_at")?,
        completed_at: row.try_get("completed_at")?,
        evaluation_case_id: row.try_get("evaluation_case_id")?,
        evaluation_case_status: row
            .try_get::<Option<String>, _>("evaluation_case_status")?
            .as_deref()
            .map(EvaluationCaseStatus::parse)
            .transpose()?,
    })
}

fn candidate_from_row(row: sqlx::sqlite::SqliteRow) -> Result<RetrievalCandidate, DaemonError> {
    let exclusion_reason =
        RetrievalExclusionReason::parse(row.try_get::<String, _>("exclusion_reason")?.as_str())?;
    let delta_action = row
        .try_get::<Option<String>, _>("delta_action")?
        .as_deref()
        .map(RetrievalDeltaAction::parse)
        .transpose()?;
    Ok(RetrievalCandidate {
        unit_key: row.try_get("unit_key")?,
        resource_id: row.try_get("resource_id")?,
        scope: parse_scope(row.try_get::<String, _>("scope")?.as_str())?,
        kind: parse_kind(row.try_get::<String, _>("kind")?.as_str())?,
        path: row.try_get("path")?,
        heading_path: serde_json::from_str(
            row.try_get::<String, _>("heading_path_json")?.as_str(),
        )?,
        locator: serde_json::from_str(row.try_get::<String, _>("locator_json")?.as_str())?,
        content_hash: row.try_get("content_hash")?,
        resource_content_hash: row.try_get("resource_content_hash")?,
        token_count: non_negative_u64(row.try_get("token_count")?, "token_count")?,
        evidence_excerpt: row.try_get("evidence_excerpt")?,
        exact_rank: optional_non_negative_u64(row.try_get("exact_rank")?, "exact_rank")?,
        bm25_rank: optional_non_negative_u64(row.try_get("bm25_rank")?, "bm25_rank")?,
        bm25_score: row.try_get("bm25_score")?,
        vector_rank: optional_non_negative_u64(row.try_get("vector_rank")?, "vector_rank")?,
        vector_score: row.try_get("vector_score")?,
        rrf_rank: optional_non_negative_u64(row.try_get("rrf_rank")?, "rrf_rank")?,
        rrf_score: row.try_get("rrf_score")?,
        reranker_rank: optional_non_negative_u64(row.try_get("reranker_rank")?, "reranker_rank")?,
        reranker_logit: row.try_get("reranker_logit")?,
        reranker_relevance: row.try_get("reranker_relevance")?,
        final_rank: optional_non_negative_u64(row.try_get("final_rank")?, "final_rank")?,
        selected: row.try_get::<i64, _>("selected")? != 0,
        exclusion_reason,
        delta_action,
    })
}

pub(super) async fn create_evaluation_case(
    state: &DaemonState,
    request: CreateEvaluationCaseRequest,
) -> Result<EvaluationCaseDetail, DaemonError> {
    let _history = state.inner.retrieval_history_lock.lock().await;
    let run = load_run(&state.inner.pool, &request.run_id).await?;
    let effective_hash = run.effective_hash.as_deref().ok_or_else(|| {
        DaemonError::InvalidRequest(
            "A Retrieval Run without an Effective Memory corpus cannot become an Evaluation Case"
                .to_owned(),
        )
    })?;
    let run_resources = load_run_resource_rows(&state.inner.pool, &request.run_id).await?;
    if run_resources.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "The Retrieval Run has no retained corpus resources".to_owned(),
        ));
    }
    let corpus_id = corpus_id(effective_hash, &run_resources)?;
    let case_id = format!("case_{}", Uuid::new_v4().simple());
    let mut tx = state.inner.pool.begin().await?;
    sqlx::query(
        "INSERT INTO evaluation_corpora (corpus_id, effective_hash, source_run_id)
         VALUES ($1, $2, $3)
         ON CONFLICT(corpus_id) DO NOTHING",
    )
    .bind(&corpus_id)
    .bind(effective_hash)
    .bind(&request.run_id)
    .execute(&mut *tx)
    .await?;
    for resource in &run_resources {
        sqlx::query(
            "INSERT INTO evaluation_corpus_resources (
                corpus_id, resource_order, resource_id, scope, kind, path, title,
                content_hash, content_preview, source_commit_id, draft_id, draft_revision
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT(corpus_id, resource_id) DO NOTHING",
        )
        .bind(&corpus_id)
        .bind(resource.resource_order)
        .bind(&resource.resource_id)
        .bind(&resource.scope)
        .bind(&resource.kind)
        .bind(&resource.path)
        .bind(&resource.title)
        .bind(&resource.content_hash)
        .bind(&resource.content_preview)
        .bind(&resource.source_commit_id)
        .bind(&resource.draft_id)
        .bind(&resource.draft_revision)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO evaluation_cases (
            case_id, source_run_id, corpus_id, project_id, query
         ) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT(source_run_id) DO NOTHING",
    )
    .bind(&case_id)
    .bind(&request.run_id)
    .bind(&corpus_id)
    .bind(&run.project_id)
    .bind(&run.query)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let actual_case_id: String =
        sqlx::query_scalar("SELECT case_id FROM evaluation_cases WHERE source_run_id = $1")
            .bind(&request.run_id)
            .fetch_one(&state.inner.pool)
            .await?;
    load_evaluation_case_detail(&state.inner.pool, &actual_case_id).await
}

pub(super) async fn resolve_evaluation_case(
    state: &DaemonState,
    request: ResolveEvaluationCaseRequest,
) -> Result<EvaluationCaseDetail, DaemonError> {
    let _history = state.inner.retrieval_history_lock.lock().await;
    validate_evidence_inputs(&request.evidence, request.none_matched)?;
    let evaluation_case = load_evaluation_case(&state.inner.pool, &request.case_id).await?;
    let candidates = load_candidates(&state.inner.pool, &evaluation_case.source_run_id).await?;
    let candidates_by_unit = candidates
        .iter()
        .map(|candidate| (candidate.unit_key.as_str(), candidate))
        .collect::<HashMap<_, _>>();
    let corpus_resources =
        load_corpus_resources(&state.inner.pool, &evaluation_case.corpus_id).await?;
    let resources_by_id = corpus_resources
        .iter()
        .map(|resource| (resource.resource_id.as_str(), resource))
        .collect::<HashMap<_, _>>();

    let mut prepared = Vec::with_capacity(request.evidence.len());
    for input in &request.evidence {
        let resource = resources_by_id
            .get(input.resource_id.as_str())
            .ok_or_else(|| {
                DaemonError::InvalidRequest(format!(
                    "Resource {} is not part of the frozen Evaluation Corpus",
                    input.resource_id
                ))
            })?;
        let excerpt = match input.unit_key.as_deref() {
            Some(unit_key) => {
                let candidate = candidates_by_unit.get(unit_key).ok_or_else(|| {
                    DaemonError::InvalidRequest(format!(
                        "Retrieval candidate {unit_key} is not part of the source run"
                    ))
                })?;
                if candidate.resource_id != input.resource_id {
                    return Err(DaemonError::InvalidRequest(format!(
                        "Retrieval candidate {unit_key} belongs to a different resource"
                    )));
                }
                candidate.evidence_excerpt.clone()
            }
            None => resource.preview.clone(),
        };
        prepared.push((
            evidence_id(
                &request.case_id,
                &input.resource_id,
                input.unit_key.as_deref(),
            ),
            input.clone(),
            excerpt,
        ));
    }

    let status = if request.none_matched {
        EvaluationCaseStatus::NeedsEvidence
    } else {
        EvaluationCaseStatus::Ready
    };
    let mut tx = state.inner.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE evaluation_cases
         SET status = $3,
             version = version + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE case_id = $1 AND version = $2",
    )
    .bind(&request.case_id)
    .bind(u64_to_i64(request.expected_version, "expected_version")?)
    .bind(status.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DaemonError::State {
            code: "evaluation_case_conflict",
            message: "The Evaluation Case changed before this update was applied".to_owned(),
        });
    }
    sqlx::query("DELETE FROM evaluation_evidence WHERE case_id = $1")
        .bind(&request.case_id)
        .execute(&mut *tx)
        .await?;
    for (evidence_id, input, excerpt) in prepared {
        sqlx::query(
            "INSERT INTO evaluation_evidence (
                evidence_id, case_id, resource_id, unit_key, evidence_excerpt
             ) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(evidence_id)
        .bind(&request.case_id)
        .bind(input.resource_id)
        .bind(input.unit_key)
        .bind(excerpt)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    load_evaluation_case_detail(&state.inner.pool, &request.case_id).await
}

pub(super) async fn clear_retrieval_runs(
    state: &DaemonState,
    request: ClearRetrievalRunsRequest,
) -> Result<ClearRetrievalRunsResponse, DaemonError> {
    let _history = state.inner.retrieval_history_lock.lock().await;
    let run_ids = sqlx::query_scalar::<_, String>(
        "SELECT r.run_id
         FROM retrieval_runs r
         LEFT JOIN evaluation_cases e ON e.source_run_id = r.run_id
         WHERE e.case_id IS NULL
           AND ($1 IS NULL OR r.project_id = $1)",
    )
    .bind(request.project_id.as_deref())
    .fetch_all(&state.inner.pool)
    .await?;
    delete_runs(&state.inner.pool, &run_ids).await?;
    garbage_collect_blobs(state).await?;
    Ok(ClearRetrievalRunsResponse {
        deleted_run_count: run_ids.len() as u64,
    })
}

pub(super) async fn export_evaluation_set(
    state: &DaemonState,
    request: ExportEvaluationSetRequest,
) -> Result<ExportEvaluationSetResponse, DaemonError> {
    let cases = select_evaluation_cases(&state.inner.pool, &request).await?;
    let report = benchmark_report(&state.inner.pool, &cases).await?;
    let mut fixture_cases = Vec::with_capacity(cases.len());
    for evaluation_case in cases {
        let resources =
            load_corpus_resources(&state.inner.pool, &evaluation_case.corpus_id).await?;
        let mut fixture_resources = Vec::with_capacity(resources.len());
        for resource in resources {
            let content = fs::read_to_string(corpus_blob_path(
                &state.inner.config.root_dir,
                &resource.content_hash,
            )?)?;
            fixture_resources.push(EvaluationFixtureResource {
                metadata: resource,
                content,
            });
        }
        let evidence = load_evidence(&state.inner.pool, &evaluation_case.case_id).await?;
        let source_run = load_run(&state.inner.pool, &evaluation_case.source_run_id).await?;
        let candidates = load_candidates(&state.inner.pool, &evaluation_case.source_run_id).await?;
        fixture_cases.push(EvaluationFixtureCase {
            evaluation_case,
            resources: fixture_resources,
            evidence,
            source_run,
            candidates,
        });
    }
    let exported_at: String = sqlx::query_scalar("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')")
        .fetch_one(&state.inner.pool)
        .await?;
    let fixture_json = serde_json::to_string_pretty(&EvaluationFixture {
        version: EVALUATION_FIXTURE_VERSION,
        exported_at,
        cases: fixture_cases,
    })?;
    Ok(ExportEvaluationSetResponse {
        fixture_json,
        report,
    })
}

#[derive(Clone, Debug, Serialize)]
struct RunResourceRow {
    resource_order: i64,
    resource_id: String,
    scope: String,
    kind: String,
    path: String,
    title: String,
    content_hash: String,
    content_preview: String,
    source_commit_id: Option<String>,
    draft_id: Option<String>,
    draft_revision: Option<String>,
}

async fn load_run_resource_rows(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Vec<RunResourceRow>, DaemonError> {
    let rows = sqlx::query(
        "SELECT resource_order, resource_id, scope, kind, path, title, content_hash,
                content_preview, source_commit_id, draft_id, draft_revision
         FROM retrieval_run_resources
         WHERE run_id = $1
         ORDER BY resource_order",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RunResourceRow {
                resource_order: row.try_get("resource_order")?,
                resource_id: row.try_get("resource_id")?,
                scope: row.try_get("scope")?,
                kind: row.try_get("kind")?,
                path: row.try_get("path")?,
                title: row.try_get("title")?,
                content_hash: row.try_get("content_hash")?,
                content_preview: row.try_get("content_preview")?,
                source_commit_id: row.try_get("source_commit_id")?,
                draft_id: row.try_get("draft_id")?,
                draft_revision: row.try_get("draft_revision")?,
            })
        })
        .collect()
}

async fn load_run(pool: &SqlitePool, run_id: &str) -> Result<RetrievalRun, DaemonError> {
    let row = sqlx::query(
        "SELECT r.*, e.case_id AS evaluation_case_id,
                e.status AS evaluation_case_status
         FROM retrieval_runs r
         LEFT JOIN evaluation_cases e ON e.source_run_id = r.run_id
         WHERE r.run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("Retrieval Run {run_id}")))?;
    run_from_row(row)
}

async fn load_evaluation_case(
    pool: &SqlitePool,
    case_id: &str,
) -> Result<EvaluationCase, DaemonError> {
    let row = sqlx::query(
        "SELECT case_id, source_run_id, corpus_id, project_id, query,
                status, version, created_at, updated_at
         FROM evaluation_cases
         WHERE case_id = $1",
    )
    .bind(case_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DaemonError::NotFound(format!("Evaluation Case {case_id}")))?;
    Ok(EvaluationCase {
        case_id: row.try_get("case_id")?,
        source_run_id: row.try_get("source_run_id")?,
        corpus_id: row.try_get("corpus_id")?,
        project_id: row.try_get("project_id")?,
        query: row.try_get("query")?,
        status: EvaluationCaseStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
        version: non_negative_u64(row.try_get("version")?, "version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_evidence(
    pool: &SqlitePool,
    case_id: &str,
) -> Result<Vec<EvaluationEvidence>, DaemonError> {
    let rows = sqlx::query(
        "SELECT evidence_id, case_id, resource_id, unit_key, evidence_excerpt
         FROM evaluation_evidence
         WHERE case_id = $1
         ORDER BY resource_id, unit_key",
    )
    .bind(case_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EvaluationEvidence {
                evidence_id: row.try_get("evidence_id")?,
                case_id: row.try_get("case_id")?,
                resource_id: row.try_get("resource_id")?,
                unit_key: row.try_get("unit_key")?,
                evidence_excerpt: row.try_get("evidence_excerpt")?,
            })
        })
        .collect()
}

async fn load_corpus_resources(
    pool: &SqlitePool,
    corpus_id: &str,
) -> Result<Vec<EvaluationCorpusResource>, DaemonError> {
    let rows = sqlx::query(
        "SELECT resource_id, scope, kind, path, title, content_hash,
                content_preview, source_commit_id, draft_id, draft_revision
         FROM evaluation_corpus_resources
         WHERE corpus_id = $1
         ORDER BY resource_order",
    )
    .bind(corpus_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EvaluationCorpusResource {
                resource_id: row.try_get("resource_id")?,
                scope: parse_scope(row.try_get::<String, _>("scope")?.as_str())?,
                kind: parse_kind(row.try_get::<String, _>("kind")?.as_str())?,
                path: row.try_get("path")?,
                title: row.try_get("title")?,
                content_hash: row.try_get("content_hash")?,
                source_commit_id: row.try_get("source_commit_id")?,
                draft_id: row.try_get("draft_id")?,
                draft_revision: row.try_get("draft_revision")?,
                preview: row.try_get("content_preview")?,
            })
        })
        .collect()
}

async fn load_evaluation_case_detail(
    pool: &SqlitePool,
    case_id: &str,
) -> Result<EvaluationCaseDetail, DaemonError> {
    let evaluation_case = load_evaluation_case(pool, case_id).await?;
    let evidence = load_evidence(pool, case_id).await?;
    let candidates = load_candidates(pool, &evaluation_case.source_run_id).await?;
    let report = if evaluation_case.status == EvaluationCaseStatus::Ready {
        Some(benchmark_report(pool, std::slice::from_ref(&evaluation_case)).await?)
    } else {
        None
    };
    Ok(EvaluationCaseDetail {
        evaluation_case,
        evidence,
        evidence_suggestions: suggest_evidence(&candidates),
        report,
    })
}

async fn select_evaluation_cases(
    pool: &SqlitePool,
    request: &ExportEvaluationSetRequest,
) -> Result<Vec<EvaluationCase>, DaemonError> {
    let rows = sqlx::query(
        "SELECT case_id
         FROM evaluation_cases
         WHERE status = 'ready'
           AND ($1 IS NULL OR project_id = $1)
         ORDER BY created_at, case_id",
    )
    .bind(request.project_id.as_deref())
    .fetch_all(pool)
    .await?;
    let requested = request.case_ids.iter().collect::<BTreeSet<_>>();
    let mut cases = Vec::new();
    for row in rows {
        let case_id: String = row.try_get("case_id")?;
        if requested.is_empty() || requested.contains(&case_id) {
            cases.push(load_evaluation_case(pool, &case_id).await?);
        }
    }
    if !requested.is_empty() && cases.len() != requested.len() {
        return Err(DaemonError::InvalidRequest(
            "One or more requested Evaluation Cases do not exist or do not match the Project"
                .to_owned(),
        ));
    }
    Ok(cases)
}

async fn benchmark_report(
    pool: &SqlitePool,
    cases: &[EvaluationCase],
) -> Result<RetrievalBenchmarkReport, DaemonError> {
    let mut variants = BTreeMap::new();
    for variant in [
        RetrievalBenchmarkVariant::Bm25,
        RetrievalBenchmarkVariant::DenseVector,
        RetrievalBenchmarkVariant::HybridRrf,
        RetrievalBenchmarkVariant::Reranked,
    ] {
        let mut accumulators = Vec::new();
        let mut latencies = Vec::new();
        for evaluation_case in cases {
            let evidence = load_evidence(pool, &evaluation_case.case_id).await?;
            let candidates = load_candidates(pool, &evaluation_case.source_run_id).await?;
            let corpus_resources = load_corpus_resources(pool, &evaluation_case.corpus_id).await?;
            let run = load_run(pool, &evaluation_case.source_run_id).await?;
            accumulators.push(case_metrics(
                variant,
                &candidates,
                &evidence,
                &corpus_resources,
            ));
            latencies.push(variant_latency(variant, &run.latencies));
        }
        variants.insert(
            variant.as_str().to_owned(),
            aggregate_metrics(&accumulators, &latencies),
        );
    }
    Ok(RetrievalBenchmarkReport { variants })
}

#[derive(Clone, Debug, Default)]
struct CaseMetrics {
    recall_at_20: f64,
    ndcg_at_10: f64,
    mrr: f64,
    resource_diversity: f64,
    scope_violation: f64,
    stale_result: f64,
}

fn case_metrics(
    variant: RetrievalBenchmarkVariant,
    candidates: &[RetrievalCandidate],
    evidence: &[EvaluationEvidence],
    corpus_resources: &[EvaluationCorpusResource],
) -> CaseMetrics {
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| candidate_rank(variant, candidate).map(|rank| (rank, candidate)))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.unit_key.cmp(&right.1.unit_key))
    });
    let relevant = evidence.iter().collect::<Vec<_>>();
    let relevant_count = relevant.len();
    let mut matched = BTreeSet::<String>::new();
    for (_, candidate) in ranked.iter().take(20) {
        if let Some(evidence) = best_matching_evidence(candidate, &relevant, &matched) {
            matched.insert(evidence.evidence_id.clone());
        }
    }
    let recall_at_20 = if relevant_count == 0 {
        0.0
    } else {
        matched.len() as f64 / relevant_count as f64
    };

    let mut seen = BTreeSet::<String>::new();
    let mut dcg = 0.0;
    let mut first_relevant_rank = None;
    for (index, (_, candidate)) in ranked.iter().take(10).enumerate() {
        if let Some(evidence) = best_matching_evidence(candidate, &relevant, &seen) {
            seen.insert(evidence.evidence_id.clone());
            dcg += 1.0 / ((index + 2) as f64).log2();
            first_relevant_rank.get_or_insert(index + 1);
        }
    }
    let idcg = relevant
        .iter()
        .take(10)
        .enumerate()
        .map(|(index, _)| 1.0 / ((index + 2) as f64).log2())
        .sum::<f64>();
    let ndcg_at_10 = if idcg == 0.0 { 0.0 } else { dcg / idcg };
    let mrr = first_relevant_rank
        .map(|rank| 1.0 / rank as f64)
        .unwrap_or_default();
    let top = ranked.iter().take(20).collect::<Vec<_>>();
    let unique_resources = top
        .iter()
        .map(|(_, candidate)| candidate.resource_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let resource_diversity = if top.is_empty() {
        0.0
    } else {
        unique_resources as f64 / top.len() as f64
    };
    let corpus_by_id = corpus_resources
        .iter()
        .map(|resource| {
            (
                resource.resource_id.as_str(),
                resource.content_hash.as_str(),
            )
        })
        .collect::<HashMap<_, _>>();
    let scope_violations = top
        .iter()
        .filter(|(_, candidate)| !corpus_by_id.contains_key(candidate.resource_id.as_str()))
        .count();
    let stale_results = top
        .iter()
        .filter(|(_, candidate)| {
            corpus_by_id
                .get(candidate.resource_id.as_str())
                .is_some_and(|content_hash| **content_hash != candidate.resource_content_hash)
        })
        .count();
    let scope_violation = if top.is_empty() {
        0.0
    } else {
        scope_violations as f64 / top.len() as f64
    };
    let stale_result = if top.is_empty() {
        0.0
    } else {
        stale_results as f64 / top.len() as f64
    };
    CaseMetrics {
        recall_at_20,
        ndcg_at_10,
        mrr,
        resource_diversity,
        scope_violation,
        stale_result,
    }
}

fn best_matching_evidence<'a>(
    candidate: &RetrievalCandidate,
    evidence: &[&'a EvaluationEvidence],
    seen: &BTreeSet<String>,
) -> Option<&'a EvaluationEvidence> {
    evidence.iter().copied().find(|evidence| {
        !seen.contains(&evidence.evidence_id)
            && evidence.resource_id == candidate.resource_id
            && evidence
                .unit_key
                .as_deref()
                .is_none_or(|unit_key| unit_key == candidate.unit_key)
    })
}

fn candidate_rank(
    variant: RetrievalBenchmarkVariant,
    candidate: &RetrievalCandidate,
) -> Option<u64> {
    match variant {
        RetrievalBenchmarkVariant::Bm25 => candidate.exact_rank.or(candidate.bm25_rank),
        RetrievalBenchmarkVariant::DenseVector => candidate.vector_rank,
        RetrievalBenchmarkVariant::HybridRrf => candidate.rrf_rank,
        RetrievalBenchmarkVariant::Reranked => candidate.reranker_rank,
    }
}

fn suggest_evidence(candidates: &[RetrievalCandidate]) -> Vec<EvaluationEvidenceSuggestion> {
    let mut ranked = candidates
        .iter()
        .filter(|candidate| !candidate.selected)
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        suggestion_rank(left)
            .cmp(&suggestion_rank(right))
            .then_with(|| left.unit_key.cmp(&right.unit_key))
    });

    let mut resources = BTreeSet::new();
    ranked
        .into_iter()
        .filter(|candidate| resources.insert(candidate.resource_id.as_str()))
        .take(EVALUATION_SUGGESTION_LIMIT)
        .map(|candidate| EvaluationEvidenceSuggestion {
            resource_id: candidate.resource_id.clone(),
            unit_key: candidate.unit_key.clone(),
            path: candidate.path.clone(),
            heading_path: candidate.heading_path.clone(),
            evidence_excerpt: candidate.evidence_excerpt.clone(),
            model_relevance: candidate.reranker_relevance,
            likely_failure_stage: likely_failure_stage(candidate),
            exclusion_reason: candidate.exclusion_reason,
        })
        .collect()
}

fn suggestion_rank(candidate: &RetrievalCandidate) -> (u8, u64, u64, u64, u64) {
    (
        u8::from(candidate.reranker_rank.is_none()),
        candidate.reranker_rank.unwrap_or(u64::MAX),
        candidate.rrf_rank.unwrap_or(u64::MAX),
        candidate
            .bm25_rank
            .or(candidate.exact_rank)
            .unwrap_or(u64::MAX),
        candidate.vector_rank.unwrap_or(u64::MAX),
    )
}

fn likely_failure_stage(candidate: &RetrievalCandidate) -> RetrievalFailureStage {
    if candidate.rrf_rank.is_none() {
        return RetrievalFailureStage::Fusion;
    }
    match candidate.exclusion_reason {
        RetrievalExclusionReason::BelowRelevance | RetrievalExclusionReason::NotReranked => {
            RetrievalFailureStage::Reranking
        }
        RetrievalExclusionReason::Overlap
        | RetrievalExclusionReason::PerResourceLimit
        | RetrievalExclusionReason::TokenBudget
        | RetrievalExclusionReason::FragmentLimit
        | RetrievalExclusionReason::Selected => RetrievalFailureStage::Assembly,
    }
}

fn variant_latency(variant: RetrievalBenchmarkVariant, latencies: &RetrievalStageLatencies) -> u64 {
    let common = latencies
        .effective_memory_us
        .saturating_add(latencies.index_ensure_us);
    match variant {
        RetrievalBenchmarkVariant::Bm25 => common.saturating_add(latencies.bm25_us),
        RetrievalBenchmarkVariant::DenseVector => common
            .saturating_add(latencies.embedding_us)
            .saturating_add(latencies.vector_us),
        RetrievalBenchmarkVariant::HybridRrf => common
            .saturating_add(latencies.bm25_us)
            .saturating_add(latencies.embedding_us)
            .saturating_add(latencies.vector_us)
            .saturating_add(latencies.rrf_us),
        RetrievalBenchmarkVariant::Reranked => latencies.total_us,
    }
}

fn aggregate_metrics(cases: &[CaseMetrics], latencies: &[u64]) -> RetrievalBenchmarkMetrics {
    if cases.is_empty() {
        return RetrievalBenchmarkMetrics::default();
    }
    let divisor = cases.len() as f64;
    let mut sorted_latencies = latencies.to_vec();
    sorted_latencies.sort_unstable();
    RetrievalBenchmarkMetrics {
        case_count: cases.len() as u64,
        recall_at_20: cases.iter().map(|value| value.recall_at_20).sum::<f64>() / divisor,
        ndcg_at_10: cases.iter().map(|value| value.ndcg_at_10).sum::<f64>() / divisor,
        mrr: cases.iter().map(|value| value.mrr).sum::<f64>() / divisor,
        resource_diversity: cases
            .iter()
            .map(|value| value.resource_diversity)
            .sum::<f64>()
            / divisor,
        scope_violation: cases.iter().map(|value| value.scope_violation).sum::<f64>() / divisor,
        stale_result: cases.iter().map(|value| value.stale_result).sum::<f64>() / divisor,
        warm_p50_us: percentile(&sorted_latencies, 0.50),
        warm_p95_us: percentile(&sorted_latencies, 0.95),
    }
}

fn percentile(sorted: &[u64], quantile: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * quantile).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn validate_evidence_inputs(
    inputs: &[EvaluationEvidenceInput],
    none_matched: bool,
) -> Result<(), DaemonError> {
    if none_matched != inputs.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "Resolve an Evaluation Case with exactly one of confirmed evidence or none_matched"
                .to_owned(),
        ));
    }
    let mut identities = BTreeSet::new();
    for input in inputs {
        if input.resource_id.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "Evaluation evidence resource_id must not be empty".to_owned(),
            ));
        }
        if input
            .unit_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(DaemonError::InvalidRequest(
                "Evaluation evidence unit_key must not be empty".to_owned(),
            ));
        }
        let identity = (input.resource_id.as_str(), input.unit_key.as_deref());
        if !identities.insert(identity) {
            return Err(DaemonError::InvalidRequest(
                "Evaluation evidence contains a duplicate identity".to_owned(),
            ));
        }
    }
    Ok(())
}

fn corpus_id(effective_hash: &str, resources: &[RunResourceRow]) -> Result<String, DaemonError> {
    let manifest = serde_json::to_vec(&(effective_hash, resources))?;
    Ok(format!("corpus_{}", hex::encode(Sha256::digest(&manifest))))
}

fn evidence_id(case_id: &str, resource_id: &str, unit_key: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    for value in [case_id, resource_id, unit_key.unwrap_or_default()] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("evidence_{}", hex::encode(hasher.finalize()))
}

async fn prune_runs(state: &DaemonState, project_id: &str) -> Result<(), DaemonError> {
    let stale = sqlx::query_scalar::<_, String>(
        "SELECT r.run_id
         FROM retrieval_runs r
         LEFT JOIN evaluation_cases e ON e.source_run_id = r.run_id
         WHERE r.project_id = $1 AND e.case_id IS NULL
         ORDER BY r.created_at DESC, r.run_id DESC
         LIMIT -1 OFFSET $2",
    )
    .bind(project_id)
    .bind(RETRIEVAL_RUN_RETENTION_PER_PROJECT)
    .fetch_all(&state.inner.pool)
    .await?;
    delete_runs(&state.inner.pool, &stale).await?;
    garbage_collect_blobs(state).await
}

async fn delete_runs(pool: &SqlitePool, run_ids: &[String]) -> Result<(), DaemonError> {
    if run_ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for run_id in run_ids {
        sqlx::query("DELETE FROM retrieval_run_candidates WHERE run_id = $1")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM retrieval_run_resources WHERE run_id = $1")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM retrieval_runs
             WHERE run_id = $1
               AND NOT EXISTS (
                   SELECT 1 FROM evaluation_cases WHERE source_run_id = $1
               )",
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn garbage_collect_blobs(state: &DaemonState) -> Result<(), DaemonError> {
    let referenced = sqlx::query_scalar::<_, String>(
        "SELECT content_hash FROM retrieval_run_resources
         UNION
         SELECT content_hash FROM evaluation_corpus_resources",
    )
    .fetch_all(&state.inner.pool)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let stored = sqlx::query_scalar::<_, String>("SELECT content_hash FROM retrieval_corpus_blobs")
        .fetch_all(&state.inner.pool)
        .await?;
    for content_hash in stored {
        if referenced.contains(&content_hash) {
            continue;
        }
        let path = corpus_blob_path(&state.inner.config.root_dir, &content_hash)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        sqlx::query("DELETE FROM retrieval_corpus_blobs WHERE content_hash = $1")
            .bind(content_hash)
            .execute(&state.inner.pool)
            .await?;
    }
    Ok(())
}

fn corpus_blob_path(root: &Path, content_hash: &str) -> Result<PathBuf, DaemonError> {
    let digest = content_hash.strip_prefix("sha256:").ok_or_else(|| {
        history_corrupt(format!(
            "Evaluation corpus content hash is not SHA-256: {content_hash}"
        ))
    })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(history_corrupt(format!(
            "Evaluation corpus content hash is invalid: {content_hash}"
        )));
    }
    Ok(root
        .join("evaluation-corpora")
        .join("blobs")
        .join(&digest[..2])
        .join(digest))
}

fn content_hash(content: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(content.as_bytes())))
}

fn truncate_excerpt(value: &str) -> String {
    let mut excerpt = value
        .chars()
        .take(RETRIEVAL_EXCERPT_CHARS)
        .collect::<String>();
    if value.chars().count() > RETRIEVAL_EXCERPT_CHARS {
        excerpt.push('…');
    }
    excerpt
}

pub(crate) fn excerpt_is_truncated(value: &str) -> bool {
    value.chars().count() > RETRIEVAL_EXCERPT_CHARS
}

fn encode_cursor(cursor: &RetrievalCursor) -> Result<String, DaemonError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor)?))
}

fn decode_cursor(cursor: &str) -> Result<RetrievalCursor, DaemonError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| DaemonError::InvalidRequest("Invalid Retrieval Run cursor".to_owned()))?;
    serde_json::from_slice(&decoded)
        .map_err(|_| DaemonError::InvalidRequest("Invalid Retrieval Run cursor".to_owned()))
}

fn parse_scope(value: &str) -> Result<SourceScope, DaemonError> {
    match value {
        "org" => Ok(SourceScope::Org),
        "project" => Ok(SourceScope::Project),
        _ => Err(history_corrupt(format!(
            "Unknown Retrieval Run scope: {value}"
        ))),
    }
}

fn parse_kind(value: &str) -> Result<MemoryKind, DaemonError> {
    match value {
        // Legacy kind values from archived history stay readable; the
        // unified runtime only records 'memory'.
        "context" | "rule" | "workflow" | "memory" => Ok(MemoryKind::Memory),
        _ => Err(history_corrupt(format!(
            "Unknown Retrieval Run memory kind: {value}"
        ))),
    }
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn usize_to_i64(value: usize, field: &str) -> Result<i64, DaemonError> {
    i64::try_from(value)
        .map_err(|_| DaemonError::InvalidRequest(format!("{field} exceeds the supported range")))
}

fn optional_usize_to_i64(value: Option<usize>, field: &str) -> Result<Option<i64>, DaemonError> {
    value.map(|value| usize_to_i64(value, field)).transpose()
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, DaemonError> {
    i64::try_from(value)
        .map_err(|_| DaemonError::InvalidRequest(format!("{field} exceeds the supported range")))
}

fn non_negative_u64(value: i64, field: &str) -> Result<u64, DaemonError> {
    u64::try_from(value).map_err(|_| history_corrupt(format!("{field} is negative")))
}

fn optional_non_negative_u64(value: Option<i64>, field: &str) -> Result<Option<u64>, DaemonError> {
    value
        .map(|value| non_negative_u64(value, field))
        .transpose()
}

fn history_corrupt(message: impl Into<String>) -> DaemonError {
    DaemonError::State {
        code: "retrieval_history_corrupt",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::{
        CredentialStore, CredentialStoreError, DaemonConfig, GetRecallFragmentRequest,
        ServerCredentials,
    };

    struct NoCredentials;

    impl CredentialStore for NoCredentials {
        fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
            Ok(None)
        }

        fn replace(&self, _credentials: &ServerCredentials) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn activity_and_fragment_endpoint_use_the_selected_frozen_run() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = DaemonConfig::for_root(temp.path().join("daemon"));
        config.project.server_url = "https://clumsies.test".to_owned();
        let codex_sessions = temp.path().join("codex/sessions");
        config.codex_home = Some(temp.path().join("codex"));
        std::fs::create_dir_all(&codex_sessions).unwrap();
        let state = DaemonState::initialize_with_credential_store(config, Arc::new(NoCredentials))
            .await
            .unwrap();

        let workspace = temp.path().join("workspace");
        let nested = workspace.join("nested");
        let child = nested.join("child");
        std::fs::create_dir_all(&child).unwrap();
        for (root, project_id) in [(&workspace, "prj_parent"), (&nested, "prj_nested")] {
            sqlx::query(
                "INSERT INTO project_bindings
                 (server_url, workspace_root, project_id, revision)
                 VALUES ('https://clumsies.test', $1, $2, 1)",
            )
            .bind(root.display().to_string())
            .bind(project_id)
            .execute(&state.inner.pool)
            .await
            .unwrap();
        }

        let frozen = "旧前言\n完整的历史召回 chunk\n旧尾声";
        let expected = "完整的历史召回 chunk";
        let start = frozen.find(expected).unwrap();
        let frozen_hash = content_hash(frozen);
        let run_id = start_run(&state, "prj_nested", "历史查询", "fingerprint")
            .await
            .unwrap();
        let candidate =
            |unit_key: &str, selected: bool, start_byte: usize, end_byte: usize, text: &str| {
                RetrievalCandidateInput {
                    unit_key: unit_key.to_owned(),
                    resource_id: "memory-1".to_owned(),
                    scope: SourceScope::Project,
                    kind: MemoryKind::Memory,
                    path: "memory/history.md".to_owned(),
                    heading_path: vec!["历史".to_owned()],
                    locator: SourceLocator::MarkdownSpan {
                        start_byte,
                        end_byte,
                        heading_path: vec!["历史".to_owned()],
                    },
                    content_hash: content_hash(text),
                    resource_content_hash: frozen_hash.clone(),
                    token_count: 5,
                    evidence_excerpt: text.to_owned(),
                    exact_rank: None,
                    bm25_rank: Some(1),
                    bm25_score: Some(1.0),
                    vector_rank: None,
                    vector_score: None,
                    rrf_rank: Some(1),
                    rrf_score: Some(1.0),
                    reranker_rank: Some(1),
                    reranker_logit: Some(1.0),
                    reranker_relevance: Some(1.0),
                    final_rank: selected.then_some(1),
                    exclusion_reason: if selected {
                        RetrievalExclusionReason::Selected
                    } else {
                        RetrievalExclusionReason::NotReranked
                    },
                    delta_action: selected.then_some(RetrievalDeltaAction::Add),
                }
            };
        finish_run(
            &state,
            &run_id,
            RetrievalRunCompletion {
                resources: vec![RetrievalCorpusResourceInput {
                    resource_id: "memory-1".to_owned(),
                    scope: SourceScope::Project,
                    kind: MemoryKind::Memory,
                    path: "memory/history.md".to_owned(),
                    title: "History".to_owned(),
                    content: frozen.to_owned(),
                    content_hash: frozen_hash.clone(),
                    source_commit_id: None,
                    draft_id: None,
                    draft_revision: None,
                }],
                candidates: vec![
                    candidate(
                        "memory-1/history/0/0",
                        true,
                        start,
                        start + expected.len(),
                        expected,
                    ),
                    candidate("memory-1/unselected/0/0", false, 0, 9, "旧前言\n"),
                ],
                unit_count: 2,
                returned_fragment_count: 1,
                latencies: RetrievalStageLatencies {
                    total_us: 123_456,
                    ..RetrievalStageLatencies::default()
                },
                ..RetrievalRunCompletion::default()
            },
        )
        .await
        .unwrap();

        let running_id = start_run(&state, "prj_nested", "历史查询", "fingerprint")
            .await
            .unwrap();
        let mut log = vec![
            serde_json::json!({
                "type": "session_meta", "payload": { "id": "activity", "cwd": child }
            }),
            serde_json::json!({
                "type": "event_msg", "payload": {
                    "type": "user_message", "message": "Show the historical recall"
                }
            }),
        ];
        for id in [&run_id, &running_id] {
            log.push(serde_json::json!({
                "type": "event_msg", "payload": {
                    "type": "item_completed", "item": {
                        "type": "McpToolCall", "id": id, "server": "clumsies", "tool": "memory",
                        "arguments": { "op": { "activate": { "query": "历史查询" } } },
                        "result": { "structuredContent": {
                            "run_id": id,
                            "fragments": [{
                                "action": "add", "unit_key": "memory-1/history/0/0",
                                "resource_id": "memory-1", "path": "memory/history.md",
                                "content": "Recorded tool preview"
                            }]
                        } }
                    }
                }
            }));
        }
        std::fs::write(
            codex_sessions.join("rollout-activity.jsonl"),
            log.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let request = crate::recall::ListRecallsRequest {
            workspace_root: None,
            project_id: Some("prj_nested".to_owned()),
            limit: None,
            cursor: None,
        };
        let activity = crate::recall::list_recalls(&state, request.clone())
            .await
            .unwrap();
        assert_eq!(activity.sessions.len(), 1);
        let detail = crate::recall::get_recall_session(
            &state,
            crate::GetRecallSessionRequest {
                session_token: activity.sessions[0].session_token.clone(),
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
        let activations = &detail.session.tasks[0].activations;
        assert_eq!(activations.len(), 2);
        assert_eq!(activations[0].run_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(activations[0].total_us, Some(123_456));
        assert_eq!(activations[0].fragments[0].content, expected);
        assert_eq!(activations[1].run_status.as_deref(), Some("running"));
        assert_eq!(activations[1].total_us, None);

        let response = crate::recall::get_recall_fragment(
            &state,
            GetRecallFragmentRequest {
                workspace_root: child.display().to_string(),
                run_id: run_id.clone(),
                unit_key: "memory-1/history/0/0".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(response.fragment.content, expected);
        assert_eq!(response.fragment.unit_key, "memory-1/history/0/0");
        assert!(!response.fragment.truncated);

        assert!(matches!(
            load_recall_fragment_content(&state, "prj_parent", &run_id, "memory-1/history/0/0")
                .await,
            Err(DaemonError::NotFound(_))
        ));
        assert!(matches!(
            load_recall_fragment_content(&state, "prj_nested", &run_id, "memory-1/unselected/0/0")
                .await,
            Err(DaemonError::InvalidRequest(_))
        ));

        let bad_hash = format!("sha256:{}", "0".repeat(64));
        sqlx::query(
            "UPDATE retrieval_run_candidates SET resource_content_hash = $3
             WHERE run_id = $1 AND unit_key = $2",
        )
        .bind(&run_id)
        .bind("memory-1/history/0/0")
        .bind(bad_hash)
        .execute(&state.inner.pool)
        .await
        .unwrap();
        assert!(matches!(
            load_recall_fragment_content(&state, "prj_nested", &run_id, "memory-1/history/0/0")
                .await,
            Err(DaemonError::State {
                code: "retrieval_history_corrupt",
                ..
            })
        ));

        clear_retrieval_runs(
            &state,
            ClearRetrievalRunsRequest {
                project_id: Some("prj_nested".to_owned()),
            },
        )
        .await
        .unwrap();
        let cleared = crate::recall::list_recalls(&state, request).await.unwrap();
        let detail = crate::recall::get_recall_session(
            &state,
            crate::GetRecallSessionRequest {
                session_token: cleared.sessions[0].session_token.clone(),
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();
        let activation = &detail.session.tasks[0].activations[0];
        assert_eq!(activation.run_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(activation.total_us, None);
        assert_eq!(activation.fragments[0].content, "Recorded tool preview");
    }

    #[test]
    fn recall_fragment_uses_the_frozen_utf8_locator() {
        let frozen = "old header\n完整的历史 chunk\nold footer";
        let expected = "完整的历史 chunk";
        let start = frozen.find(expected).unwrap();
        let mut candidate = candidate("memory-1", &content_hash(frozen));
        candidate.unit_key = "memory-1/history/0/0".to_owned();
        candidate.locator = SourceLocator::MarkdownSpan {
            start_byte: start,
            end_byte: start + expected.len(),
            heading_path: vec!["History".to_owned()],
        };
        candidate.content_hash = content_hash(expected);

        assert_eq!(
            candidate_content(&candidate, frozen).unwrap(),
            (expected.to_owned(), false)
        );
    }

    #[test]
    fn description_fragment_explicitly_falls_back_to_its_historical_excerpt() {
        let description = "d".repeat(RETRIEVAL_EXCERPT_CHARS + 20);
        let mut candidate = candidate("memory-1", "sha256:unused");
        candidate.unit_key = "memory-1/description".to_owned();
        candidate.locator = SourceLocator::MarkdownSpan {
            start_byte: 0,
            end_byte: 0,
            heading_path: Vec::new(),
        };
        candidate.evidence_excerpt = truncate_excerpt(&description);

        let (content, truncated) = candidate_content(&candidate, "resource body").unwrap();
        assert_eq!(content, truncate_excerpt(&description));
        assert!(truncated);
    }

    #[test]
    fn recall_fragment_rejects_an_invalid_locator() {
        let mut candidate = candidate("memory-1", "sha256:unused");
        candidate.unit_key = "memory-1/body/0/0".to_owned();
        candidate.locator = SourceLocator::MarkdownSpan {
            start_byte: 1,
            end_byte: 999,
            heading_path: Vec::new(),
        };

        assert!(matches!(
            candidate_content(&candidate, "short"),
            Err(DaemonError::State {
                code: "retrieval_history_corrupt",
                ..
            })
        ));
    }

    #[test]
    fn scope_violation_and_stale_result_have_independent_semantics() {
        let corpus = vec![EvaluationCorpusResource {
            resource_id: "context-1".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: "context/one.md".to_owned(),
            title: "One".to_owned(),
            content_hash: "sha256:current".to_owned(),
            source_commit_id: Some("commit-1".to_owned()),
            draft_id: None,
            draft_revision: None,
            preview: "Current content".to_owned(),
        }];

        let current = case_metrics(
            RetrievalBenchmarkVariant::Bm25,
            &[candidate("context-1", "sha256:current")],
            &[],
            &corpus,
        );
        assert_eq!(current.scope_violation, 0.0);
        assert_eq!(current.stale_result, 0.0);

        let stale = case_metrics(
            RetrievalBenchmarkVariant::Bm25,
            &[candidate("context-1", "sha256:old")],
            &[],
            &corpus,
        );
        assert_eq!(stale.scope_violation, 0.0);
        assert_eq!(stale.stale_result, 1.0);

        let out_of_scope = case_metrics(
            RetrievalBenchmarkVariant::Bm25,
            &[candidate("context-outside", "sha256:current")],
            &[],
            &corpus,
        );
        assert_eq!(out_of_scope.scope_violation, 1.0);
        assert_eq!(out_of_scope.stale_result, 0.0);
    }

    fn candidate(resource_id: &str, resource_content_hash: &str) -> RetrievalCandidate {
        RetrievalCandidate {
            unit_key: format!("{resource_id}:unit"),
            resource_id: resource_id.to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: format!("context/{resource_id}.md"),
            heading_path: Vec::new(),
            locator: SourceLocator::MarkdownSpan {
                start_byte: 0,
                end_byte: 7,
                heading_path: Vec::new(),
            },
            content_hash: "sha256:unit".to_owned(),
            resource_content_hash: resource_content_hash.to_owned(),
            token_count: 1,
            evidence_excerpt: "content".to_owned(),
            exact_rank: None,
            bm25_rank: Some(1),
            bm25_score: Some(1.0),
            vector_rank: None,
            vector_score: None,
            rrf_rank: None,
            rrf_score: None,
            reranker_rank: None,
            reranker_logit: None,
            reranker_relevance: None,
            final_rank: None,
            selected: false,
            exclusion_reason: RetrievalExclusionReason::NotReranked,
            delta_action: None,
        }
    }
}
