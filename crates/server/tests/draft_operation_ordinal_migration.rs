mod common;

use sqlx::Executor;

#[tokio::test]
async fn draft_operation_ordinal_migration_freezes_legacy_order_per_draft() {
    let postgres = common::postgres_without_migrations().await;
    postgres
        .pool
        .execute(
            r#"
            CREATE TABLE drafts (
                draft_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL,
                version BIGINT NOT NULL
            );
            CREATE TABLE draft_operations (
                operation_id TEXT PRIMARY KEY,
                draft_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
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
            INSERT INTO drafts (draft_id, project_id, status, version) VALUES
                ('draft_a', 'project_a', 'open', 7),
                ('draft_b', 'project_b', 'merged', 4);
            INSERT INTO draft_operations (operation_id, draft_id, created_at) VALUES
                ('dop_middle', 'draft_a', '2026-08-19T23:59:59Z'),
                ('dop_z', 'draft_a', '2026-08-20T00:00:00Z'),
                ('dop_a', 'draft_a', '2026-08-20T00:00:00Z'),
                ('dop_other', 'draft_b', '2026-08-20T00:00:00Z');
            "#,
        )
        .await
        .unwrap();

    postgres
        .pool
        .execute(include_str!(
            "../migrations/20260820000100_add_draft_operation_ordinals.sql"
        ))
        .await
        .unwrap();

    let operations: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT draft_id, operation_id, ordinal
         FROM draft_operations
         ORDER BY draft_id, ordinal",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        operations,
        vec![
            ("draft_a".to_owned(), "dop_middle".to_owned(), 1),
            ("draft_a".to_owned(), "dop_a".to_owned(), 2),
            ("draft_a".to_owned(), "dop_z".to_owned(), 3),
            ("draft_b".to_owned(), "dop_other".to_owned(), 1),
        ]
    );

    let refresh_events: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT draft_id, event_type, version
         FROM draft_events
         ORDER BY server_sequence",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        refresh_events,
        vec![("draft_a".to_owned(), "updated".to_owned(), 7)],
        "active Draft projections must reload the canonical ordinal order"
    );

    let duplicate = sqlx::query(
        "INSERT INTO draft_operations (operation_id, draft_id, ordinal)
         VALUES ('dop_duplicate', 'draft_a', 1)",
    )
    .execute(&postgres.pool)
    .await
    .unwrap_err();
    assert!(
        duplicate
            .to_string()
            .contains("draft_operations_draft_ordinal_unique"),
        "unexpected duplicate-ordinal error: {duplicate}"
    );

    let missing = sqlx::query(
        "INSERT INTO draft_operations (operation_id, draft_id)
         VALUES ('dop_missing', 'draft_c')",
    )
    .execute(&postgres.pool)
    .await
    .unwrap_err();
    assert!(
        missing.to_string().contains("ordinal"),
        "unexpected missing-ordinal error: {missing}"
    );
    postgres.shutdown().await;
}
