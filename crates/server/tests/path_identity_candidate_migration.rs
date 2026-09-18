mod common;

use sqlx::Executor;

#[tokio::test]
async fn path_identity_migration_invalidates_only_active_legacy_org_candidates() {
    let postgres = common::postgres_without_migrations().await;
    postgres
        .pool
        .execute(
            r#"
            CREATE TABLE drafts (
                draft_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                resource_scope TEXT NOT NULL,
                target_id TEXT,
                path TEXT,
                status TEXT NOT NULL,
                version BIGINT NOT NULL
            );
            CREATE TABLE draft_operations (
                operation_id TEXT PRIMARY KEY,
                draft_id TEXT NOT NULL,
                action TEXT NOT NULL,
                ordinal BIGINT NOT NULL
            );
            CREATE TABLE draft_reconciliation_candidates (
                candidate_id TEXT PRIMARY KEY,
                draft_id TEXT NOT NULL,
                invalidated_at TIMESTAMPTZ
            );
            CREATE TABLE draft_events (
                server_sequence BIGSERIAL NOT NULL UNIQUE,
                event_id TEXT PRIMARY KEY,
                draft_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                version BIGINT NOT NULL,
                daemon_installation_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            INSERT INTO drafts (
                draft_id, project_id, resource_scope, target_id, path, status, version
            ) VALUES
                ('legacy_open', 'project_a', 'org', NULL, 'context/old.md', 'open', 7),
                ('legacy_submitted', 'project_a', 'org', NULL, 'context/submitted.md', 'submitted', 8),
                ('create_first', 'project_a', 'org', NULL, 'context/new.md', 'open', 2),
                ('stable_id', 'project_a', 'org', 'mem_stable', 'context/stable.md', 'open', 3),
                ('project_scope', 'project_a', 'project', NULL, 'context/project.md', 'open', 4),
                ('terminal', 'project_a', 'org', NULL, 'context/terminal.md', 'merged', 5),
                ('already_invalid', 'project_a', 'org', NULL, 'context/invalid.md', 'open', 6);

            INSERT INTO draft_operations (operation_id, draft_id, action, ordinal) VALUES
                ('op_legacy_open', 'legacy_open', 'update', 1),
                ('op_legacy_submitted', 'legacy_submitted', 'rename', 1),
                ('op_create_first', 'create_first', 'create', 1),
                ('op_stable_id', 'stable_id', 'update', 1),
                ('op_project_scope', 'project_scope', 'update', 1),
                ('op_terminal', 'terminal', 'update', 1),
                ('op_already_invalid', 'already_invalid', 'update', 1);

            INSERT INTO draft_reconciliation_candidates (
                candidate_id, draft_id, invalidated_at
            ) VALUES
                ('candidate_legacy_open', 'legacy_open', NULL),
                ('candidate_legacy_submitted', 'legacy_submitted', NULL),
                ('candidate_create_first', 'create_first', NULL),
                ('candidate_stable_id', 'stable_id', NULL),
                ('candidate_project_scope', 'project_scope', NULL),
                ('candidate_terminal', 'terminal', NULL),
                ('candidate_already_invalid', 'already_invalid', '2026-08-19T00:00:00Z');
            "#,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260820000200_invalidate_path_only_org_candidates.sql"
        ))
        .await
        .unwrap();

    let candidates: Vec<(String, bool)> = sqlx::query_as(
        "SELECT candidate_id, invalidated_at IS NOT NULL
         FROM draft_reconciliation_candidates
         ORDER BY candidate_id",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        candidates,
        vec![
            ("candidate_already_invalid".to_owned(), true),
            ("candidate_create_first".to_owned(), false),
            ("candidate_legacy_open".to_owned(), true),
            ("candidate_legacy_submitted".to_owned(), true),
            ("candidate_project_scope".to_owned(), false),
            ("candidate_stable_id".to_owned(), false),
            ("candidate_terminal".to_owned(), false),
        ]
    );

    let refresh_events: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT draft_id, event_type, version
         FROM draft_events
         ORDER BY draft_id",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        refresh_events,
        vec![
            ("legacy_open".to_owned(), "updated".to_owned(), 7),
            ("legacy_submitted".to_owned(), "updated".to_owned(), 8),
        ]
    );
    postgres.shutdown().await;
}
