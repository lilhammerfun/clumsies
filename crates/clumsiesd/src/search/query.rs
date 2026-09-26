use std::collections::{BTreeSet, HashMap, HashSet};

use sqlx::{Row, SqlitePool};

use super::activation::{ActivationStateToken, activation_response};
use super::index::decode_vector;
use super::{
    ActivateMemoryResponse, DaemonError, MIN_RERANK_RELEVANCE, MemoryKind, RetrievalCandidateInput,
    RetrievalDeltaAction, RetrievalExclusionReason, RetrievalRunCompletion, SearchFailure,
    SourceLocator, SourceScope, elapsed_us, parse_memory_kind, parse_source_scope,
};

pub(super) const BM25_TOP_K: usize = 60;
pub(super) const VECTOR_TOP_K: usize = 60;
const RRF_CONSTANT: f32 = 60.0;
const RRF_CANDIDATES: usize = 40;
const RERANK_CANDIDATES: usize = 24;
pub(super) const FINAL_FRAGMENTS: usize = 12;
pub(super) const PER_RESOURCE_LIMIT: usize = 2;
const FRAGMENT_TOKEN_BUDGET: usize = 2400;

#[derive(Clone, Debug)]
pub(super) struct IndexRow {
    pub(super) rowid: i64,
    pub(super) unit_key: String,
    pub(super) resource_id: String,
    pub(super) scope: SourceScope,
    pub(super) kind: MemoryKind,
    pub(super) path: String,
    pub(super) title: String,
    pub(super) heading_path: Vec<String>,
    pub(super) locator: SourceLocator,
    pub(super) text: String,
    pub(super) text_hash: String,
    pub(super) resource_content_hash: String,
    pub(super) token_count: usize,
    pub(super) vector: Vec<f32>,
}

#[derive(Clone, Debug)]
pub(super) struct RankedRow {
    pub(super) row: IndexRow,
    pub(super) exact_rank: Option<usize>,
    pub(super) bm25_rank: Option<usize>,
    pub(super) bm25_score: Option<f32>,
    pub(super) vector_rank: Option<usize>,
    pub(super) vector_score: Option<f32>,
    pub(super) rrf_rank: usize,
    pub(super) rrf_score: f32,
    pub(super) reranker_rank: Option<usize>,
    pub(super) rerank_score: Option<f32>,
    pub(super) final_rank: Option<usize>,
    pub(super) exclusion_reason: RetrievalExclusionReason,
    pub(super) delta_action: Option<RetrievalDeltaAction>,
}

#[derive(Clone, Debug)]
pub(super) struct LexicalRank {
    pub(super) rowid: i64,
    pub(super) exact_rank: Option<usize>,
    pub(super) bm25_rank: Option<usize>,
    pub(super) bm25_score: Option<f32>,
}

#[derive(Clone, Debug)]
pub(super) struct VectorRank {
    pub(super) rowid: i64,
    pub(super) rank: usize,
    pub(super) score: f32,
}

