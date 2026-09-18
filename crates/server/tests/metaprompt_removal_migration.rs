mod common;

use sqlx::Executor;

#[tokio::test]
async fn migration_rewrites_commit_history_and_removes_metaprompt_domain_data() {
    let postgres = common::postgres_without_migrations().await;
    for migration in [
        include_str!("../migrations/20260708000100_create_server_schema.sql"),
        include_str!("../migrations/20260714000100_add_user_avatar.sql"),
        include_str!("../migrations/20260715000100_add_draft_conflicts.sql"),
        include_str!("../migrations/20260716000100_add_server_installation_setup.sql"),
        include_str!("../migrations/20260716000200_add_web_admin_sessions.sql"),
        include_str!("../migrations/20260720000100_flatten_workflow_content.sql"),
        include_str!("../migrations/20260720000200_flatten_workflow_draft_content.sql"),
    ] {
        postgres.pool.execute(migration).await.unwrap();
    }

    postgres
        .pool
        .execute(
            r##"
            INSERT INTO orgs (org_id, name) VALUES ('org_test', 'Test');
            INSERT INTO projects (project_id, org_id, name)
            VALUES ('project_test', 'org_test', 'Project');
            INSERT INTO users (user_id, email, display_name, role, status)
            VALUES ('user_test', 'owner@example.com', 'Owner', 'owner', 'active');

            INSERT INTO blobs (blob_id, content) VALUES
                ('blob_context_v1', '# Context v1'),
                ('blob_context_v2', '# Context v2'),
                ('blob_context_v3', '# Context v3'),
                ('blob_metaprompt', '# Obsolete MCP bootstrap');
            INSERT INTO trees (tree_id) VALUES ('tree_v1'), ('tree_v2'), ('tree_v3');
            INSERT INTO tree_entries (
                tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
            ) VALUES
                ('tree_v1', 'context_test', 'context', 'org', NULL, 'context/test.md', 'blob_context_v1', 'org'),
                ('tree_v1', 'metaprompt_test', 'metaprompt', 'org', NULL, 'META_PROMPT.md', 'blob_metaprompt', 'org'),
                ('tree_v2', 'context_test', 'context', 'org', NULL, 'context/test.md', 'blob_context_v2', 'org'),
                ('tree_v2', 'metaprompt_test', 'metaprompt', 'org', NULL, 'META_PROMPT.md', 'blob_metaprompt', 'org'),
                ('tree_v3', 'context_test', 'context', 'org', NULL, 'context/test.md', 'blob_context_v3', 'org');
            INSERT INTO commits (
                commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version, created_at
            ) VALUES
                ('commit_v1', 'org', 'org_test', NULL, 'tree_v1', NULL, 1, '2026-07-16T00:00:00Z'),
                ('commit_v2', 'org', 'org_test', NULL, 'tree_v2', 'commit_v1', 2, '2026-07-17T00:00:00Z'),
                ('commit_v3', 'org', 'org_test', NULL, 'tree_v3', 'commit_v2', 3, '2026-07-18T00:00:00Z');
            INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id, commit_id)
            VALUES ('ref_test', 'refs/heads/main', 'org', 'org_test', NULL, 'commit_v3');
            INSERT INTO metaprompts (
                metaprompt_id, org_id, project_id, scope, status, content_hash, body
            ) VALUES (
                'metaprompt_test', 'org_test', NULL, 'org', 'active',
                'sha256:obsolete', '# Obsolete MCP bootstrap'
            );

            INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, resource_scope, resource_kind,
                base_commit_id, target_id, path, status, daemon_installation_id
            ) VALUES
                ('draft_context', 'project_test', 'user_test', 'Keep context', 'org', 'context',
                 'commit_v1', 'context_test', NULL, 'open', 'daemon_test'),
                ('draft_metaprompt', 'project_test', 'user_test', 'Remove metaprompt', 'org', 'metaprompt',
                 'commit_v2', 'metaprompt_test', NULL, 'submitted', 'daemon_test');
            INSERT INTO draft_operations (
                operation_id, draft_id, action, resource_scope, resource_kind, target_id, content
            ) VALUES
                ('operation_context', 'draft_context', 'update', 'org', 'context', 'context_test',
                 '{"kind":"context","content":"# Context draft"}'),
                ('operation_metaprompt', 'draft_metaprompt', 'update', 'org', 'metaprompt', 'metaprompt_test',
                 '{"kind":"metaprompt","content":"# Old draft"}');
            INSERT INTO draft_events (
                event_id, draft_id, project_id, event_type, version, daemon_installation_id
            ) VALUES
                ('event_metaprompt', 'draft_metaprompt', 'project_test', 'submitted', 1, 'daemon_test');
            INSERT INTO draft_conflicts (draft_id, base_commit_id, current_commit_id)
            VALUES ('draft_context', 'commit_v1', 'commit_v3');
            INSERT INTO reviews (
                review_id, draft_id, project_id, author_user_id, title, status
            ) VALUES (
                'review_metaprompt', 'draft_metaprompt', 'project_test', 'user_test',
                'Old review', 'merged'
            );
            INSERT INTO review_comments (comment_id, review_id, author_user_id, body)
            VALUES ('comment_metaprompt', 'review_metaprompt', 'user_test', 'Old comment');
            INSERT INTO review_merges (merge_id, review_id, commit_id, applied_operation_count)
            VALUES ('merge_metaprompt', 'review_metaprompt', 'commit_v2', 1);
            "##,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260722000100_remove_metaprompt.sql"
        ))
        .await
        .unwrap();

    let table_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.metaprompts') IS NOT NULL")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert!(!table_exists);
    for table in ["drafts", "draft_operations", "tree_entries"] {
        let query = format!("SELECT COUNT(*) FROM {table} WHERE resource_kind = 'metaprompt'");
        let count: i64 = sqlx::query_scalar(&query)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} retained Metaprompt data");
    }

    let new_head: String =
        sqlx::query_scalar("SELECT commit_id FROM refs WHERE ref_id = 'ref_test'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_ne!(new_head, "commit_v3");
    let (head_version, head_tree_id, new_parent): (i64, String, Option<String>) = sqlx::query_as(
        "SELECT version, tree_id, parent_commit_id FROM commits WHERE commit_id = $1",
    )
    .bind(&new_head)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(head_version, 3);
    assert_eq!(head_tree_id, "tree_v3");
    let new_parent = new_parent.unwrap();
    assert_ne!(new_parent, "commit_v2");
    let (parent_version, new_grandparent): (i64, Option<String>) =
        sqlx::query_as("SELECT version, parent_commit_id FROM commits WHERE commit_id = $1")
            .bind(&new_parent)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(parent_version, 2);
    let new_grandparent = new_grandparent.unwrap();
    assert_ne!(new_grandparent, "commit_v1");
    let grandparent_version: i64 =
        sqlx::query_scalar("SELECT version FROM commits WHERE commit_id = $1")
            .bind(&new_grandparent)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(grandparent_version, 1);

    let context_base: String =
        sqlx::query_scalar("SELECT base_commit_id FROM drafts WHERE draft_id = 'draft_context'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(context_base, new_grandparent);
    let conflict_commits: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT base_commit_id, current_commit_id FROM draft_conflicts WHERE draft_id = 'draft_context'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        conflict_commits,
        (Some(new_grandparent), Some(new_head.clone()))
    );

    let old_commit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM commits WHERE commit_id IN ('commit_v1', 'commit_v2', 'commit_v3')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(old_commit_count, 0);
    let head_entry_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT entry.resource_kind
         FROM commits AS commit
         JOIN tree_entries AS entry ON entry.tree_id = commit.tree_id
         WHERE commit.commit_id = $1
         ORDER BY entry.resource_kind",
    )
    .bind(&new_head)
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(head_entry_kinds, vec!["context"]);
    let obsolete_blob_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM blobs WHERE blob_id = 'blob_metaprompt'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(obsolete_blob_count, 0);

    let rejected = sqlx::query(
        "INSERT INTO drafts (
            draft_id, project_id, author_user_id, title, resource_scope, resource_kind,
            status, daemon_installation_id
         ) VALUES (
            'draft_rejected', 'project_test', 'user_test', 'Rejected', 'project',
            'metaprompt', 'open', 'daemon_test'
         )",
    )
    .execute(&postgres.pool)
    .await;
    assert!(rejected.is_err());
    postgres.shutdown().await;
}
