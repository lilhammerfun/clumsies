//! Migration of structured legacy workflow content into canonical Markdown.

mod common;

use sqlx::Row;

#[tokio::test]
async fn structured_workflows_and_drafts_migrate_to_markdown() {
    let postgres = common::postgres_without_migrations().await;
    for migration in [
        include_str!("../migrations/20260708000100_create_server_schema.sql"),
        include_str!("../migrations/20260714000100_add_user_avatar.sql"),
        include_str!("../migrations/20260715000100_add_draft_conflicts.sql"),
        include_str!("../migrations/20260716000100_add_server_installation_setup.sql"),
        include_str!("../migrations/20260716000200_add_web_admin_sessions.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&postgres.pool)
            .await
            .unwrap();
    }

    sqlx::raw_sql(
        "INSERT INTO orgs (org_id, name) VALUES ('org_migration', 'Migration');
         INSERT INTO users (user_id, email, display_name, role, status)
         VALUES ('usr_migration', 'migration@example.com', 'Migration', 'owner', 'active');
         INSERT INTO projects (project_id, org_id, name)
         VALUES ('prj_migration', 'org_migration', 'Migration');
         INSERT INTO project_org_selection_states (project_id) VALUES ('prj_migration');
         INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id)
         VALUES
           ('ref_org_migration', 'refs/heads/main', 'org', 'org_migration', NULL),
           ('ref_project_migration', 'refs/heads/main', 'project', 'org_migration', 'prj_migration');
         INSERT INTO resources (
             resource_id, org_id, project_id, scope, resource_kind, path, name,
             status, content_hash, body
         ) VALUES (
             'wfl_migration', 'org_migration', 'prj_migration', 'project', 'workflow',
             'workflow/RELEASE.md', 'Release', 'active', 'sha256:old', 'Publish safely.'
         );
         INSERT INTO drafts (
             draft_id, project_id, author_user_id, title, resource_scope,
             resource_kind, path, status, daemon_installation_id
         ) VALUES (
             'drf_migration', 'prj_migration', 'usr_migration', 'Release', 'project',
             'workflow', 'workflow/RELEASE.md', 'open', 'ins_migration'
         );
         INSERT INTO draft_operations (
             operation_id, draft_id, action, resource_scope, resource_kind, path, content
         ) VALUES (
             'dop_migration', 'drf_migration', 'update', 'project', 'workflow',
             'workflow/RELEASE.md',
             '{\"kind\":\"workflow\",\"name\":\"RELEASE\",\"description\":\"Publish safely.\",\"steps\":[{\"rule_id\":\"rul_release\",\"body\":null},{\"rule_id\":null,\"body\":\"Publish the release.\"}]}'
         );
         INSERT INTO workflow_steps (resource_id, step_order, rule_id, body)
         VALUES
           ('wfl_migration', 1, NULL, 'Run focused tests.'),
           ('wfl_migration', 2, NULL, 'Publish the release.');
         INSERT INTO blobs (blob_id, content) VALUES (
             'blob_old_workflow',
             '{\"format\":\"clumsies.workflow.v1\",\"content\":{\"name\":\"Release\",\"description\":\"Publish safely.\",\"steps\":[{\"order\":1,\"rule_id\":null,\"body\":\"Run focused tests.\"}]}}'
         );
         INSERT INTO trees (tree_id) VALUES ('tree_old_workflow');
         INSERT INTO commits (
             commit_id, scope, org_id, project_id, tree_id, version
         ) VALUES (
             'commit_old_workflow', 'project', 'org_migration', 'prj_migration',
             'tree_old_workflow', 1
         );
         INSERT INTO tree_entries (
             tree_id, item_id, resource_kind, scope, project_id, path, blob_id, source
         ) VALUES (
             'tree_old_workflow', 'wfl_migration', 'workflow', 'project', 'prj_migration',
             'workflow/RELEASE.md', 'blob_old_workflow', 'project'
         );
         UPDATE refs SET commit_id = 'commit_old_workflow'
         WHERE ref_id = 'ref_project_migration';",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260720000100_flatten_workflow_content.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260720000200_flatten_workflow_draft_content.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();

    let resource_expected =
        "# Release\n\nPublish safely.\n\n1. Run focused tests.\n2. Publish the release.";
    let migrated = sqlx::query("SELECT body, content_hash FROM resources WHERE resource_id = $1")
        .bind("wfl_migration")
        .fetch_one(&postgres.pool)
        .await
        .unwrap();
    assert_eq!(migrated.get::<String, _>("body"), resource_expected);
    assert!(
        migrated
            .get::<String, _>("content_hash")
            .starts_with("sha256:")
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('workflow_steps')::text")
            .fetch_one(&postgres.pool)
            .await
            .unwrap(),
        None
    );

    sqlx::raw_sql(include_str!(
        "../migrations/20260720000200_flatten_workflow_draft_content.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260722000100_remove_metaprompt.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260722000200_flatten_markdown_memory.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();
    let current = current_project_ref(&postgres.pool).await;
    assert_ne!(current, "commit_old_workflow");
    assert_eq!(
        current_workflow_blob(&postgres.pool).await,
        "# Release\n\nPublish safely.\n\n1. Run focused tests."
    );

    let migrated_draft: serde_json::Value = sqlx::query_scalar(
        "SELECT content FROM draft_operations WHERE operation_id = 'dop_migration'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        migrated_draft,
        serde_json::json!({
            "kind": "workflow",
            "content": "# RELEASE\n\nPublish safely.\n\n1. Apply rule `rul_release`.\n2. Publish the release."
        })
    );

    let rerun_draft: serde_json::Value = sqlx::query_scalar(
        "SELECT content FROM draft_operations WHERE operation_id = 'dop_migration'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(rerun_draft, migrated_draft);
    let old_commit_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM commits WHERE commit_id = 'commit_old_workflow')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert!(!old_commit_exists);
    postgres.shutdown().await;
}

async fn current_project_ref(pool: &sqlx::PgPool) -> String {
    sqlx::query_scalar(
        "SELECT commit_id FROM refs
         WHERE scope = 'project' AND project_id = 'prj_migration'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn current_workflow_blob(pool: &sqlx::PgPool) -> String {
    sqlx::query_scalar(
        "SELECT blob.content
         FROM refs ref
         JOIN commits commit ON commit.commit_id = ref.commit_id
         JOIN tree_entries entry ON entry.tree_id = commit.tree_id
         JOIN blobs blob ON blob.blob_id = entry.blob_id
         WHERE ref.scope = 'project'
           AND ref.project_id = 'prj_migration'
           AND entry.resource_kind = 'workflow'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