pub(super) async fn query_index(
    state: &super::DaemonState,
    pool: &SqlitePool,
    revision_id: &str,
    query: &str,
    previous_state: ActivationStateToken,
    completion: &mut RetrievalRunCompletion,
    failure_stage: &mut &str,
) -> Result<ActivateMemoryResponse, DaemonError> {
    let rows = fetch_index_rows(pool, revision_id, state.inner.search_models.dimensions()).await?;
    completion.unit_count = rows.len();
    completion.model_revision =
        sqlx::query_scalar("SELECT model_revision FROM search_revisions WHERE revision_id = $1")
            .bind(revision_id)
            .fetch_optional(pool)
            .await?;
    if rows.is_empty() {
        *failure_stage = "assembly";
        let started = std::time::Instant::now();
        let response = activation_response(revision_id, &mut [], &rows, previous_state)?;
        completion.latencies.assembly_us = elapsed_us(started);
        return Ok(response);
    }

    *failure_stage = "bm25";
    let started = std::time::Instant::now();
    let lexical = lexical_ranks(pool, revision_id, query, &rows).await?;
    completion.latencies.bm25_us = elapsed_us(started);

    *failure_stage = "embedding";
    let started = std::time::Instant::now();
    let query_owned = query.to_owned();
    let models = state.inner.search_models.clone();
    let query_vector =
        super::run_model_work(state, move || models.embed_query(&query_owned)).await?;
    completion.latencies.embedding_us = elapsed_us(started);
    if query_vector.len() != state.inner.search_models.dimensions()
        || !super::index::valid_normalized_vector(&query_vector)
    {
        return Err(SearchFailure::vector("query embedding is corrupt").into());
    }

    *failure_stage = "vector";
    let started = std::time::Instant::now();
    let vector = vector_ranks(&rows, &query_vector);
    completion.latencies.vector_us = elapsed_us(started);

    *failure_stage = "rrf";
    let started = std::time::Instant::now();
    let mut candidates = rrf_candidates(&rows, &lexical, &vector);
    completion.latencies.rrf_us = elapsed_us(started);
    completion.candidates = candidates.iter().map(retrieval_candidate_input).collect();

    let rerank_count = candidates.len().min(RRF_CANDIDATES).min(RERANK_CANDIDATES);
    if rerank_count > 0 {
        *failure_stage = "rerank";
        let started = std::time::Instant::now();
        let documents = candidates
            .iter()
            .take(rerank_count)
            .map(|candidate| {
                format!(
                    "{}\n{}\n{}",
                    candidate.row.path,
                    candidate.row.heading_path.join(" > "),
                    candidate.row.text
                )
            })
            .collect::<Vec<_>>();
        let query_owned = query.to_owned();
        let models = state.inner.search_models.clone();
        let scores =
            super::run_model_work(state, move || models.rerank(&query_owned, &documents)).await?;
        completion.latencies.rerank_us = elapsed_us(started);
        if scores.len() != rerank_count {
            return Err(SearchFailure::failed(
                "reranker result count does not match its candidate count",
            )
            .into());
        }
        if scores.iter().any(|score| !score.is_finite()) {
            return Err(SearchFailure::model("reranker returned a non-finite score").into());
        }
        for (candidate, score) in candidates.iter_mut().take(rerank_count).zip(scores) {
            candidate.rerank_score = Some(score);
        }
        let mut reranked = (0..rerank_count).collect::<Vec<_>>();
        reranked.sort_by(|left, right| {
            candidates[*right]
                .rerank_score
                .expect("reranked candidate score")
                .total_cmp(
                    &candidates[*left]
                        .rerank_score
                        .expect("reranked candidate score"),
                )
                .then_with(|| {
                    candidates[*right]
                        .rrf_score
                        .total_cmp(&candidates[*left].rrf_score)
                })
                .then_with(|| {
                    candidates[*left]
                        .row
                        .unit_key
                        .cmp(&candidates[*right].row.unit_key)
                })
        });
        for (index, candidate_index) in reranked.into_iter().enumerate() {
            candidates[candidate_index].reranker_rank = Some(index + 1);
        }
    }

    *failure_stage = "assembly";
    let started = std::time::Instant::now();
    apply_fragment_budget(&mut candidates);
    let response = activation_response(revision_id, &mut candidates, &rows, previous_state)?;
    completion.latencies.assembly_us = elapsed_us(started);
    completion.returned_token_count = candidates
        .iter()
        .filter(|candidate| candidate.final_rank.is_some())
        .map(|candidate| candidate.row.token_count)
        .sum();
    completion.candidates = candidates.iter().map(retrieval_candidate_input).collect();
    Ok(response)
}

