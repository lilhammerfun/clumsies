//! Repair migration restoring content-addressed organization-selection blobs.

mod common;

use sqlx::Executor;

/// ISSUE-062: 20260816000100 rewrote org-selection Blob CONTENT in place while
/// preserving blob_id, breaking content-address verification for every legacy
/// selection Blob. The repair migration must insert correctly-addressed rows
/// and re-point tree_entries so Commit payloads validate again.
#[tokio::test]
async fn repair_restores_content_address_integrity() {
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

    // Seed a legacy-shape org-selection Blob referenced by a Tree entry, then
    // apply the rewrite migration (breaks its content address), then the
    // repair migration (restores integrity by re-addressing).
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
            ) VALUES (
                'res_legacy', 'org_test', NULL, 'org', 'memory',
                'rules/legacy.md', 'Legacy Rule', 'active', 1, 'hash_rule', '# Rule', 'Rule desc'
            );
            INSERT INTO blobs (blob_id, content) VALUES (
                'blob_legacy_old',
                '{"project_id":"project_test","rules":[{"rule_id":"res_legacy","scope":"org","path":"rules/legacy.md","name":"legacy"}],"revision":1}'
            );
            INSERT INTO trees (tree_id) VALUES ('tree_test');
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            ) VALUES (
                'tree_test', 'project_test', 'project_org_selection', 'project',
                'project_test', NULL, 'blob_legacy_old', 'config'
            );
            "##
        )
        .await
        .unwrap();

    // The rewrite migration rewrites the legacy Blob in place.
    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260816000100_rewrite_org_selection_blobs.sql"
        ))
        .await
        .unwrap();
    let broken: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blobs
         WHERE encode(sha256(convert_to('blob', 'UTF8') || decode('00', 'hex') || convert_to(content, 'UTF8')), 'hex') <> blob_id"
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        broken, 1,
        "rewrite migration must leave one broken content address"
    );

    // The repair migration restores integrity.
    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260817000100_repair_selection_blob_content_addresses.sql"
        ))
        .await
        .unwrap();
    let broken_referenced: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM tree_entries e
         JOIN blobs b ON b.blob_id = e.blob_id
         WHERE encode(sha256(convert_to('blob', 'UTF8') || decode('00', 'hex') || convert_to(b.content, 'UTF8')), 'hex') <> b.blob_id"
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        broken_referenced, 0,
        "repair migration must fix every referenced broken content address"
    );

    // Tree entries must point at the correctly-addressed row.
    let entry_blob: Option<String> =
        sqlx::query_scalar("SELECT blob_id FROM tree_entries WHERE tree_id = 'tree_test'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    let valid: Option<String> = sqlx::query_scalar(
        "SELECT blob_id FROM blobs
         WHERE encode(sha256(convert_to('blob', 'UTF8') || decode('00', 'hex') || convert_to(content, 'UTF8')), 'hex') = blob_id
         AND content LIKE '%memories%'"
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        entry_blob, valid,
        "tree entry must reference the repaired Blob"
    );
    postgres.shutdown().await;
}
