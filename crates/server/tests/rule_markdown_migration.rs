mod common;

use sqlx::Executor;

#[tokio::test]
async fn structured_rules_and_commit_history_migrate_to_markdown() {
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
    ] {
        postgres.pool.execute(migration).await.unwrap();
    }

    let markdown = "# Testing\n\n## Applies when\n\nChanging production code\n\n## Constraint\n\n# Testing\n\nRun focused tests before committing.\n\nTags: testing";
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
                status, content_hash, body, applies_when, tags
            ) VALUES (
                'rule_test', 'org_test', NULL, 'org', 'rule', 'testing/TESTING.md',
                'Testing', 'active', 'sha256:structured',
                '# Testing

Run focused tests before committing.',
                'Changing production code', ARRAY['testing']
            );

            INSERT INTO blobs (blob_id, content) VALUES
                ('blob_rule_old', '{"format":"clumsies.rule.v1","content":{"name":"Testing","applies_when":"Changing production code","constraint":"# Testing\n\nRun focused tests before committing.","tags":["testing"]}}'),
                ('blob_context', '# Context'),
                ('blob_workflow_plain', '# Existing Workflow');
            INSERT INTO trees (tree_id) VALUES ('tree_rule_old'), ('tree_context');
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            ) VALUES
                ('tree_rule_old', 'rule_test', 'rule', 'org', NULL, 'testing/TESTING.md', 'blob_rule_old', 'org'),
                ('tree_rule_old', 'context_shared_blob', 'context', 'org', NULL, 'context/envelope.json', 'blob_rule_old', 'org'),
                ('tree_context', 'context_test', 'context', 'org', NULL, 'context/test.md', 'blob_context', 'org'),
                ('tree_context', 'workflow_plain', 'workflow', 'org', NULL, 'workflows/existing.md', 'blob_workflow_plain', 'org');

            INSERT INTO blobs (blob_id, content)
            SELECT 'blob_markdown_' || value, '# Context ' || value
            FROM generate_series(1, 1000) AS value;
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            )
            SELECT
                'tree_context',
                'context_' || value,
                'context',
                'org',
                NULL,
                'context/' || value || '.md',
                'blob_markdown_' || value,
                'org'
            FROM generate_series(1, 1000) AS value;
            ANALYZE blobs;
            ANALYZE tree_entries;

            INSERT INTO commits (
                commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version, created_at
            ) VALUES
                ('commit_rule_old', 'org', 'org_test', NULL, 'tree_rule_old', NULL, 1, '2026-07-16T00:00:00Z'),
                ('commit_context', 'org', 'org_test', NULL, 'tree_context', 'commit_rule_old', 2, '2026-07-17T00:00:00Z');
            INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id, commit_id)
            VALUES ('ref_test', 'refs/heads/main', 'org', 'org_test', NULL, 'commit_context');

            INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, resource_scope, resource_kind,
                base_commit_id, target_id, status, daemon_installation_id
            ) VALUES (
                'draft_rule', 'project_test', 'user_test', 'Update testing', 'org', 'rule',
                'commit_rule_old', 'rule_test', 'open', 'daemon_test'
            );
            INSERT INTO draft_operations (
                operation_id, draft_id, action, resource_scope, resource_kind, target_id, content
            ) VALUES (
                'operation_rule', 'draft_rule', 'update', 'org', 'rule', 'rule_test',
                '{"kind":"rule","name":"Testing","applies_when":"Before publishing","constraint":"# Testing\n\nRun the complete suite.","tags":["testing"]}'
            );
            INSERT INTO draft_conflicts (draft_id, base_commit_id, current_commit_id)
            VALUES ('draft_rule', 'commit_rule_old', 'commit_context');
            "##,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260722000200_flatten_markdown_memory.sql"
        ))
        .await
        .unwrap();

    let removed_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM information_schema.columns
         WHERE table_schema = 'public'
           AND table_name = 'resources'
           AND column_name IN ('applies_when', 'tags')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(removed_columns, 0);

    let (body, content_hash, revision): (String, String, i64) = sqlx::query_as(
        "SELECT body, content_hash, revision FROM resources WHERE resource_id = 'rule_test'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(body, markdown);
    assert!(content_hash.starts_with("sha256:"));
    assert_eq!(revision, 2);

    let draft_content: serde_json::Value = sqlx::query_scalar(
        "SELECT content FROM draft_operations WHERE operation_id = 'operation_rule'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        draft_content,
        serde_json::json!({
            "kind": "rule",
            "content": "# Testing\n\n## Applies when\n\nBefore publishing\n\n## Constraint\n\n# Testing\n\nRun the complete suite.\n\nTags: testing"
        })
    );

    let new_head: String =
        sqlx::query_scalar("SELECT commit_id FROM refs WHERE ref_id = 'ref_test'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_ne!(new_head, "commit_context");
    let (head_tree_id, new_parent): (String, Option<String>) =
        sqlx::query_as("SELECT tree_id, parent_commit_id FROM commits WHERE commit_id = $1")
            .bind(&new_head)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(head_tree_id, "tree_context");
    let new_parent = new_parent.unwrap();
    assert_ne!(new_parent, "commit_rule_old");

    let migrated_blob: String = sqlx::query_scalar(
        "SELECT blob.content
         FROM commits AS commit
         JOIN tree_entries AS entry ON entry.tree_id = commit.tree_id
         JOIN blobs AS blob ON blob.blob_id = entry.blob_id
         WHERE commit.commit_id = $1 AND entry.resource_kind = 'rule'",
    )
    .bind(&new_parent)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(migrated_blob, markdown);
    let shared_context_blob: String = sqlx::query_scalar(
        "SELECT blob.content
         FROM commits AS commit
         JOIN tree_entries AS entry ON entry.tree_id = commit.tree_id
         JOIN blobs AS blob ON blob.blob_id = entry.blob_id
         WHERE commit.commit_id = $1 AND entry.item_id = 'context_shared_blob'",
    )
    .bind(&new_parent)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert!(shared_context_blob.starts_with("{\"format\":\"clumsies.rule.v1\""));

    let conflict_commits: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT base_commit_id, current_commit_id
         FROM draft_conflicts WHERE draft_id = 'draft_rule'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(conflict_commits, (Some(new_parent.clone()), Some(new_head)));
    let draft_base: String =
        sqlx::query_scalar("SELECT base_commit_id FROM drafts WHERE draft_id = 'draft_rule'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(draft_base, new_parent);

    let stale_commits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commits
         WHERE commit_id IN ('commit_rule_old', 'commit_context')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(stale_commits, 0);
    let shared_blob_still_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM blobs WHERE blob_id = 'blob_rule_old')")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert!(shared_blob_still_exists);
    postgres.shutdown().await;
}