async fn fetch_index_rows(
    pool: &SqlitePool,
    revision_id: &str,
    dimensions: usize,
) -> Result<Vec<IndexRow>, DaemonError> {
    let rows = sqlx::query(
        "SELECT u.unit_rowid, u.unit_key, u.resource_id, u.heading_path_json,
                u.locator_json, u.text, u.text_hash, u.token_count, u.vector,
                r.scope, r.kind, r.path, r.title, r.content_hash AS resource_content_hash
         FROM search_units u
         JOIN search_resources r
           ON r.revision_id = u.revision_id AND r.resource_id = u.resource_id
         WHERE u.revision_id = $1
         ORDER BY u.unit_key",
    )
    .bind(revision_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let kind_value: String = row.try_get("kind")?;
            let scope_value: String = row.try_get("scope")?;
            let vector_bytes: Vec<u8> = row.try_get("vector")?;
            let token_count: i64 = row.try_get("token_count")?;
            if token_count < 0 {
                return Err(SearchFailure::failed("stored token count is negative").into());
            }
            Ok(IndexRow {
                rowid: row.try_get("unit_rowid")?,
                unit_key: row.try_get("unit_key")?,
                resource_id: row.try_get("resource_id")?,
                scope: parse_source_scope(&scope_value)?,
                kind: parse_memory_kind(&kind_value).ok_or_else(|| {
                    SearchFailure::failed(format!("unknown indexed memory kind: {kind_value}"))
                })?,
                path: row.try_get("path")?,
                title: row.try_get("title")?,
                heading_path: serde_json::from_str(
                    row.try_get::<String, _>("heading_path_json")?.as_str(),
                )?,
                locator: serde_json::from_str(row.try_get::<String, _>("locator_json")?.as_str())?,
                text: row.try_get("text")?,
                text_hash: row.try_get("text_hash")?,
                resource_content_hash: row.try_get("resource_content_hash")?,
                token_count: token_count as usize,
                vector: decode_vector(&vector_bytes, dimensions)?,
            })
        })
        .collect()
}

pub(super) async fn lexical_ranks(
    pool: &SqlitePool,
    revision_id: &str,
    query: &str,
    rows: &[IndexRow],
) -> Result<Vec<LexicalRank>, DaemonError> {
    let normalized = query.trim().to_lowercase();
    let mut exact = rows
        .iter()
        .filter_map(|row| {
            let resource_id = row.resource_id.to_lowercase();
            let path = row.path.to_lowercase();
            let title = row.title.to_lowercase();
            let priority = if resource_id == normalized {
                Some(0u8)
            } else if path == normalized || title == normalized {
                Some(1)
            } else if resource_id.starts_with(&normalized)
                || path.starts_with(&normalized)
                || title.starts_with(&normalized)
            {
                Some(2)
            } else {
                None
            }?;
            Some((priority, row.unit_key.as_str(), row.rowid))
        })
        .collect::<Vec<_>>();
    exact.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    let mut ranks = HashMap::<i64, LexicalRank>::new();
    let mut ordered = Vec::new();
    for (index, (_, _, rowid)) in exact.into_iter().enumerate() {
        ranks.insert(
            rowid,
            LexicalRank {
                rowid,
                exact_rank: Some(index + 1),
                bm25_rank: None,
                bm25_score: None,
            },
        );
        ordered.push(rowid);
    }
    let mut seen = ordered.iter().copied().collect::<HashSet<_>>();

    if let Some(expression) = fts_expression(query) {
        let matches = sqlx::query(
            "SELECT rowid, bm25(search_units_fts, 0.0, 0.0, 8.0, 6.0, 4.0, 1.0) AS score
             FROM search_units_fts
             WHERE search_units_fts MATCH $1 AND revision_id = $2
             ORDER BY score, unit_key
             LIMIT $3",
        )
        .bind(expression)
        .bind(revision_id)
        .bind(BM25_TOP_K as i64)
        .fetch_all(pool)
        .await?;
        for (index, row) in matches.into_iter().enumerate() {
            let rowid: i64 = row.try_get("rowid")?;
            let score: f64 = row.try_get("score")?;
            if !score.is_finite() {
                return Err(SearchFailure::failed("BM25 returned a non-finite score").into());
            }
            let rank = ranks.entry(rowid).or_insert(LexicalRank {
                rowid,
                exact_rank: None,
                bm25_rank: None,
                bm25_score: None,
            });
            rank.bm25_rank = Some(index + 1);
            rank.bm25_score = Some(score as f32);
            if seen.insert(rowid) {
                ordered.push(rowid);
            }
        }
    }
    ordered.truncate(BM25_TOP_K);
    Ok(ordered
        .into_iter()
        .filter_map(|rowid| ranks.remove(&rowid))
        .collect())
}

