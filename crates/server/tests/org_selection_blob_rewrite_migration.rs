//! Migration rewriting stored organization selection configuration blobs.

mod common;

use sqlx::Executor;

/// ISSUE-012 follow-up: the unify migration narrowed kinds and added
/// description, but historical Commit payloads still carry the org
/// selection in the legacy shape {rules, context, workflows}. New Server
/// code deserializes every Commit payload into the unified
/// ProjectOrgSelection {project_id, memories, revision}, so reading an
/// archived Commit fails. The rewrite migration must convert old Blobs in
/// place (identity preserved, description backfilled, obsolete context
/// keys dropped) and leave already-unified Blobs untouched.
#[tokio::test]
async fn rewrite_migration_converts_legacy_selection_blobs() {
    let postgres = common::postgres_without_migrations().await;
    for migration in [
        include_str!("../migrations/20260708000100_create_server_schema.sql"),
        include_str!("../migrations/20260714000100_add_user_avatar.sql"),
        include_str!("../migrations/20260715000100_add_draft_conflicts.sql"),
        include_str!("../migrations/20260716000100_add_server_installation_setup.sql"),
        include_str!("../migrations/20260716000200_add_web_admin_sessions.sql"),
        include_str!("../migrations/20260720000100_flatten_workflow_content.sql"),
        include_str!("../migrations/20260720000200_flatten_workflow_draft_content.sql"),
        include_str!("../migrations/20260722000100_remove_metaprompt.sql"),
        include_str!("../migrations/20260722000200_flatten_markdown_memory.sql"),
        include_str!("../migrations/20260722000300_add_draft_reconciliation.sql"),
        include_str!("../migrations/20260722000400_refresh_draft_projections.sql"),
        include_str!("../migrations/20260731000100_close_project_creation.sql"),
        include_str!("../migrations/20260809000100_add_review_comment_anchors.sql"),
        include_str!("../migrations/20260810000100_add_review_versions_and_decisions.sql"),
        include_str!("../migrations/20260815000100_unify_memory_model.sql"),
    ] {
        postgres.pool.execute(migration).await.unwrap();
    }

    // Post-unify database that still holds archived Commits with legacy
    // org-selection Blobs: one legacy-shape Blob and one already-unified
    // Blob, both referenced as org-selection tree entries.
    postgres
        .pool
        .execute(
            r##"
            INSERT INTO orgs (org_id, name) VALUES ('org_test', 'Test');
            INSERT INTO projects (project_id, org_id, name)
            VALUES ('project_test', 'org_test', 'Project');
            INSERT INTO users (user_id, email, display_name, role, status)
            VALUES ('user_test', 'owner@example.com', 'Owner', 'owner', 'active');

            INSERT INTO resources (
                resource_id, org_id, project_id, scope, resource_kind, path, name,
                status, revision, content_hash, body, description
            ) VALUES
                ('rul_legacy_rule', 'org_test', NULL, 'org', 'memory',
                 'rules/legacy-rule.md', 'Legacy Rule', 'active', 1, 'hash_rule', '# Rule', 'Rule desc'),
                ('ctx_legacy_context', 'org_test', NULL, 'org', 'memory',
                 'context/legacy-context.md', 'Legacy Context', 'active', 1, 'hash_context', '# Context', 'Context desc'),
                ('wfl_legacy_workflow', 'org_test', 'project_test', 'project', 'memory',
                 'workflows/legacy-workflow.md', 'Legacy Workflow', 'active', 1, 'hash_workflow', '# Workflow', 'Workflow desc');

            INSERT INTO blobs (blob_id, content) VALUES
                ('blob_selection_legacy', '{
                    "project_id": "project_test",
                    "rules": [{
                        "rule_id": "rul_legacy_rule", "scope": "org", "project_id": null,
                        "path": "rules/legacy-rule.md", "name": "Legacy Rule",
                        "content_hash": "hash_rule", "status": "active",
                        "updated_at": "2026-07-16T00:00:00Z"
                    }],
                    "context": [{
                        "context_id": "ctx_legacy_context", "scope": "org", "project_id": null,
                        "kind": "file", "path": "context/legacy-context.md",
                        "content_hash": "hash_context", "size": 12,
                        "updated_at": "2026-07-16T00:00:00Z"
                    }],
                    "workflows": [{
                        "workflow_id": "wfl_legacy_workflow", "scope": "project",
                        "project_id": "project_test", "path": "workflows/legacy-workflow.md",
                        "name": "Legacy Workflow", "content_hash": "hash_workflow",
                        "status": "active", "updated_at": "2026-07-16T00:00:00Z"
                    }],
                    "revision": 3
                }'),
                ('blob_selection_unified', '{
                    "project_id": "project_test",
                    "memories": [{
                        "memory_id": "rul_legacy_rule", "scope": "org", "project_id": null,
                        "path": "rules/legacy-rule.md", "name": "Legacy Rule",
                        "description": "Rule desc", "content_hash": "hash_rule",
                        "status": "active", "updated_at": "2026-07-16T00:00:00Z"
                    }],
                    "revision": 5
                }'),
                -- Production-shaped data: plain Markdown resource bodies share
                -- the blobs table with org-selection payloads, and a corrupt
                -- selection entry may reference a non-object Blob. Neither
                -- may abort the migration.
                ('blob_markdown_body', '# s6-0 TUI 组件体系总览

clumsies TUI 的完整 UI 组件库文档正文。'),
                ('blob_selection_corrupt', '# 不是 JSON 的 selection 内容');

            INSERT INTO trees (tree_id) VALUES ('tree_v1');
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            ) VALUES
                ('tree_v1', 'selection_legacy', 'project_org_selection', 'project',
                 'project_test', NULL, 'blob_selection_legacy', 'config'),
                ('tree_v1', 'selection_unified', 'project_org_selection', 'project',
                 'project_test', NULL, 'blob_selection_unified', 'config'),
                ('tree_v1', 'selection_corrupt', 'project_org_selection', 'project',
                 'project_test', NULL, 'blob_selection_corrupt', 'config');
            "##,
        )
        .await
        .unwrap();

    let rewrite_migration =
        include_str!("../migrations/20260816000100_rewrite_org_selection_blobs.sql");
    postgres.pool.execute(rewrite_migration).await.unwrap();

    let legacy: serde_json::Value = sqlx::query_scalar(
        "SELECT content::jsonb FROM blobs WHERE blob_id = 'blob_selection_legacy'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(legacy["project_id"], "project_test");
    assert_eq!(legacy["revision"], 3);
    assert!(
        legacy.get("rules").is_none() && legacy.get("context").is_none(),
        "legacy keys removed: {legacy}"
    );
    assert!(legacy["memories"].is_array());
    let memories = legacy["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 3, "{legacy}");
    let by_id = |id: &str| {
        memories
            .iter()
            .find(|item| item["memory_id"] == id)
            .unwrap_or_else(|| panic!("memory {id} missing: {legacy}"))
    };
    let rule = by_id("rul_legacy_rule");
    assert_eq!(rule["memory_id"], "rul_legacy_rule");
    assert_eq!(rule["name"], "Legacy Rule");
    assert_eq!(rule["description"], "Rule desc");
    assert_eq!(rule["path"], "rules/legacy-rule.md");
    assert_eq!(rule["content_hash"], "hash_rule");
    let context = by_id("ctx_legacy_context");
    assert_eq!(
        context["description"], "Context desc",
        "backfilled from resources"
    );
    assert!(
        context.get("kind").is_none() && context.get("size").is_none(),
        "obsolete context keys dropped: {context}"
    );
    assert_eq!(context["updated_at"], "2026-07-16T00:00:00Z");
    let workflow = by_id("wfl_legacy_workflow");
    assert_eq!(workflow["project_id"], "project_test");
    assert_eq!(workflow["description"], "Workflow desc");
    assert_eq!(workflow["status"], "active");

    // Production-shaped data: Markdown resource bodies in the blobs table
    // and a corrupt org-selection entry must not abort the migration, and
    // the corrupt entry is skipped rather than rewritten.
    let markdown: String =
        sqlx::query_scalar("SELECT content FROM blobs WHERE blob_id = 'blob_markdown_body'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert!(markdown.starts_with("# s6-0"), "Markdown body untouched");
    let corrupt: String =
        sqlx::query_scalar("SELECT content FROM blobs WHERE blob_id = 'blob_selection_corrupt'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(
        corrupt, "# 不是 JSON 的 selection 内容",
        "corrupt selection Blob skipped, not rewritten"
    );

    // Idempotent: the already-unified Blob is untouched, and re-running the
    // migration changes nothing.
    let unified: serde_json::Value = sqlx::query_scalar(
        "SELECT content::jsonb FROM blobs WHERE blob_id = 'blob_selection_unified'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(unified["revision"], 5);
    assert_eq!(unified["memories"].as_array().unwrap().len(), 1);
    assert_eq!(unified["memories"][0]["description"], "Rule desc");

    postgres.pool.execute(rewrite_migration).await.unwrap();
    let after_rerun: serde_json::Value = sqlx::query_scalar(
        "SELECT content::jsonb FROM blobs WHERE blob_id = 'blob_selection_legacy'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(after_rerun, legacy, "rewrite is idempotent");
    postgres.shutdown().await;
}
