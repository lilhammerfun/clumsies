//! Keeps Activity list snapshots lightweight and loads only selected session details.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::Arc;

use super::*;

/// Number of concurrent window/filter snapshots retained by the resident daemon.
const SNAPSHOT_CAPACITY: usize = 8;
/// Number of recently selected, unenriched session bodies retained for paging.
const DETAIL_CAPACITY: usize = 4;
/// Default list and task page size.
const PAGE_SIZE: usize = 20;
/// Maximum number of rows returned in one desktop reply.
const MAX_PAGE_SIZE: usize = 100;
/// Bound for a compressed session's list preview; details have no such truncation.
const DSH_PREVIEW_BYTES: u64 = 64 * 1024;

/// Ephemeral per-daemon snapshots. No session transcripts are persisted by Activity.
#[derive(Default)]
pub(crate) struct RecallCache {
    /// Recent listings keep continuation tokens stable while files are appended.
    listings: VecDeque<Arc<Listing>>,
    /// Only selected sessions are parsed; pages reuse their raw task snapshot.
    details: VecDeque<(String, Arc<RecallSession>)>,
}

/// A discovered file and its lightweight display metadata.
struct SessionFile {
    /// Internal path discovered under a host's session directory.
    path: PathBuf,
    /// Project binding at discovery, checked again before exposing details.
    project_id: String,
    /// Display metadata; contains no task or retrieval bodies.
    summary: RecallSessionSummary,
}

/// Immutable ordering for one initial list request and all its subsequent pages.
struct Listing {
    /// Random identity, used with an index as an opaque continuation/selection handle.
    id: String,
    /// Exact bindings authorized for this listing.
    roots: Vec<(String, String)>,
    /// Full binding snapshot, including nested projects outside the active filter.
    bindings: Vec<(String, String)>,
    /// Deduplicated summaries in newest-first order.
    files: Vec<SessionFile>,
}

/// Reports a stale handle with a user-recoverable refresh action.
fn expired() -> DaemonError {
    DaemonError::InvalidRequest("Activity snapshot expired. Refresh Activity and try again.".into())
}

/// Splits an opaque handle without interpreting any user input as a filesystem path.
///
/// # Errors
/// Rejects malformed identifiers and non-numeric indices.
fn decode_handle(value: &str) -> Result<(&str, usize), DaemonError> {
    let (id, index) = value.split_once(':').ok_or_else(expired)?;
    uuid::Uuid::parse_str(id).map_err(|_| expired())?;
    Ok((id, index.parse().map_err(|_| expired())?))
}