pub(super) fn fts_expression(query: &str) -> Option<String> {
    let mut runs = Vec::<String>::new();
    let mut current = String::new();
    for character in query.trim().chars() {
        if character.is_alphanumeric()
            || character == '_'
            || matches!(character, '/' | '-' | '.' | ':')
        {
            current.push(character);
        } else if !current.is_empty() {
            runs.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }

    let mut terms = BTreeSet::new();
    for run in runs {
        let characters = run.chars().collect::<Vec<_>>();
        if characters.len() < 3 {
            continue;
        }
        if characters.iter().any(|character| !character.is_ascii()) {
            for window in characters.windows(3) {
                terms.insert(window.iter().collect::<String>());
                if terms.len() >= 32 {
                    break;
                }
            }
        } else {
            terms.insert(run);
        }
        if terms.len() >= 32 {
            break;
        }
    }
    (!terms.is_empty()).then(|| {
        terms
            .into_iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

pub(super) fn vector_ranks(rows: &[IndexRow], query_vector: &[f32]) -> Vec<VectorRank> {
    let mut scored = rows
        .iter()
        .map(|row| {
            let score = row
                .vector
                .iter()
                .zip(query_vector)
                .map(|(left, right)| left * right)
                .sum::<f32>();
            (score, row.unit_key.as_str(), row.rowid)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    scored
        .into_iter()
        .take(VECTOR_TOP_K)
        .enumerate()
        .map(|(index, (score, _, rowid))| VectorRank {
            rowid,
            rank: index + 1,
            score,
        })
        .collect()
}

pub(super) fn rrf_candidates(
    rows: &[IndexRow],
    lexical: &[LexicalRank],
    vector: &[VectorRank],
) -> Vec<RankedRow> {
    let mut scores = HashMap::<i64, (f32, usize)>::new();
    for (index, lexical) in lexical.iter().enumerate() {
        let rank = index + 1;
        let entry = scores.entry(lexical.rowid).or_insert((0.0, usize::MAX));
        entry.0 += 1.0 / (RRF_CONSTANT + rank as f32);
        entry.1 = entry.1.min(rank);
    }
    for vector in vector {
        let entry = scores.entry(vector.rowid).or_insert((0.0, usize::MAX));
        entry.0 += 1.0 / (RRF_CONSTANT + vector.rank as f32);
        entry.1 = entry.1.min(vector.rank);
    }
    let lexical_by_rowid = lexical
        .iter()
        .map(|rank| (rank.rowid, rank))
        .collect::<HashMap<_, _>>();
    let vector_by_rowid = vector
        .iter()
        .map(|rank| (rank.rowid, rank))
        .collect::<HashMap<_, _>>();
    let by_rowid = rows
        .iter()
        .map(|row| (row.rowid, row))
        .collect::<HashMap<_, _>>();
    let mut fused = scores
        .into_iter()
        .filter_map(|(rowid, (rrf_score, best_rank))| {
            by_rowid.get(&rowid).map(|row| {
                (
                    RankedRow {
                        row: (*row).clone(),
                        exact_rank: lexical_by_rowid
                            .get(&rowid)
                            .and_then(|rank| rank.exact_rank),
                        bm25_rank: lexical_by_rowid.get(&rowid).and_then(|rank| rank.bm25_rank),
                        bm25_score: lexical_by_rowid
                            .get(&rowid)
                            .and_then(|rank| rank.bm25_score),
                        vector_rank: vector_by_rowid.get(&rowid).map(|rank| rank.rank),
                        vector_score: vector_by_rowid.get(&rowid).map(|rank| rank.score),
                        rrf_rank: 0,
                        rrf_score,
                        reranker_rank: None,
                        rerank_score: None,
                        final_rank: None,
                        exclusion_reason: RetrievalExclusionReason::NotReranked,
                        delta_action: None,
                    },
                    best_rank,
                )
            })
        })
        .collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .0
            .rrf_score
            .total_cmp(&left.0.rrf_score)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.0.row.unit_key.cmp(&right.0.row.unit_key))
    });
    fused
        .into_iter()
        .enumerate()
        .map(|(index, (mut row, _))| {
            row.rrf_rank = index + 1;
            row
        })
        .collect()
}

pub(super) fn apply_fragment_budget(candidates: &mut [RankedRow]) {
    let mut reranked = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| candidate.reranker_rank.map(|rank| (rank, index)))
        .collect::<Vec<_>>();
    reranked.sort_by_key(|(rank, _)| *rank);
    let mut selected_indices = Vec::new();
    let mut per_resource = HashMap::<String, usize>::new();
    let mut tokens = 0usize;
    let mut terminal_reason = None;
    for (_, candidate_index) in reranked {
        let candidate = &candidates[candidate_index];
        if let Some(reason) = terminal_reason {
            candidates[candidate_index].exclusion_reason = reason;
            continue;
        }
        if selected_indices.len() >= FINAL_FRAGMENTS {
            candidates[candidate_index].exclusion_reason = RetrievalExclusionReason::FragmentLimit;
            terminal_reason = Some(RetrievalExclusionReason::FragmentLimit);
            continue;
        }
        let relevance = candidate
            .rerank_score
            .map(rerank_relevance)
            .unwrap_or_default();
        if relevance < MIN_RERANK_RELEVANCE {
            candidates[candidate_index].exclusion_reason = RetrievalExclusionReason::BelowRelevance;
            terminal_reason = Some(RetrievalExclusionReason::BelowRelevance);
            continue;
        }
        if selected_indices
            .iter()
            .any(|selected| fragments_overlap(&candidates[*selected], candidate))
        {
            candidates[candidate_index].exclusion_reason = RetrievalExclusionReason::Overlap;
            continue;
        }
        let count = per_resource
            .get(&candidate.row.resource_id)
            .copied()
            .unwrap_or_default();
        if count >= PER_RESOURCE_LIMIT {
            candidates[candidate_index].exclusion_reason =
                RetrievalExclusionReason::PerResourceLimit;
            continue;
        }
        if !selected_indices.is_empty()
            && tokens.saturating_add(candidate.row.token_count) > FRAGMENT_TOKEN_BUDGET
        {
            candidates[candidate_index].exclusion_reason = RetrievalExclusionReason::TokenBudget;
            terminal_reason = Some(RetrievalExclusionReason::TokenBudget);
            continue;
        }
        tokens = tokens.saturating_add(candidate.row.token_count);
        *per_resource
            .entry(candidate.row.resource_id.clone())
            .or_default() += 1;
        candidates[candidate_index].final_rank = Some(selected_indices.len() + 1);
        candidates[candidate_index].exclusion_reason = RetrievalExclusionReason::Selected;
        selected_indices.push(candidate_index);
    }
}

fn retrieval_candidate_input(candidate: &RankedRow) -> RetrievalCandidateInput {
    RetrievalCandidateInput {
        unit_key: candidate.row.unit_key.clone(),
        resource_id: candidate.row.resource_id.clone(),
        scope: candidate.row.scope,
        kind: candidate.row.kind,
        path: candidate.row.path.clone(),
        heading_path: candidate.row.heading_path.clone(),
        locator: candidate.row.locator.clone(),
        content_hash: candidate.row.text_hash.clone(),
        resource_content_hash: candidate.row.resource_content_hash.clone(),
        token_count: candidate.row.token_count,
        evidence_excerpt: candidate.row.text.clone(),
        exact_rank: candidate.exact_rank,
        bm25_rank: candidate.bm25_rank,
        bm25_score: candidate.bm25_score,
        vector_rank: candidate.vector_rank,
        vector_score: candidate.vector_score,
        rrf_rank: Some(candidate.rrf_rank),
        rrf_score: Some(candidate.rrf_score),
        reranker_rank: candidate.reranker_rank,
        reranker_logit: candidate.rerank_score,
        reranker_relevance: candidate.rerank_score.map(rerank_relevance),
        final_rank: candidate.final_rank,
        exclusion_reason: candidate.exclusion_reason,
        delta_action: candidate.delta_action,
    }
}

pub(super) fn rerank_relevance(score: f32) -> f32 {
    if score >= 0.0 {
        1.0 / (1.0 + (-score).exp())
    } else {
        let exponential = score.exp();
        exponential / (1.0 + exponential)
    }
}

fn fragments_overlap(left: &RankedRow, right: &RankedRow) -> bool {
    if left.row.resource_id != right.row.resource_id {
        return false;
    }
    match (&left.row.locator, &right.row.locator) {
        (
            SourceLocator::MarkdownSpan {
                start_byte: left_start,
                end_byte: left_end,
                ..
            },
            SourceLocator::MarkdownSpan {
                start_byte: right_start,
                end_byte: right_end,
                ..
            },
        ) => left_start < right_end && right_start < left_end,
    }
}
