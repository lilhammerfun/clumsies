use std::collections::{HashMap, HashSet};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use super::query::{IndexRow, RankedRow};
use super::{
    ActivateMemoryResponse, ActivationAction, ActivationFragment, ActivationRemoval, DaemonError,
    RANKING_CONFIG_VERSION, RetrievalDeltaAction, SearchFailure,
};

pub(super) const MAX_ACTIVATION_IDENTITIES: usize = 256;
pub(super) const MAX_ACTIVATION_STATE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ActivationStateToken {
    pub(super) version: u8,
    pub(super) epoch: u64,
    pub(super) known: Vec<KnownActivationIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct KnownActivationIdentity {
    pub(super) unit_key: String,
    pub(super) content_hash: String,
    pub(super) last_seen: u64,
}

pub(super) fn decode_activation_state(
    state: Option<&str>,
) -> Result<ActivationStateToken, DaemonError> {
    let Some(state) = state else {
        return Ok(ActivationStateToken {
            version: 1,
            epoch: 0,
            known: Vec::new(),
        });
    };
    if state.len() > MAX_ACTIVATION_STATE_BYTES * 2 {
        return Err(SearchFailure::invalid_state("activation state is too large").into());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(state)
        .map_err(|_| SearchFailure::invalid_state("activation state is not valid base64url"))?;
    if bytes.len() > MAX_ACTIVATION_STATE_BYTES {
        return Err(SearchFailure::invalid_state("activation state is too large").into());
    }
    let decoded: ActivationStateToken = serde_json::from_slice(&bytes)
        .map_err(|_| SearchFailure::invalid_state("activation state is not valid JSON"))?;
    if decoded.version != 1 || decoded.known.len() > MAX_ACTIVATION_IDENTITIES {
        return Err(SearchFailure::invalid_state(
            "activation state version or identity count is unsupported",
        )
        .into());
    }
    let mut identities = HashSet::new();
    if decoded
        .known
        .iter()
        .any(|identity| !identities.insert(identity.unit_key.as_str()))
    {
        return Err(SearchFailure::invalid_state(
            "activation state contains duplicate unit identities",
        )
        .into());
    }
    Ok(decoded)
}

pub(super) fn activation_response(
    revision_id: &str,
    candidates: &mut [RankedRow],
    all_rows: &[IndexRow],
    previous_state: ActivationStateToken,
) -> Result<ActivateMemoryResponse, DaemonError> {
    let epoch = previous_state.epoch.saturating_add(1);
    let existing = all_rows
        .iter()
        .map(|row| row.unit_key.as_str())
        .collect::<HashSet<_>>();
    let previous = previous_state
        .known
        .iter()
        .map(|identity| (identity.unit_key.clone(), identity.content_hash.clone()))
        .collect::<HashMap<_, _>>();
    let mut removed = previous_state
        .known
        .iter()
        .filter(|identity| !existing.contains(identity.unit_key.as_str()))
        .map(|identity| ActivationRemoval {
            unit_key: identity.unit_key.clone(),
        })
        .collect::<Vec<_>>();
    removed.sort_by(|left, right| left.unit_key.cmp(&right.unit_key));

    let mut next_known = previous_state
        .known
        .into_iter()
        .filter(|identity| existing.contains(identity.unit_key.as_str()))
        .map(|identity| (identity.unit_key.clone(), identity))
        .collect::<HashMap<_, _>>();
    let mut fragments = Vec::new();
    let mut selected = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| candidate.final_rank.map(|rank| (rank, index)))
        .collect::<Vec<_>>();
    selected.sort_by_key(|(rank, _)| *rank);
    for (_, candidate_index) in selected {
        let candidate = &mut candidates[candidate_index];
        let action = match previous.get(candidate.row.unit_key.as_str()) {
            Some(content_hash) if *content_hash == candidate.row.text_hash => {
                ActivationAction::Reuse
            }
            Some(_) => ActivationAction::Replace,
            None => ActivationAction::Add,
        };
        candidate.delta_action = Some(match action {
            ActivationAction::Add => RetrievalDeltaAction::Add,
            ActivationAction::Replace => RetrievalDeltaAction::Replace,
            ActivationAction::Reuse => RetrievalDeltaAction::Reuse,
        });
        next_known.insert(
            candidate.row.unit_key.clone(),
            KnownActivationIdentity {
                unit_key: candidate.row.unit_key.clone(),
                content_hash: candidate.row.text_hash.clone(),
                last_seen: epoch,
            },
        );
        fragments.push(ActivationFragment {
            action,
            unit_key: candidate.row.unit_key.clone(),
            content_hash: candidate.row.text_hash.clone(),
            resource_id: candidate.row.resource_id.clone(),
            scope: candidate.row.scope,
            kind: candidate.row.kind,
            path: candidate.row.path.clone(),
            heading_path: candidate.row.heading_path.clone(),
            content: (action != ActivationAction::Reuse).then(|| candidate.row.text.clone()),
        });
    }
    let mut known = next_known.into_values().collect::<Vec<_>>();
    known.sort_by(|left, right| {
        right
            .last_seen
            .cmp(&left.last_seen)
            .then_with(|| left.unit_key.cmp(&right.unit_key))
    });
    known.truncate(MAX_ACTIVATION_IDENTITIES);
    let next_state = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&ActivationStateToken {
        version: 1,
        epoch,
        known,
    })?);
    Ok(ActivateMemoryResponse {
        run_id: None,
        index_revision: revision_id.to_owned(),
        profile: RANKING_CONFIG_VERSION.to_owned(),
        next_state,
        fragments,
        removed,
    })
}
