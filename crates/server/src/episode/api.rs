use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpisodeEvidenceRecord {
    pub sequence: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub kind: String,
    pub content: String,
}

pub fn project_episode_evidence_hash(
    records: &[EpisodeEvidenceRecord],
) -> Result<String, serde_json::Error> {
    let mut hasher = Sha256::new();
    for record in records {
        hasher.update(serde_json::to_vec(record)?);
        hasher.update(b"\n");
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinalizeProjectEpisodeRequest {
    pub run_id: String,
    pub host_session_id: Option<String>,
    pub host: String,
    pub evidence_format: String,
    pub evidence_format_revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub activity_at: OffsetDateTime,
    pub evidence_hash: String,
    pub evidence: Vec<EpisodeEvidenceRecord>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEpisodeStatus {
    PendingSummary,
    Active,
    NoMemory,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpisodeSummary {
    pub revision: i64,
    pub summary_algorithm_revision: String,
    pub policy_revision: i64,
    pub evidence_hash: String,
    pub body: String,
    pub no_memory: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEpisode {
    pub episode_id: String,
    pub project_id: String,
    pub run_id: String,
    pub host_session_id: Option<String>,
    pub host: String,
    pub evidence_format: String,
    pub evidence_format_revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub activity_at: OffsetDateTime,
    pub evidence_hash: String,
    pub evidence_bytes: i64,
    pub status: ProjectEpisodeStatus,
    pub current_summary: Option<EpisodeSummary>,
    pub revision: i64,
    pub corpus_revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEpisodeListResponse {
    pub items: Vec<ProjectEpisode>,
    pub corpus_revision: i64,
    pub next_cursor: Option<i64>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpisodeEvidenceSegment {
    pub sequence: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub kind: String,
    pub byte_offset: i64,
    pub content: String,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpisodeEvidencePage {
    pub episode_id: String,
    pub project_id: String,
    pub run_id: String,
    pub evidence_hash: String,
    pub items: Vec<EpisodeEvidenceSegment>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub untrusted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEpisodeSummaryPolicy {
    pub project_id: String,
    pub instructions: String,
    pub revision: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProjectEpisodeSummaryPolicyRequest {
    pub instructions: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewEpisodeSummaryRequest {
    pub instructions: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpisodeSummaryPreview {
    pub episode_id: String,
    pub evidence_hash: String,
    pub summary_algorithm_revision: String,
    pub policy_revision: i64,
    pub body: String,
    pub no_memory: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn evidence_hash_is_stable_and_order_sensitive() {
        let first = EpisodeEvidenceRecord {
            sequence: 1,
            occurred_at: datetime!(2026-08-30 10:00 UTC),
            kind: "message".to_owned(),
            content: "hello".to_owned(),
        };
        let second = EpisodeEvidenceRecord {
            sequence: 2,
            occurred_at: datetime!(2026-08-30 10:01 UTC),
            kind: "tool".to_owned(),
            content: "world".to_owned(),
        };
        let hash = project_episode_evidence_hash(&[first.clone(), second.clone()]).unwrap();
        assert_eq!(
            hash,
            project_episode_evidence_hash(&[first.clone(), second.clone()]).unwrap()
        );
        assert_ne!(
            hash,
            project_episode_evidence_hash(&[second, first]).unwrap()
        );
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 71);
    }

    #[test]
    fn canonical_evidence_matches_the_daemon_fixture() {
        let record = EpisodeEvidenceRecord {
            sequence: 7,
            occurred_at: datetime!(2026-08-30 8:00 UTC),
            kind: "userMessage".to_owned(),
            content: r#"{"type":"userMessage","text":"hello"}"#.to_owned(),
        };
        assert_eq!(
            format!("{}\n", serde_json::to_string(&record).unwrap()),
            "{\"sequence\":7,\"occurred_at\":\"2026-08-30T08:00:00Z\",\"kind\":\"userMessage\",\"content\":\"{\\\"type\\\":\\\"userMessage\\\",\\\"text\\\":\\\"hello\\\"}\"}\n"
        );
        assert_eq!(
            project_episode_evidence_hash(&[record]).unwrap(),
            "sha256:b1e10b4dde6c5ae421507346405d26be19a7cbf940985118a5f967e503893627"
        );
    }
}
