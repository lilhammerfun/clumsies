//! User-level harness choices and installations; Project bindings only route Memory.
use super::*;

const ADAPTERS: [ProjectAgentAdapterKind; 5] = [
    ProjectAgentAdapterKind::Codex,
    ProjectAgentAdapterKind::ClaudeCode,
    ProjectAgentAdapterKind::Opencode,
    ProjectAgentAdapterKind::Dsh,
    ProjectAgentAdapterKind::Antigravity,
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonAgentAdapterSetting {
    pub adapter: ProjectAgentAdapterKind,
    pub enabled: bool,
    pub configured: bool,
    pub installed: bool,
    pub legacy_repositories: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonAgentAdapterSettings {
    pub items: Vec<DaemonAgentAdapterSetting>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonSetAgentAdapterRequest {
    pub adapter: ProjectAgentAdapterKind,
    pub enabled: bool,
    pub runtime_binary_path: String,
    pub host_binary_path: Option<String>,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS host_agent_adapters (
            adapter TEXT PRIMARY KEY CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
            enabled BOOLEAN NOT NULL CHECK (enabled IN (0, 1)),
            revision BIGINT NOT NULL CHECK (revision > 0),
            manifest_json TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS host_adapter_fs_ops (
            adapter TEXT PRIMARY KEY CHECK (adapter IN ('codex', 'claude-code', 'opencode', 'dsh', 'antigravity')),
            operation_json TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn enabled(
    pool: &SqlitePool,
    adapter: ProjectAgentAdapterKind,
) -> Result<Option<bool>, DaemonError> {
    Ok(
        sqlx::query_scalar("SELECT enabled FROM host_agent_adapters WHERE adapter = $1")
            .bind(adapter.as_str())
            .fetch_optional(pool)
            .await?,
    )
}

pub(crate) async fn list(state: &DaemonState) -> Result<DaemonAgentAdapterSettings, DaemonError> {
    let mut items = Vec::new();
    for adapter in ADAPTERS {
        let preference = enabled(&state.inner.pool, adapter).await?;
        let installed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM host_agent_adapters
             WHERE adapter = $1 AND manifest_json IS NOT NULL)",
        )
        .bind(adapter.as_str())
        .fetch_one(&state.inner.pool)
        .await?;
        let legacy: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM project_agent_adapters WHERE adapter = $1")
                .bind(adapter.as_str())
                .fetch_one(&state.inner.pool)
                .await?;
        items.push(DaemonAgentAdapterSetting {
            adapter,
            enabled: preference.unwrap_or(adapter == ProjectAgentAdapterKind::Codex || legacy > 0),
            configured: preference.is_some(),
            installed,
            legacy_repositories: legacy as usize,
        });
    }
    Ok(DaemonAgentAdapterSettings { items })
}

pub(crate) async fn set(
    state: &DaemonState,
    request: DaemonSetAgentAdapterRequest,
) -> Result<DaemonAgentAdapterSettings, DaemonError> {
    let home = if state.inner.config.dev_instance_id.is_some() {
        // A development App must never write the user's real harness configuration.
        let path = state.inner.config.root_dir.join("agent-host-home");
        crate::project_storage::ensure_private_directory(&path)?;
        path
    } else {
        crate::util::home_dir()?
    };
    set_at(state, request, &fs::canonicalize(home)?).await?;
    list(state).await
}

async fn set_at(
    state: &DaemonState,
    request: DaemonSetAgentAdapterRequest,
    home: &Path,
) -> Result<(), DaemonError> {
    let _guard = state.inner.local_setup_lock.lock().await;
    recover_adapter(&state.inner.pool, request.adapter).await?;
    let runtime = canonical_agent_runtime_binary(&request.runtime_binary_path)?;
    #[cfg(target_os = "macos")]
    verify_code_signature(&runtime)?;
    let hash = sha256_file(&runtime)?;
    let row =
        sqlx::query("SELECT revision, manifest_json FROM host_agent_adapters WHERE adapter = $1")
            .bind(request.adapter.as_str())
            .fetch_optional(&state.inner.pool)
            .await?;
    let revision = row
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?;
    let manifest = row
        .as_ref()
        .map(|row| row.try_get::<Option<String>, _>("manifest_json"))
        .transpose()?
        .flatten()
        .map(|raw| serde_json::from_str::<AdapterManifest>(&raw))
        .transpose()?;

    // Retire only daemon-owned repository files. Missing disks stay recorded for a later retry.
    let legacy = sqlx::query(
        "SELECT server_url, workspace_root, project_id, adapter, revision,
                manifest_json, created_at, updated_at
         FROM project_agent_adapters WHERE adapter = $1 ORDER BY server_url, workspace_root",
    )
    .bind(request.adapter.as_str())
    .fetch_all(&state.inner.pool)
    .await?;
    for row in &legacy {
        let record = adapter_record_from_row(row)?;
        let workspace = Path::new(&record.status.workspace_root);
        if workspace.is_dir() {
            remove_plan(&record.manifest, workspace)?;
        }
    }

    if request.adapter == ProjectAgentAdapterKind::Codex {
        let mut installed_manifest = None;
        if let Some(host) = request.host_binary_path.as_deref() {
            if request.enabled {
                codex_plugin::ensure_installed(
                    &state.inner.config.root_dir,
                    &runtime,
                    &hash,
                    Some(host),
                    state.inner.config.dev_instance_id.as_deref(),
                )
                .await?;
                installed_manifest = Some(serde_json::to_string(&AdapterManifest {
                    runtime_binary_hash: hash,
                    runtime_binary_path: runtime.display().to_string(),
                    delivery: ProjectAgentAdapterDelivery::HostPlugin,
                    managed_files: Vec::new(),
                })?);
            } else {
                let status = codex_plugin::inspect(
                    &state.inner.config.root_dir,
                    &runtime,
                    &hash,
                    Some(host),
                    state.inner.config.dev_instance_id.as_deref(),
                )
                .await?;
                if status.marketplace_conflict {
                    return Err(adapter_conflict(
                        "The Codex marketplace belongs to another installation.",
                    ));
                }
                codex_plugin::remove(host).await?;
            }
        }
        save(
            &state.inner.pool,
            request.adapter,
            request.enabled,
            revision.unwrap_or(0) + 1,
            installed_manifest.as_deref(),
        )
        .await?;
    } else {
        let changes = if request.enabled {
            install_plan(request.adapter, home, &runtime, manifest.as_ref())?
        } else if let Some(manifest) = &manifest {
            remove_plan(manifest, home)?
        } else {
            Vec::new()
        };
        let next_manifest = request
            .enabled
            .then(|| manifest_for_changes(&changes, &runtime, hash));
        if changes.is_empty() {
            save(
                &state.inner.pool,
                request.adapter,
                false,
                revision.unwrap_or(0) + 1,
                None,
            )
            .await?;
        } else if manifest != next_manifest
            || changes
                .iter()
                .map(change_is_needed)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|needed| needed)
        {
            let operation = prepared_adapter_fs_op(
                PreparedAdapterFsOp {
                    global: true,
                    operation_id: Uuid::new_v4().to_string(),
                    server_url: String::new(),
                    project_id: String::new(),
                    workspace_root: home.to_path_buf(),
                    adapter: request.adapter,
                    action: if request.enabled {
                        AdapterFsAction::Install
                    } else {
                        AdapterFsAction::Remove
                    },
                    expected_revision: revision,
                    next_revision: request.enabled.then_some(revision.unwrap_or(0) + 1),
                    manifest_json: next_manifest
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    changes: Vec::new(),
                },
                &changes,
            )?;
            sqlx::query(
                "INSERT INTO host_adapter_fs_ops (adapter, operation_json) VALUES ($1, $2)",
            )
            .bind(request.adapter.as_str())
            .bind(serde_json::to_string(&operation)?)
            .execute(&state.inner.pool)
            .await?;
            finish(&state.inner.pool, &operation).await?;
        }
    }
    // Retire only daemon-owned repository files after the user-level install succeeds.
    for row in legacy {
        let record = adapter_from_row(&row)?;
        if Path::new(&record.workspace_root).is_dir() {
            remove_from_server(
                state,
                DaemonProjectAgentAdapterRemoveRequest {
                    workspace_root: record.workspace_root,
                    adapter: record.adapter,
                    expected_revision: record.revision,
                },
                record.server_url,
            )
            .await?;
        }
    }
    Ok(())
}

async fn save(
    pool: &SqlitePool,
    adapter: ProjectAgentAdapterKind,
    enabled: bool,
    revision: i64,
    manifest: Option<&str>,
) -> Result<(), DaemonError> {
    sqlx::query(
        "INSERT INTO host_agent_adapters (adapter, enabled, revision, manifest_json)
         VALUES ($1, $2, $3, $4) ON CONFLICT(adapter) DO UPDATE SET
         enabled = excluded.enabled, revision = excluded.revision, manifest_json = excluded.manifest_json
         WHERE host_agent_adapters.enabled != excluded.enabled
            OR host_agent_adapters.manifest_json IS NOT excluded.manifest_json",
    ).bind(adapter.as_str()).bind(enabled).bind(revision).bind(manifest).execute(pool).await?;
    Ok(())
}

pub(super) async fn recover(pool: &SqlitePool) -> Result<(), DaemonError> {
    for adapter in ADAPTERS {
        recover_adapter(pool, adapter).await?;
    }
    Ok(())
}

async fn recover_adapter(
    pool: &SqlitePool,
    adapter: ProjectAgentAdapterKind,
) -> Result<(), DaemonError> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT operation_json FROM host_adapter_fs_ops WHERE adapter = $1")
            .bind(adapter.as_str())
            .fetch_optional(pool)
            .await?;
    if let Some(raw) = raw {
        if raw.len() > MAX_ADAPTER_FS_JOURNAL_BYTES {
            return Err(adapter_conflict("The global adapter journal is too large."));
        }
        let operation: PreparedAdapterFsOp = serde_json::from_str(&raw)?;
        if !operation.global || operation.adapter != adapter {
            return Err(adapter_conflict(
                "The global adapter journal has a mismatched operation.",
            ));
        }
        finish(pool, &operation).await?;
    }
    Ok(())
}

async fn finish(pool: &SqlitePool, operation: &PreparedAdapterFsOp) -> Result<(), DaemonError> {
    apply_prepared_adapter_fs_op(operation)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM host_agent_adapters WHERE adapter = $1")
            .bind(operation.adapter.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    if current != operation.expected_revision {
        return Err(adapter_conflict(
            "The global adapter changed during installation.",
        ));
    }
    sqlx::query(
        "INSERT INTO host_agent_adapters (adapter, enabled, revision, manifest_json)
         VALUES ($1, $2, $3, $4) ON CONFLICT(adapter) DO UPDATE SET
         enabled = excluded.enabled, revision = excluded.revision, manifest_json = excluded.manifest_json",
    ).bind(operation.adapter.as_str()).bind(operation.action == AdapterFsAction::Install)
        .bind(operation.next_revision.unwrap_or(current.unwrap_or(0) + 1))
        .bind(&operation.manifest_json).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM host_adapter_fs_ops WHERE adapter = $1")
        .bind(operation.adapter.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

fn install_plan(
    adapter: ProjectAgentAdapterKind,
    home: &Path,
    runtime: &Path,
    previous: Option<&AdapterManifest>,
) -> Result<Vec<PendingChange>, DaemonError> {
    let binary = runtime
        .to_str()
        .ok_or_else(|| adapter_conflict("Runtime path is not UTF-8."))?;
    let old = previous.map(|manifest| manifest.runtime_binary_path.as_str());
    match adapter {
        ProjectAgentAdapterKind::ClaudeCode => install_plan_with_claude_mcp_path(
            adapter,
            home,
            runtime,
            previous,
            Some(&home.join(".claude.json")),
            None,
            None,
        ),
        ProjectAgentAdapterKind::Opencode => Ok(vec![
            merged_change(
                home.join(".config/opencode/opencode.json"),
                ManagedFileKind::OpencodeConfig,
                0o644,
                |current| render_opencode_config_for_scope(current, binary, old, false),
            )?,
            exclusive_change(
                home.join(".config/opencode/plugins/clumsies.ts"),
                render_opencode_plugin(binary).as_bytes(),
                previous,
                0o644,
            )?,
        ]),
        ProjectAgentAdapterKind::Antigravity => {
            let directory = home.join(".gemini/config");
            let script = directory.join("hooks/agent-run-event.sh");
            let ownership = HookOwnership {
                lifecycle: manifest_manages_path(previous, &script),
                legacy_prompt: false,
            };
            Ok(vec![
                merged_change(
                    directory.join("mcp_config.json"),
                    ManagedFileKind::ClaudeMcp,
                    0o644,
                    |current| render_claude_mcp(current, binary, old),
                )?,
                merged_change(
                    directory.join("hooks.json"),
                    ManagedFileKind::AntigravityHooks,
                    0o644,
                    |current| render_antigravity_hooks(current, &script, ownership),
                )?,
                exclusive_change(
                    directory.join("hooks/resolve-binary.sh"),
                    render_managed_binary_resolver(adapter, binary).as_bytes(),
                    previous,
                    0o755,
                )?,
                exclusive_change(
                    script,
                    render_managed_hook_script(ISSUE_RUN_EVENT_ANTIGRAVITY, binary).as_bytes(),
                    previous,
                    0o755,
                )?,
            ])
        }
        ProjectAgentAdapterKind::Dsh => Ok(vec![exclusive_change_with_kind(
            home.join(".dsh/clumsies.json"),
            &render_json(&json!({"runtime": binary}))?,
            previous,
            0o644,
            ManagedFileKind::DshConfig,
        )?]),
        ProjectAgentAdapterKind::Codex => {
            Err(adapter_conflict("Codex uses its global plugin installer."))
        }
    }
}

pub(super) fn validate_path(
    adapter: ProjectAgentAdapterKind,
    path: &Path,
    kind: ManagedFileKind,
) -> Result<(), DaemonError> {
    let allowed = match adapter {
        ProjectAgentAdapterKind::ClaudeCode => matches!(
            (path.to_str(), kind),
            (Some(".claude.json"), ManagedFileKind::ClaudeMcp)
                | (
                    Some(".claude/settings.json"),
                    ManagedFileKind::ClaudeSettings
                )
                | (
                    Some(".claude/hooks/resolve-binary.sh" | ".claude/hooks/agent-run-event.sh"),
                    ManagedFileKind::Exclusive
                )
        ),
        ProjectAgentAdapterKind::Opencode => matches!(
            (path.to_str(), kind),
            (
                Some(".config/opencode/opencode.json"),
                ManagedFileKind::OpencodeConfig
            ) | (
                Some(".config/opencode/plugins/clumsies.ts"),
                ManagedFileKind::Exclusive
            )
        ),
        ProjectAgentAdapterKind::Antigravity => matches!(
            (path.to_str(), kind),
            (
                Some(".gemini/config/mcp_config.json"),
                ManagedFileKind::ClaudeMcp
            ) | (
                Some(".gemini/config/hooks.json"),
                ManagedFileKind::AntigravityHooks
            ) | (
                Some(
                    ".gemini/config/hooks/resolve-binary.sh"
                        | ".gemini/config/hooks/agent-run-event.sh"
                ),
                ManagedFileKind::Exclusive
            )
        ),
        ProjectAgentAdapterKind::Dsh => {
            path == Path::new(".dsh/clumsies.json") && kind == ManagedFileKind::DshConfig
        }
        ProjectAgentAdapterKind::Codex => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(adapter_conflict(
            "A global adapter path is outside its harness namespace.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn global_files_recover_and_remove_without_a_project_binding() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        migrate(&pool).await.unwrap();
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        fs::write(
            home.join(".claude.json"),
            br#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();
        for adapter in ADAPTERS
            .into_iter()
            .filter(|adapter| *adapter != ProjectAgentAdapterKind::Codex)
        {
            let changes = install_plan(adapter, &home, runtime, None).unwrap();
            let manifest = manifest_for_changes(&changes, runtime, "a".repeat(64));
            let operation = prepared_adapter_fs_op(
                PreparedAdapterFsOp {
                    global: true,
                    operation_id: Uuid::new_v4().to_string(),
                    server_url: String::new(),
                    project_id: String::new(),
                    workspace_root: home.clone(),
                    adapter,
                    action: AdapterFsAction::Install,
                    expected_revision: None,
                    next_revision: Some(1),
                    manifest_json: Some(serde_json::to_string(&manifest).unwrap()),
                    changes: Vec::new(),
                },
                &changes,
            )
            .unwrap();
            sqlx::query("INSERT INTO host_adapter_fs_ops VALUES ($1, $2)")
                .bind(adapter.as_str())
                .bind(serde_json::to_string(&operation).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            // Simulate a process exit after the first file was committed, before the record was saved.
            apply_journal_change_cas(&operation, 0, &operation.changes[0]).unwrap();
            recover(&pool).await.unwrap();
            assert_eq!(enabled(&pool, adapter).await.unwrap(), Some(true));
            assert!(
                install_plan(adapter, &home, runtime, Some(&manifest))
                    .unwrap()
                    .iter()
                    .all(|change| !change_is_needed(change).unwrap())
            );
            if adapter == ProjectAgentAdapterKind::Opencode {
                let config: Value = serde_json::from_slice(
                    &fs::read(home.join(".config/opencode/opencode.json")).unwrap(),
                )
                .unwrap();
                assert!(
                    config.get("plugin").is_none(),
                    "Global plugins are discovered by the harness, without a repository path"
                );
            }
            if adapter == ProjectAgentAdapterKind::Dsh {
                let config: Value =
                    serde_json::from_slice(&fs::read(home.join(".dsh/clumsies.json")).unwrap())
                        .unwrap();
                assert!(config.get("project_id").is_none());
                assert!(config.get("server_url").is_none());
            }
            let removals = remove_plan(&manifest, &home).unwrap();
            let removal = prepared_adapter_fs_op(
                PreparedAdapterFsOp {
                    global: true,
                    operation_id: Uuid::new_v4().to_string(),
                    server_url: String::new(),
                    project_id: String::new(),
                    workspace_root: home.clone(),
                    adapter,
                    action: AdapterFsAction::Remove,
                    expected_revision: Some(1),
                    next_revision: None,
                    manifest_json: None,
                    changes: Vec::new(),
                },
                &removals,
            )
            .unwrap();
            sqlx::query("INSERT INTO host_adapter_fs_ops VALUES ($1, $2)")
                .bind(adapter.as_str())
                .bind(serde_json::to_string(&removal).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            recover(&pool).await.unwrap();
            assert_eq!(enabled(&pool, adapter).await.unwrap(), Some(false));
            for file in &manifest.managed_files {
                if file.kind == ManagedFileKind::Exclusive {
                    assert!(!Path::new(&file.path).exists());
                }
            }
        }
        let claude: Value =
            serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(
            claude,
            json!({"theme":"dark", "mcpServers":{"other":{"command":"other"}}})
        );
        assert!(!home.join(".mcp.json").exists());
        assert!(!home.join("opencode.json").exists());
    }

    #[test]
    fn global_install_and_remove_protect_foreign_configuration() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let runtime = Path::new("/Applications/Clumsies.app/Contents/Resources/clumsiesd");
        let foreign = br#"{"mcpServers":{"clumsies":{"command":"user-owned"}}}"#;
        fs::write(home.join(".claude.json"), foreign).unwrap();
        assert!(install_plan(ProjectAgentAdapterKind::ClaudeCode, &home, runtime, None).is_err());
        assert_eq!(fs::read(home.join(".claude.json")).unwrap(), foreign);
        let changes =
            install_plan(ProjectAgentAdapterKind::Antigravity, &home, runtime, None).unwrap();
        apply_changes(&changes).unwrap();
        let manifest = manifest_for_changes(&changes, runtime, "a".repeat(64));
        fs::write(
            home.join(".gemini/config/hooks/agent-run-event.sh"),
            "user change",
        )
        .unwrap();
        assert!(remove_plan(&manifest, &home).is_err());
        assert!(
            validate_path(
                ProjectAgentAdapterKind::ClaudeCode,
                Path::new(".mcp.json"),
                ManagedFileKind::ClaudeMcp
            )
            .is_err()
        );
    }
}
