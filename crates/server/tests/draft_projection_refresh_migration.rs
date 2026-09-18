mod common;

use sqlx::Executor;

#[tokio::test]
async fn active_drafts_emit_one_projection_refresh_event() {
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
            INSERT INTO trees (tree_id) VALUES ('tree_base');
            INSERT INTO commits (
                commit_id, scope, org_id, project_id, tree_id, version
            ) VALUES (
                'commit_base', 'project', 'org_test', 'project_test', 'tree_base', 1
            );
            INSERT INTO drafts (
                draft_id, project_id, author_user_id, title, resource_scope,
                resource_kind, base_commit_id, path, status, version,
                daemon_installation_id
            ) VALUES
                (
                    'draft_open', 'project_test', 'user_test', 'Open', 'project',
                    'context', 'commit_base', 'context/open.md', 'open', 3,
                    'daemon_test'
                ),
                (
                    'draft_discarded', 'project_test', 'user_test', 'Discarded',
                    'project', 'context', 'commit_base', 'context/discarded.md',
                    'discarded', 4, 'daemon_test'
                );
            INSERT INTO draft_events (
                event_id, draft_id, project_id, event_type, version,
                daemon_installation_id
            ) VALUES (
                'event_created', 'draft_open', 'project_test', 'created', 1,
                'daemon_test'
            );
            "#,
        )
        .await
        .unwrap();

    let migration = include_str!("../migrations/20260722000400_refresh_draft_projections.sql");
    postgres.pool.execute(migration).await.unwrap();
    postgres.pool.execute(migration).await.unwrap();

    let refresh: (String, i64, Option<String>) = sqlx::query_as(
        "SELECT event_type, version, daemon_installation_id
         FROM draft_events
         WHERE event_id = 'evt_projection_refresh_' || md5('20260722000400:draft_open')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(refresh, ("updated".to_owned(), 3, None));

    let open_refresh_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM draft_events
         WHERE event_id = 'evt_projection_refresh_' || md5('20260722000400:draft_open')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(open_refresh_count, 1);

    let discarded_refresh_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM draft_events
         WHERE event_id = 'evt_projection_refresh_' || md5('20260722000400:draft_discarded')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(discarded_refresh_count, 0);
    postgres.shutdown().await;
}
