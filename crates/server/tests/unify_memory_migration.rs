//! Migration unifying legacy resource categories under Memory.

mod common;

use sqlx::Executor;

/// ISSUE-012: the unify-memory migration must apply cleanly to a LIVE
/// database that still contains legacy kind values (rule/context/workflow,
/// metaprompt) — PostgreSQL validates existing rows when a CHECK constraint
/// is added, so the migration rewrites kind labels before narrowing them.
/// Identity (resource_id) is preserved verbatim: this is the old_id ->
/// memory_id map.
#[tokio::test]
async fn unify_memory_migration_rewrites_legacy_kinds_on_live_data() {
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
    ] {
        postgres.pool.execute(migration).await.unwrap();
    }

    // Legacy effective state: one resource of every pre-unification kind,
    // legacy Commit tree entries, a legacy draft with operations, and a
    // bundle item carrying its redundant kind column.
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
                status, revision, content_hash, body, context_kind
            ) VALUES
                ('rul_legacy_rule', 'org_test', NULL, 'org', 'rule',
                 'rules/legacy-rule.md', 'Legacy Rule', 'active', 1, 'hash_rule', '# Rule', NULL),
                ('ctx_legacy_context', 'org_test', NULL, 'org', 'context',
                 'context/legacy-context.md', 'Legacy Context', 'active', 1, 'hash_context', '# Context', 'file'),
                ('wfl_legacy_workflow', 'org_test', 'project_test', 'project', 'workflow',
                 'workflows/legacy-workflow.md', 'Legacy Workflow', 'active', 1, 'hash_workflow', '# Workflow', NULL);

            INSERT INTO blobs (blob_id, content) VALUES
                ('blob_v1', '# Legacy v1'),
                ('blob_v2', '# Legacy v2');
            INSERT INTO trees (tree_id) VALUES ('tree_v1'), ('tree_v2');
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            ) VALUES
                ('tree_v1', 'rul_legacy_rule', 'rule', 'org', NULL, 'rules/legacy-rule.md', 'blob_v1', 'org'),
                ('tree_v1', 'ctx_legacy_context', 'context', 'org', NULL, 'context/legacy-context.md', 'blob_v1', 'org'),
                ('tree_v1', 'wfl_legacy_workflow', 'workflow', 'project', 'project_test', 'workflows/legacy-workflow.md', 'blob_v1', 'project'),
                ('tree_v1', 'selection_1', 'project_org_selection', 'project', 'project_test', 'ORG_MEMORY_SELECTION', 'blob_v2', 'project'),
                ('tree_v2', 'rul_legacy_rule', 'rule', 'org', NULL, 'rules/legacy-rule.md', 'blob_v2', 'org');
            INSERT INTO commits (
                commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version, created_at
            ) VALUES
                ('commit_v1', 'org', 'org_test', NULL, 'tree_v1', NULL, 1, '2026-07-16T00:00:00Z'),
                ('commit_v2', 'project', 'org_test', 'project_test', 'tree_v2', 'commit_v1', 2, '2026-07-17T00:00:00Z');

            INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, description, resource_scope,
                resource_kind, base_commit_id, target_id, path, status, version,
                daemon_installation_id
            ) VALUES (
                'draft_legacy', 'project_test', 'user_test', 'Legacy draft', 'Draft desc',
                'org', 'rule', 'commit_v1', 'rul_legacy_rule', NULL, 'open', 1, 'daemon_test'
            );
            INSERT INTO draft_operations (
                operation_id, draft_id, action, resource_scope, resource_kind, target_id, path, content
            ) VALUES (
                'operation_legacy', 'draft_legacy', 'update', 'org', 'rule', 'rul_legacy_rule',
                'rules/legacy-rule.md', '{"kind":"rule","content":"# Updated"}'
            );

            INSERT INTO personal_bundles (bundle_id, owner_user_id, name, description, revision)
            VALUES ('bundle_legacy', 'user_test', 'Bundle', '', 1);
            INSERT INTO personal_bundle_items (bundle_id, resource_id, resource_kind, position)
            VALUES ('bundle_legacy', 'rul_legacy_rule', 'rule', 0);
            "##,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260815000100_unify_memory_model.sql"
        ))
        .await
        .unwrap();

    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT resource_kind FROM resources ORDER BY resource_id")
            .fetch_all(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(
        kinds,
        vec![
            "memory".to_owned(),
            "memory".to_owned(),
            "memory".to_owned()
        ],
        "every legacy resource kind rewritten to memory"
    );

    // Description backfilled from name; identity (old_id) preserved.
    let descriptions: Vec<(String, String)> =
        sqlx::query_as("SELECT resource_id, description FROM resources ORDER BY resource_id")
            .fetch_all(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(
        descriptions,
        vec![
            ("ctx_legacy_context".to_owned(), "Legacy Context".to_owned()),
            ("rul_legacy_rule".to_owned(), "Legacy Rule".to_owned()),
            (
                "wfl_legacy_workflow".to_owned(),
                "Legacy Workflow".to_owned()
            ),
        ]
    );

    // Unified path namespace indexes.
    let org_index: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE indexname = 'resources_org_path_idx'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert!(
        org_index.contains("(org_id, path)"),
        "org path index unified: {org_index}"
    );

    // Removed columns and tables.
    let context_kind_gone: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns
         WHERE table_name = 'resources' AND column_name = 'context_kind'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(context_kind_gone, 0, "context_kind column dropped");
    let bundle_kind_gone: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns
         WHERE table_name = 'personal_bundle_items' AND column_name = 'resource_kind'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(bundle_kind_gone, 0, "bundle item kind column dropped");
    let workflow_steps_gone: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('workflow_steps')::text")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert!(
        workflow_steps_gone.is_none(),
        "workflow_steps table dropped"
    );

    // Historical tree entries: legacy kinds rewritten, selection kind kept,
    // description column present.
    let tree_kinds: Vec<(String, String)> =
        sqlx::query_as("SELECT item_id, resource_kind FROM tree_entries ORDER BY item_id")
            .fetch_all(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(
        tree_kinds,
        vec![
            ("ctx_legacy_context".to_owned(), "memory".to_owned()),
            ("rul_legacy_rule".to_owned(), "memory".to_owned()),
            ("rul_legacy_rule".to_owned(), "memory".to_owned()),
            ("selection_1".to_owned(), "project_org_selection".to_owned()),
            ("wfl_legacy_workflow".to_owned(), "memory".to_owned()),
        ]
    );

    // Drafts and operations carry the single Memory kind.
    let draft_kind: String =
        sqlx::query_scalar("SELECT resource_kind FROM drafts WHERE draft_id = 'draft_legacy'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(draft_kind, "memory");
    let operation_kind: String = sqlx::query_scalar(
        "SELECT resource_kind FROM draft_operations WHERE operation_id = 'operation_legacy'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(operation_kind, "memory");
    postgres.shutdown().await;
}