/// Bounds reply work without truncating the complete session history.
fn page_size(limit: Option<u32>) -> usize {
    limit
        .map(|v| v as usize)
        .unwrap_or(PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

/// Reads only a small decompressed prefix for DSH's list title and identity.
///
/// # Errors
/// Returns unreadable file or invalid compressed-header errors. A partial final
/// frame leaves the already decoded prefix available, as with live session logs.
fn dsh_summary(path: &Path, root: &str) -> Result<Option<RecallSession>, DaemonError> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < DSH_PREVIEW_BYTES {
        let remaining = DSH_PREVIEW_BYTES - bytes.len() as u64;
        let decoder = match ruzstd::decoding::StreamingDecoder::new(&mut file) {
            Ok(decoder) => decoder,
            Err(_) if !bytes.is_empty() => break,
            Err(error) => {
                return Err(DaemonError::InvalidRequest(format!(
                    "Invalid Activity header: {error}"
                )));
            }
        };
        if decoder.take(remaining).read_to_end(&mut bytes).is_err() {
            break;
        }
    }
    Ok(parse_session_text(&String::from_utf8_lossy(&bytes), root))
}

/// Discovers summaries on the blocking pool. Never enriches retrievals or parses
/// a complete Codex rollout merely to populate the list.
///
/// # Errors
/// Returns session-directory I/O errors. A malformed individual session is skipped.
fn discover(
    home: &Path,
    codex_home: &Path,
    roots: Vec<(String, String)>,
    bindings: Vec<(String, String)>,
) -> Result<Listing, DaemonError> {
    // ponytail: initial loads and explicit refreshes walk bounded headers;
    // continuations reuse this snapshot. Add a persistent metadata index only
    // if profiling shows discovery itself still dominates first-page latency.
    let mut files = Vec::new();
    for (root, project_id) in &roots {
        for path in
            list_session_files(&home.join(".dsh/sessions").join(encode_workspace_dir(root)))?
        {
            match dsh_summary(&path, root) {
                Ok(Some(session))
                    if binding_for_cwd(&session.workspace_root, &bindings)
                        .is_some_and(|binding| binding == &(root.clone(), project_id.clone())) =>
                {
                    files.push(SessionFile {
                        path,
                        project_id: project_id.clone(),
                        summary: RecallSessionSummary {
                            host: AgentHost::Dsh,
                            session_id: session.session_id,
                            title: session.title.or_else(|| {
                                session
                                    .tasks
                                    .first()
                                    .map(|t| t.text.chars().take(160).collect())
                            }),
                            workspace_root: root.clone(),
                            created_at: session.created_at,
                            session_token: String::new(),
                        },
                    })
                }
                Ok(_) => {}
                Err(error) => tracing::warn!("skipping unreadable Activity header: {error}"),
            }
        }
    }
    let titles = codex::read_session_titles(&codex_home.join("session_index.jsonl"))
        .unwrap_or_else(|error| {
            tracing::warn!("cannot read Activity title index: {error}");
            Default::default()
        });
    let candidates = codex::list_candidates(codex_home, |cwd| {
        binding_for_cwd(cwd, &bindings).is_some_and(|binding| roots.contains(binding))
    })
    .unwrap_or_else(|error| {
        tracing::warn!("cannot discover Codex Activity: {error}");
        Vec::new()
    });
    for candidate in candidates {
        let Some((root, project_id)) = binding_for_cwd(&candidate.header.cwd, &roots) else {
            continue;
        };
        let title = titles
            .get(&candidate.header.session_id)
            .cloned()
            .or_else(|| {
                codex::read_title_preview(&candidate.file.path)
                    .ok()
                    .flatten()
            });
        files.push(SessionFile {
            path: candidate.file.path,
            project_id: project_id.clone(),
            summary: RecallSessionSummary {
                host: AgentHost::Codex,
                session_id: candidate.header.session_id,
                title,
                workspace_root: root.clone(),
                created_at: candidate.file.created_at,
                session_token: String::new(),
            },
        });
    }
    files.sort_by(|a, b| {
        b.summary
            .created_at
            .cmp(&a.summary.created_at)
            .then_with(|| a.summary.host.as_str().cmp(b.summary.host.as_str()))
            .then_with(|| b.summary.session_id.cmp(&a.summary.session_id))
    });
    let id = uuid::Uuid::new_v4().to_string();
    for (index, file) in files.iter_mut().enumerate() {
        file.summary.session_token = format!("{id}:{index}");
    }
    Ok(Listing {
        id,
        roots,
        bindings,
        files,
    })
}

/// Returns one summary page, reusing the original discovery result for continuations.
///
/// # Errors
/// Returns binding/discovery errors and rejects expired or differently scoped cursors.
pub(crate) async fn list_recalls(
    state: &DaemonState,
    request: ListRecallsRequest,
) -> Result<ListRecallsResponse, DaemonError> {
    let bindings = load_bindings(state).await?;
    let roots = filter_bindings(
        bindings.clone(),
        request.workspace_root.as_deref(),
        request.project_id.as_deref(),
    );
    let (listing, start) = if let Some(cursor) = &request.cursor {
        let (id, start) = decode_handle(cursor)?;
        let cache = state.inner.recall_cache.lock().await;
        let listing = cache
            .listings
            .iter()
            .find(|l| l.id == id && l.roots == roots && l.bindings == bindings)
            .cloned()
            .ok_or_else(expired)?;
        (listing, start)
    } else {
        let home = home_dir()?;
        let codex_home = codex_sessions_home(state.inner.config.codex_home.as_deref(), &home);
        let listing =
            tokio::task::spawn_blocking(move || discover(&home, &codex_home, roots, bindings))
                .await
                .map_err(|error| {
                    DaemonError::InvalidRequest(format!("Activity discovery failed: {error}"))
                })??;
        let listing = Arc::new(listing);
        let mut cache = state.inner.recall_cache.lock().await;
        cache.listings.push_back(listing.clone());
        while cache.listings.len() > SNAPSHOT_CAPACITY {
            cache.listings.pop_front();
        }
        (listing, 0)
    };
    if start > listing.files.len() {
        return Err(expired());
    }
    let end = start
        .saturating_add(page_size(request.limit))
        .min(listing.files.len());
    Ok(ListRecallsResponse {
        sessions: listing.files[start..end]
            .iter()
            .map(|f| f.summary.clone())
            .collect(),
        workspace_roots: listing.roots.iter().map(|(root, _)| root.clone()).collect(),
        next_cursor: (end < listing.files.len()).then(|| format!("{}:{end}", listing.id)),
    })
}

/// Parses one selected file, confirming it still belongs to the requested binding.
///
/// # Errors
/// Returns file/parse failures and rejects replaced or moved session identities.
fn read_detail(
    file: &SessionFile,
    bindings: &[(String, String)],
) -> Result<RecallSession, DaemonError> {
    let summary = &file.summary;
    let session = match summary.host {
        AgentHost::Codex => {
            let session = codex::parse_rollout_file(
                &file.path,
                summary.title.as_deref(),
                summary.created_at,
            )?
            .ok_or_else(expired)?;
            if binding_for_cwd(&session.cwd, bindings).is_none_or(|(root, project)| {
                root != &summary.workspace_root || project != &file.project_id
            }) {
                return Err(expired());
            }
            codex_recall_session(session, summary.workspace_root.clone())
        }
        AgentHost::Dsh => {
            parse_session(&file.path, &summary.workspace_root)?.ok_or_else(expired)?
        }
        _ => return Err(expired()),
    };
    if session.session_id != summary.session_id
        || binding_for_cwd(&session.workspace_root, bindings).is_none_or(|(root, project)| {
            root != &summary.workspace_root || project != &file.project_id
        })
    {
        return Err(expired());
    }
    Ok(session)
}

/// Returns a task page and enriches only its activations. Subsequent pages reuse
/// the raw session snapshot, so appending a live log cannot shift task offsets.
///
/// # Errors
/// Returns binding, file, and pagination errors. A cleared retrieval history leaves
/// the session's original tool previews intact.
pub(crate) async fn get_recall_session(
    state: &DaemonState,
    request: GetRecallSessionRequest,
) -> Result<GetRecallSessionResponse, DaemonError> {
    let bindings = load_bindings(state).await?;
    let (id, index) = decode_handle(&request.session_token)?;
    let (listing, cached) = {
        let cache = state.inner.recall_cache.lock().await;
        let listing = cache
            .listings
            .iter()
            .find(|l| l.id == id && l.bindings == bindings)
            .cloned()
            .ok_or_else(expired)?;
        let cached = cache
            .details
            .iter()
            .find(|(token, _)| token == &request.session_token)
            .map(|(_, s)| s.clone());
        (listing, cached)
    };
    let file = listing.files.get(index).ok_or_else(expired)?;
    if !bindings.contains(&(file.summary.workspace_root.clone(), file.project_id.clone())) {
        return Err(expired());
    }
    let project_id = file.project_id.clone();
    let start = request.offset.unwrap_or(0);
    let raw = if let Some(session) = cached {
        session
    } else {
        // A continuation must never silently switch to a newer version of a live log.
        if start != 0 {
            return Err(expired());
        }
        let session =
            tokio::task::spawn_blocking(move || read_detail(&listing.files[index], &bindings))
                .await
                .map_err(|error| {
                    DaemonError::InvalidRequest(format!("Activity read failed: {error}"))
                })??;
        let session = Arc::new(session);
        let mut cache = state.inner.recall_cache.lock().await;
        // Concurrent readers of one token must all page the same raw snapshot.
        let session = cache
            .details
            .iter()
            .find(|(token, _)| token == &request.session_token)
            .map(|(_, cached)| cached.clone())
            .unwrap_or(session);
        cache
            .details
            .retain(|(token, _)| token != &request.session_token);
        cache
            .details
            .push_back((request.session_token, session.clone()));
        while cache.details.len() > DETAIL_CAPACITY {
            cache.details.pop_front();
        }
        session
    };
    if start > raw.tasks.len() {
        return Err(expired());
    }
    let end = start
        .saturating_add(page_size(request.limit))
        .min(raw.tasks.len());
    let mut session = RecallSession {
        host: raw.host,
        session_id: raw.session_id.clone(),
        title: raw.title.clone(),
        workspace_root: raw.workspace_root.clone(),
        created_at: raw.created_at,
        tasks: raw.tasks[start..end].to_vec(),
    };
    enrich_session(state, &project_id, &mut session).await;
    Ok(GetRecallSessionResponse {
        session,
        total_tasks: raw.tasks.len(),
        next_offset: (end < raw.tasks.len()).then_some(end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CredentialStore, CredentialStoreError, DaemonConfig, ServerCredentials};

    struct NoCredentials;
    impl CredentialStore for NoCredentials {
        fn load(&self) -> Result<Option<ServerCredentials>, CredentialStoreError> {
            Ok(None)
        }
        fn replace(&self, _: &ServerCredentials) -> Result<(), CredentialStoreError> {
            Ok(())
        }
        fn clear(&self) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    // Every writable path and the Codex home belong to this fixture. No credentials
    // or resident daemon are used, and its synthetic workspace has no DSH history.
    async fn fixture(temp: &tempfile::TempDir) -> (DaemonState, PathBuf, String) {
        let mut config = DaemonConfig::for_root(temp.path().join("daemon"));
        config.codex_home = Some(temp.path().join("codex"));
        let directory = temp.path().join("codex/sessions");
        std::fs::create_dir_all(&directory).unwrap();
        let state = DaemonState::initialize_with_credential_store(config, Arc::new(NoCredentials))
            .await
            .unwrap();
        let root = temp.path().join("workspace").display().to_string();
        sqlx::query("INSERT INTO project_bindings (server_url, workspace_root, project_id, revision) VALUES ('https://test.invalid', $1, 'project', 1)")
            .bind(&root).execute(&state.inner.pool).await.unwrap();
        (state, directory, root)
    }

    fn request(cursor: Option<String>) -> ListRecallsRequest {
        ListRecallsRequest {
            workspace_root: None,
            project_id: Some("project".into()),
            limit: Some(20),
            cursor,
        }
    }

    fn rollout(id: &str, root: &str, tasks: usize) -> Vec<u8> {
        let mut text =
            serde_json::json!({"type":"session_meta", "payload":{"id":id,"cwd":root}}).to_string();
        for index in 0..tasks {
            text.push('\n');
            text.push_str(&serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":format!("task-{index}")}}).to_string());
        }
        text.into_bytes()
    }

    #[tokio::test]
    async fn list_pages_do_not_read_bodies_or_rediscover_files() {
        let temp = tempfile::tempdir().unwrap();
        let (state, directory, root) = fixture(&temp).await;
        for i in 0..55 {
            let mut bytes = rollout(&format!("session-{i}"), &root, 0);
            bytes.extend_from_slice(b"\n\xff\xfe\n"); // Full parsing would fail UTF-8 decoding.
            std::fs::write(directory.join(format!("rollout-{i}.jsonl")), bytes).unwrap();
        }
        let first = list_recalls(&state, request(None)).await.unwrap();
        assert_eq!(first.sessions.len(), 20);
        assert!(
            serde_json::to_value(&first.sessions[0])
                .unwrap()
                .get("tasks")
                .is_none()
        );
        assert!(state.inner.recall_cache.lock().await.details.is_empty());
        let mut wrong_project = request(first.next_cursor.clone());
        wrong_project.project_id = Some("different".into());
        assert!(list_recalls(&state, wrong_project).await.is_err());
        // Removing discovery input cannot affect continuation pages of the snapshot.
        std::fs::remove_dir_all(&directory).unwrap();
        let second = list_recalls(&state, request(first.next_cursor))
            .await
            .unwrap();
        let third = list_recalls(&state, request(second.next_cursor))
            .await
            .unwrap();
        assert_eq!(second.sessions.len(), 20);
        assert_eq!(third.sessions.len(), 15);
        assert!(third.next_cursor.is_none());
        let ids: std::collections::HashSet<_> = first
            .sessions
            .into_iter()
            .chain(second.sessions)
            .chain(third.sessions)
            .map(|s| s.session_id)
            .collect();
        assert_eq!(ids.len(), 55);
        assert!(
            get_recall_session(
                &state,
                GetRecallSessionRequest {
                    session_token: "../../etc/passwd:0".into(),
                    offset: None,
                    limit: None
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn selected_tasks_page_beyond_old_limit_and_keep_a_stable_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let (state, directory, root) = fixture(&temp).await;
        let path = directory.join("rollout-long.jsonl");
        std::fs::write(&path, rollout("long", &root, 505)).unwrap();
        let list = list_recalls(&state, request(None)).await.unwrap();
        let token = list.sessions[0].session_token.clone();
        let mut detail_request = GetRecallSessionRequest {
            session_token: token,
            offset: None,
            limit: Some(100),
        };
        let mut page = get_recall_session(&state, detail_request.clone())
            .await
            .unwrap();
        assert_eq!(page.total_tasks, 505);
        assert_eq!(page.session.tasks.len(), 100);
        // Continuations must use the original parsed tasks, not the changed file.
        std::fs::write(&path, rollout("long", &root, 506)).unwrap();
        let mut texts: Vec<_> = page.session.tasks.iter().map(|t| t.text.clone()).collect();
        while let Some(offset) = page.next_offset {
            detail_request.offset = Some(offset);
            page = get_recall_session(&state, detail_request.clone())
                .await
                .unwrap();
            assert_eq!(page.total_tasks, 505);
            texts.extend(page.session.tasks.iter().map(|t| t.text.clone()));
        }
        assert_eq!(
            texts,
            (0..505).map(|i| format!("task-{i}")).collect::<Vec<_>>()
        );
        detail_request.offset = Some(usize::MAX);
        assert!(
            get_recall_session(&state, detail_request.clone())
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM project_bindings")
            .execute(&state.inner.pool)
            .await
            .unwrap();
        detail_request.offset = None;
        assert!(get_recall_session(&state, detail_request).await.is_err());
    }

    #[tokio::test]
    async fn project_filter_respects_nested_bindings_and_expired_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let (state, directory, root) = fixture(&temp).await;
        let nested = format!("{root}/nested");
        sqlx::query("INSERT INTO project_bindings (server_url, workspace_root, project_id, revision) VALUES ('https://test.invalid', $1, 'nested-project', 1)")
            .bind(&nested).execute(&state.inner.pool).await.unwrap();
        std::fs::write(
            directory.join("rollout-parent.jsonl"),
            rollout("parent", &root, 1),
        )
        .unwrap();
        std::fs::write(
            directory.join("rollout-nested.jsonl"),
            rollout("nested", &nested, 1),
        )
        .unwrap();
        let page = list_recalls(&state, request(None)).await.unwrap();
        assert_eq!(
            page.sessions
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            ["parent"]
        );
        let token = page.sessions[0].session_token.clone();
        for _ in 0..SNAPSHOT_CAPACITY {
            list_recalls(&state, request(None)).await.unwrap();
        }
        assert!(
            get_recall_session(
                &state,
                GetRecallSessionRequest {
                    session_token: token,
                    offset: None,
                    limit: None
                }
            )
            .await
            .is_err()
        );
        let cache = state.inner.recall_cache.lock().await;
        assert_eq!(cache.listings.len(), SNAPSHOT_CAPACITY);
    }

    #[tokio::test]
    async fn new_nested_binding_invalidates_cached_parent_details() {
        let temp = tempfile::tempdir().unwrap();
        let (state, directory, root) = fixture(&temp).await;
        let nested = format!("{root}/nested");
        std::fs::write(
            directory.join("rollout-nested.jsonl"),
            rollout("nested", &nested, 2),
        )
        .unwrap();
        let page = list_recalls(&state, request(None)).await.unwrap();
        let detail_request = GetRecallSessionRequest {
            session_token: page.sessions[0].session_token.clone(),
            offset: None,
            limit: Some(1),
        };
        let detail = get_recall_session(&state, detail_request.clone())
            .await
            .unwrap();
        assert_eq!(detail.next_offset, Some(1));
        sqlx::query("INSERT INTO project_bindings (server_url, workspace_root, project_id, revision) VALUES ('https://test.invalid', $1, 'nested-project', 1)")
            .bind(&nested).execute(&state.inner.pool).await.unwrap();
        assert!(get_recall_session(&state, detail_request).await.is_err());
        assert!(
            list_recalls(&state, request(None))
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
    }

    #[test]
    fn dsh_summary_reads_a_bounded_multiframe_prefix() {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};
        let temp = tempfile::NamedTempFile::new().unwrap();
        let header = b"{\"type\":\"session\",\"id\":\"dsh\",\"cwd\":\"/workspace\"}\n";
        let mut bytes = compress_to_vec(header.as_slice(), CompressionLevel::Uncompressed);
        let prompt = serde_json::json!({"type":"user/message","data":{"source":{"kind":"user"},"content":[{"type":"text","text":"first request"}],"id":"one"}}).to_string() + "\n";
        bytes.extend(compress_to_vec(
            prompt.as_bytes(),
            CompressionLevel::Uncompressed,
        ));
        bytes.extend(compress_to_vec(
            vec![b' '; 100_000].as_slice(),
            CompressionLevel::Uncompressed,
        ));
        bytes.extend_from_slice(b"invalid trailing frame");
        std::fs::write(temp.path(), bytes).unwrap();
        let session = dsh_summary(temp.path(), "/workspace").unwrap().unwrap();
        assert_eq!(session.session_id, "dsh");
        assert_eq!(session.tasks[0].text, "first request");
    }
}
