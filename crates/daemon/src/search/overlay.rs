use std::collections::BTreeMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use super::{
    DaemonDraftContent, DaemonDraftOperation, DaemonError, DaemonUpdateDraftOperation,
    EffectiveResource, MemoryKind, SearchFailure, SourceResource, SourceScope, markdown_title,
    parse_memory_kind, parse_source_scope, project_authority_content, sha256, title_from_path,
};

pub(super) fn cached_memory_kind(kind: CachedMemoryKind) -> Option<MemoryKind> {
    match kind {
        CachedMemoryKind::Context
        | CachedMemoryKind::Rule
        | CachedMemoryKind::Workflow
        | CachedMemoryKind::Memory => Some(MemoryKind::Memory),
        CachedMemoryKind::ProjectOrgSelection => None,
    }
}

pub(super) fn cached_scope(scope: CachedScope) -> Option<SourceScope> {
    match scope {
        CachedScope::Org => Some(SourceScope::Org),
        CachedScope::Project => Some(SourceScope::Project),
        CachedScope::Daemon => None,
    }
}

#[derive(Deserialize)]
pub(super) struct CachedCommitPayload {
    pub(super) commit: CachedCommit,
    pub(super) tree: CachedTree,
    pub(super) blobs: Vec<CachedBlob>,
}

#[derive(Deserialize)]
pub(super) struct CachedCommit {
    pub(super) commit_id: String,
}

#[derive(Deserialize)]
pub(super) struct CachedTree {
    pub(super) entries: Vec<CachedTreeEntry>,
}

