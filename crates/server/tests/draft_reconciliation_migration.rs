//! Migration of legacy proposal state into revisioned reconciliation storage.

mod common;

use sqlx::Executor;

#[tokio::test]
async fn conflicted_drafts_migrate_once_to_separate_coordination_state() {
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
    ] {
        postgres.pool.execute(migration).await.unwrap();
    }

    postgres
        .pool
        .execute(
            r#"
            INSERT INTO orgs (org_id, name) VALUES ('org_test', 'Test');
            INSERT INTO projects (project_id, org_id, name)
            VALUES ('project_test', 'org_test', 'Project');
            INSERT INTO users (user_id, email, display_name, role, status)
            VALUES ('user_test', 'owner@example.com', 'Owner', 'owner', 'active');
            INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, resource_scope,
                resource_kind, path, status, version, daemon_installation_id
            ) VALUES (
                'draft_conflicted', 'project_test', 'user_test', 'Legacy conflict',
                'project', 'context', 'context/legacy.md', 'conflicted', 4, 'daemon_test'
            );
            INSERT INTO draft_events (
                event_id, draft_id, project_id, event_type, version
            ) VALUES (
                'event_conflicted', 'draft_conflicted', 'project_test', 'conflicted', 4
            );
            INSERT INTO reviews (
                review_id, draft_id, project_id, author_user_id, title, status, version
            ) VALUES (
                'review_conflicted', 'draft_conflicted', 'project_test', 'user_test',
                'Legacy conflict', 'approved', 2
            );
            INSERT INTO draft_conflicts (draft_id, base_commit_id, current_commit_id)
            VALUES ('draft_conflicted', NULL, NULL);
            "#,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260722000300_add_draft_reconciliation.sql"
        ))
        .await
        .unwrap();

    let status: String =
        sqlx::query_scalar("SELECT status FROM drafts WHERE draft_id = 'draft_conflicted'")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(status, "submitted");
    let event_type: String = sqlx::query_scalar(
        "SELECT event_type FROM draft_events WHERE event_id = 'event_conflicted'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(event_type, "updated");

    let old_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('draft_conflicts')::text")
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    assert_eq!(old_table, None);
    for table in [
        "draft_reconciliation_candidates",
        "draft_revisions",
        "draft_rebases",
    ] {
        let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(table)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
        assert_eq!(exists.as_deref(), Some(table));
    }
    let approved_result_hash: Option<String> = sqlx::query_scalar(
        "SELECT approved_result_hash FROM reviews WHERE review_id = 'review_conflicted'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(approved_result_hash, None);

    assert!(
        sqlx::query("UPDATE drafts SET status = 'conflicted' WHERE draft_id = 'draft_conflicted'")
            .execute(&postgres.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE draft_events SET event_type = 'conflicted' WHERE event_id = 'event_conflicted'",
        )
        .execute(&postgres.pool)
        .await
        .is_err()
    );
    postgres.shutdown().await;
}