#[derive(Deserialize)]
pub(super) struct CachedTreeEntry {
    #[serde(default)]
    pub(super) org_source: Option<crate::types::OrgMemorySource>,
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: CachedMemoryKind,
    pub(super) scope: CachedScope,
    pub(super) path: Option<String>,
    pub(super) blob_id: String,
    #[serde(default)]
    pub(super) description: String,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum CachedMemoryKind {
    Context,
    Rule,
    Workflow,
    Memory,
    ProjectOrgSelection,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum CachedScope {
    Org,
    Project,
    Daemon,
}

#[derive(Deserialize)]
pub(super) struct CachedBlob {
    pub(super) blob_id: String,
    pub(super) content: String,
}

#[derive(Debug)]
pub(super) struct DraftOverlay {
    pub(super) draft_id: String,
    pub(super) base_commit_id: Option<String>,
    pub(super) scope: SourceScope,
    pub(super) kind: Option<MemoryKind>,
    pub(super) target_id: Option<String>,
    pub(super) path: Option<String>,
    pub(super) base_resource: Option<EffectiveResource>,
    pub(super) operations: Vec<(i64, String, DaemonDraftOperation)>,
}

pub(super) async fn load_draft_overlays(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<DraftOverlay>, DaemonError> {
    let rows = sqlx::query(
        "SELECT d.draft_id, d.base_commit_id, d.resource_scope, d.resource_kind,
                d.target_id, d.path,
                o.rowid AS operation_order, o.operation_json
         FROM local_drafts d
         JOIN local_draft_operations o ON o.draft_id = d.draft_id
         WHERE d.project_id = $1
           AND d.status IN ('open', 'submitted')
         ORDER BY d.created_at, d.draft_id, o.rowid",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    let mut overlays = Vec::<DraftOverlay>::new();
    for row in rows {
        let draft_id: String = row.try_get("draft_id")?;
        let operation_json: String = row.try_get("operation_json")?;
        let operation = serde_json::from_str(&operation_json).map_err(|error| {
            SearchFailure::failed(format!(
                "local Draft {draft_id} contains an invalid operation: {error}"
            ))
        })?;
        if overlays
            .last()
            .is_none_or(|overlay| overlay.draft_id != draft_id)
        {
            let scope = parse_source_scope(row.try_get::<String, _>("resource_scope")?.as_str())?;
            let kind = parse_memory_kind(row.try_get::<String, _>("resource_kind")?.as_str());
            overlays.push(DraftOverlay {
                draft_id: draft_id.clone(),
                base_commit_id: row.try_get("base_commit_id")?,
                scope,
                kind,
                target_id: row.try_get("target_id")?,
                path: row.try_get("path")?,
                base_resource: None,
                operations: Vec::new(),
            });
        }
        overlays
            .last_mut()
            .expect("overlay inserted above")
            .operations
            .push((row.try_get("operation_order")?, operation_json, operation));
    }
    for overlay in &mut overlays {
        overlay.base_resource = load_draft_base_resource(pool, project_id, overlay).await?;
    }
    Ok(overlays)
}

async fn load_draft_base_resource(
    pool: &SqlitePool,
    project_id: &str,
    draft: &DraftOverlay,
) -> Result<Option<EffectiveResource>, DaemonError> {
    let (Some(commit_id), Some(kind)) = (draft.base_commit_id.as_deref(), draft.kind) else {
        return Ok(None);
    };
    let tree_id: Option<String> =
        sqlx::query_scalar("SELECT tree_id FROM cached_commits WHERE commit_id = $1")
            .bind(commit_id)
            .fetch_optional(pool)
            .await?;
    let Some(tree_id) = tree_id else {
        return Err(SearchFailure::not_ready(format!(
            "Draft {} requires uncached Base Commit {commit_id}",
            draft.draft_id
        ))
        .into());
    };
    let tree_json: String =
        sqlx::query_scalar("SELECT payload_json FROM cached_trees WHERE tree_id = $1")
            .bind(tree_id)
            .fetch_one(pool)
            .await?;
    let tree: CachedTree = serde_json::from_str(&tree_json)?;
    let creates_resource = draft
        .operations
        .first()
        .is_some_and(|(_, _, operation)| operation.create.is_some());
    let entry = tree.entries.into_iter().find(|entry| {
        cached_memory_kind(entry.kind) == Some(kind)
            && cached_scope(entry.scope) == Some(draft.scope)
            && match draft.target_id.as_deref() {
                Some(target_id) => entry.id == target_id,
                None => !creates_resource && entry.path == draft.path,
            }
    });
    let Some(entry) = entry else {
        return Ok(None);
    };
    let path = entry.path.ok_or_else(|| {
        SearchFailure::failed(format!(
            "Draft {} Base resource {} has no path",
            draft.draft_id, entry.id
        ))
    })?;
    let content: String = sqlx::query_scalar("SELECT content FROM cached_blobs WHERE blob_id = $1")
        .bind(entry.blob_id)
        .fetch_one(pool)
        .await?;
    let (content, title) = project_authority_content(kind, &path, &content)?;
    Ok(Some(EffectiveResource {
        source: SourceResource {
            resource_id: entry.id,
            project_id: project_id.to_owned(),
            scope: draft.scope,
            kind,
            path,
            title,
            description: entry.description,
            content_hash: sha256(&content),
            content,
            source_commit_id: Some(commit_id.to_owned()),
            draft_id: None,
            draft_revision: None,
        },
    }))
}

pub(super) fn apply_draft_overlay(
    project_id: &str,
    resources: &mut BTreeMap<String, EffectiveResource>,
    draft: DraftOverlay,
) -> Result<(), DaemonError> {
    let Some(kind) = draft.kind else {
        return Ok(());
    };
    if draft
        .operations
        .iter()
        .any(|(_, _, operation)| operation.discard.is_some())
    {
        return Ok(());
    }
    // Draft synchronization acknowledgements update metadata timestamps but
    // do not change Effective Memory. The semantic revision is therefore
    // derived only from stable identity plus the ordered operation payloads.
    let mut revision_hasher = Sha256::new();
    revision_hasher.update(draft.draft_id.as_bytes());
    for (order, operation_json, _) in &draft.operations {
        revision_hasher.update(order.to_le_bytes());
        revision_hasher.update(operation_json.as_bytes());
    }
    let draft_revision = format!("sha256:{}", hex::encode(revision_hasher.finalize()));

    let target_key = draft
        .target_id
        .clone()
        .unwrap_or_else(|| draft.draft_id.clone());
    // A Draft created on another daemon keeps the server `drf_*` identity
    // locally, while later operations still target the originating daemon's
    // provisional `draft_*` resource identity. Recover that identity from the
    // first operation that targets the created resource.
    let created_resource_id = draft
        .operations
        .iter()
        .find_map(|(_, _, operation)| operation.target_id().map(ToOwned::to_owned))
        .unwrap_or_else(|| draft.draft_id.clone());
    let creates_resource = draft
        .operations
        .first()
        .is_some_and(|(_, _, operation)| operation.create.is_some());
    let mut draft_resources = BTreeMap::new();
    if let Some(base_resource) = draft.base_resource {
        draft_resources.insert(target_key.clone(), base_resource);
    }
    for (_, _, operation) in &draft.operations {
        if let Some(create) = &operation.create {
            if let Some(source) = &create.content.org_source {
                resources.remove(&source.resource_id);
            }
            let resource_id = created_resource_id.clone();
            let resource = draft_content_resource(
                project_id,
                resource_id.clone(),
                draft.scope,
                kind,
                create.path.clone(),
                &create.content,
                None,
                &draft.draft_id,
                &draft_revision,
            )?;
            draft_resources.clear();
            draft_resources.insert(resource_id, resource);
        }
        if let Some(update) = &operation.update {
            let DaemonUpdateDraftOperation::Content(update) = update else {
                return Err(SearchFailure::failed(format!(
                    "Draft {} contains an unmaterialized text replacement",
                    draft.draft_id
                ))
                .into());
            };
            let target_id = if creates_resource && update.id == draft.draft_id {
                &created_resource_id
            } else {
                &update.id
            };
            let existing = draft_resources.remove(target_id).ok_or_else(|| {
                SearchFailure::failed(format!(
                    "Draft {} update target {} is absent from its Base Commit",
                    draft.draft_id, update.id
                ))
            })?;
            let path = existing.source.path.clone();
            let updated = draft_content_resource(
                project_id,
                update.id.clone(),
                existing.source.scope,
                kind,
                path,
                &update.content,
                Some(existing),
                &draft.draft_id,
                &draft_revision,
            )?;
            draft_resources.insert(target_id.clone(), updated);
        }
        if let Some(rename) = &operation.rename
            && let Some(resource) =
                draft_resources.get_mut(if creates_resource && rename.id == draft.draft_id {
                    &created_resource_id
                } else {
                    &rename.id
                })
        {
            resource.source.path = rename.new_path.clone();
            if markdown_title(&resource.source.content).is_none() {
                resource.source.title = title_from_path(&rename.new_path);
            }
            resource.source.draft_id = Some(draft.draft_id.clone());
            resource.source.draft_revision = Some(draft_revision.clone());
        }
        if let Some(delete) = &operation.delete {
            draft_resources.remove(if creates_resource && delete.id == draft.draft_id {
                &created_resource_id
            } else {
                &delete.id
            });
        }
    }
    if let Some(target_id) = draft.target_id.as_ref() {
        resources.remove(target_id);
    }
    resources.remove(&draft.draft_id);
    resources.extend(draft_resources);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draft_content_resource(
    project_id: &str,
    resource_id: String,
    scope: SourceScope,
    kind: MemoryKind,
    path: String,
    content: &DaemonDraftContent,
    existing: Option<EffectiveResource>,
    draft_id: &str,
    draft_revision: &str,
) -> Result<EffectiveResource, DaemonError> {
    let source_commit_id = existing
        .as_ref()
        .and_then(|resource| resource.source.source_commit_id.clone());
    let description = content
        .description
        .clone()
        .or_else(|| {
            existing
                .as_ref()
                .map(|resource| resource.source.description.clone())
        })
        .unwrap_or_default();
    let (rendered, title) = {
        let content = &content.content;
        (
            content.clone(),
            markdown_title(content).unwrap_or_else(|| title_from_path(&path)),
        )
    };
    Ok(EffectiveResource {
        source: SourceResource {
            resource_id,
            project_id: project_id.to_owned(),
            scope,
            kind,
            path,
            title,
            description,
            content_hash: sha256(&rendered),
            content: rendered,
            source_commit_id,
            draft_id: Some(draft_id.to_owned()),
            draft_revision: Some(draft_revision.to_owned()),
        },
    })
}
