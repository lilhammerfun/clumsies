mod common;

use server::maintenance::project_authority::{MigrationMode, migrate_project_authority};
use sha2::{Digest, Sha256};
use sqlx::Executor;

#[tokio::test]
async fn migration_flattens_effective_project_memory_into_org_drafts() {
    let postgres = common::migrated_postgres().await;
    let pool = postgres.pool.clone();
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Project Authority Migration",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Legacy Project",
    )
    .await;
    let selected_org_id = server::app::memory::service::create_org_context(
        &pool,
        &bootstrap.org_id,
        "shared/selected.md",
        "# Selected Organization Memory",
    )
    .await
    .unwrap();
    server::app::memory::service::select_org_resource_for_project(
        &pool,
        &bootstrap.project_id,
        &selected_org_id,
    )
    .await
    .unwrap();

    // Recreate the state seen during a rolling upgrade: the NOT VALID guards
    // are installed around rows written by an older release.
    postgres
        .pool
        .execute(
            "ALTER TABLE resources DROP CONSTRAINT resources_no_active_project_authority;
             ALTER TABLE drafts DROP CONSTRAINT drafts_no_active_project_authority;",
        )
        .await
        .unwrap();

    for (id, path, description, body) in [
        (
            "mem_project_a",
            "project/a.md",
            "Authority A",
            "# Authority A",
        ),
        (
            "mem_project_b",
            "project/b.md",
            "Authority B",
            "# Authority B",
        ),
    ] {
        sqlx::query(
            "INSERT INTO resources (
                resource_id, org_id, project_id, scope, resource_kind, path, name,
                status, revision, content_hash, body, description
             ) VALUES ($1, $2, $3, 'project', 'memory', $4, $4,
                       'active', 1, $5, $6, $7)",
        )
        .bind(id)
        .bind(&bootstrap.org_id)
        .bind(&bootstrap.project_id)
        .bind(path)
        .bind(sha256(body))
        .bind(body)
        .bind(description)
        .execute(&postgres.pool)
        .await
        .unwrap();
    }

    let previous_commit_id: String = sqlx::query_scalar(
        "SELECT commit_id
         FROM refs
         WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(&bootstrap.project_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    let previous_tree_id: String =
        sqlx::query_scalar("SELECT tree_id FROM commits WHERE commit_id = $1")
            .bind(&previous_commit_id)
            .fetch_one(&postgres.pool)
            .await
            .unwrap();
    postgres
        .pool
        .execute(
            "INSERT INTO blobs (blob_id, content) VALUES
                ('blob_project_a', '# Authority A'),
                ('blob_project_b', '# Authority B');
             INSERT INTO trees (tree_id) VALUES ('tree_legacy_project');",
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tree_entries (
            tree_id, item_id, resource_kind, scope, project_id, path,
            blob_id, source, description
         )
         SELECT 'tree_legacy_project', item_id, resource_kind, scope, project_id,
                path, blob_id, source, description
         FROM tree_entries WHERE tree_id = $1",
    )
    .bind(&previous_tree_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tree_entries (
            tree_id, item_id, resource_kind, scope, project_id, path,
            blob_id, source, description
         ) VALUES
            ('tree_legacy_project', 'mem_project_a', 'memory', 'project', $1,
             'project/a.md', 'blob_project_a', 'project', 'Authority A'),
            ('tree_legacy_project', 'mem_project_b', 'memory', 'project', $1,
             'project/b.md', 'blob_project_b', 'project', 'Authority B')",
    )
    .bind(&bootstrap.project_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commits (
            commit_id, scope, org_id, project_id, tree_id, parent_commit_id, version
         ) VALUES (
            'cmt_legacy_project', 'project', $1, $2, 'tree_legacy_project', $3, 2
         )",
    )
    .bind(&bootstrap.org_id)
    .bind(&bootstrap.project_id)
    .bind(&previous_commit_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE refs SET commit_id = 'cmt_legacy_project'
         WHERE scope = 'project' AND project_id = $1 AND ref_name = 'refs/heads/main'",
    )
    .bind(&bootstrap.project_id)
    .execute(&postgres.pool)
    .await
    .unwrap();

    sqlx::query(
        r##"
        INSERT INTO drafts (
            draft_id, project_id, author_user_id, title, description,
            resource_scope, resource_kind, base_commit_id, target_id, path,
            status, version, daemon_installation_id, created_at
        ) VALUES
            ('drf_rename_a', $1, $2, 'Rename A', '', 'project', 'memory',
             'cmt_legacy_project', 'mem_project_a', NULL, 'open', 1, 'daemon_old',
             '2026-08-01T00:00:00Z'),
            ('drf_update_b', $1, $2, 'Update B', '', 'project', 'memory',
             'cmt_legacy_project', 'mem_project_b', NULL, 'open', 1, 'daemon_old',
             '2026-08-01T00:00:01Z'),
            ('drf_create_c', $1, $2, 'Create C', '', 'project', 'memory',
             'cmt_legacy_project', NULL, 'project/c.md', 'open', 1, 'daemon_old',
             '2026-08-01T00:00:02Z'),
            ('drf_delete_c', $1, $2, 'Delete C', '', 'project', 'memory',
             'cmt_legacy_project', 'draft_local_c', NULL, 'open', 1, 'daemon_old',
             '2026-08-01T00:00:03Z'),
            ('drf_create_d', $1, $2, 'Create D', '', 'project', 'memory',
             'cmt_legacy_project', NULL, 'project/d.md', 'open', 1, 'daemon_old',
             '2026-08-01T00:00:04Z'),
            ('drf_shadow_selected', $1, $2, 'Replace selected Org path', '',
             'project', 'memory', 'cmt_legacy_project', NULL,
             'shared/selected.md', 'open', 1, 'daemon_old',
             '2026-08-01T00:00:05Z')
        "##,
    )
    .bind(&bootstrap.project_id)
    .bind(&bootstrap.user_id)
    .execute(&postgres.pool)
    .await
    .unwrap();
    sqlx::query(
        r##"
        INSERT INTO draft_operations (
            operation_id, draft_id, action, resource_scope, resource_kind,
            target_id, path, new_path, content, ordinal
        ) VALUES
            ('dop_rename_a', 'drf_rename_a', 'rename', 'project', 'memory',
             'mem_project_a', NULL, 'project/renamed-a.md', NULL, 1),
            ('dop_update_b', 'drf_update_b', 'update', 'project', 'memory',
             'mem_project_b', NULL, NULL,
             '{"description":"Updated B","content":"# Updated B"}', 1),
            ('dop_create_c', 'drf_create_c', 'create', 'project', 'memory',
             'draft_local_c', 'project/c.md', NULL,
             '{"description":"Created C","content":"# Created C"}', 1),
            ('dop_delete_c', 'drf_delete_c', 'delete', 'project', 'memory',
             'draft_local_c', NULL, NULL, NULL, 1),
            ('dop_create_d', 'drf_create_d', 'create', 'project', 'memory',
             'draft_local_d', 'project/d.md', NULL,
             '{"description":"Created D","content":"# Created D"}', 1),
            ('dop_shadow_selected', 'drf_shadow_selected', 'create', 'project', 'memory',
             'draft_shadow_selected', 'shared/selected.md', NULL,
             '{"description":"Project refinement","content":"# Refined selected memory"}', 1);
        "##,
    )
    .execute(&postgres.pool)
    .await
    .unwrap();

    postgres
        .pool
        .execute(
            "ALTER TABLE resources
                 ADD CONSTRAINT resources_no_active_project_authority
                 CHECK (scope <> 'project' OR status <> 'active') NOT VALID;
             ALTER TABLE drafts
                 ADD CONSTRAINT drafts_no_active_project_authority
                 CHECK (resource_scope <> 'project' OR status NOT IN ('open', 'submitted'))
                 NOT VALID;",
        )
        .await
        .unwrap();

    let dry_run = migrate_project_authority(&postgres.pool, MigrationMode::DryRun)
        .await
        .unwrap();
    assert!(dry_run.ready, "blockers: {:?}", dry_run.blockers);
    assert!(!dry_run.applied);
    assert_eq!(dry_run.legacy_authority_count, 2);
    assert_eq!(dry_run.legacy_active_draft_count, 6);
    assert_eq!(dry_run.replacement_draft_count, 4);

    let stale_plan = migrate_project_authority(
        &postgres.pool,
        MigrationMode::Apply {
            expected_plan_hash: "sha256:stale",
        },
    )
    .await
    .unwrap_err();
    assert!(stale_plan.to_string().contains("plan changed"));
    let still_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resources WHERE scope = 'project' AND status = 'active'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(still_active, 2);

    let applied = migrate_project_authority(
        &postgres.pool,
        MigrationMode::Apply {
            expected_plan_hash: &dry_run.plan_hash,
        },
    )
    .await
    .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.plan_hash, dry_run.plan_hash);

    let active_project_resources: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resources WHERE scope = 'project' AND status = 'active'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(active_project_resources, 0);
    let active_project_drafts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drafts
         WHERE resource_scope = 'project' AND status IN ('open', 'submitted')",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(active_project_drafts, 0);

    let replacements: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT operation.path,
                operation.content->>'description',
                operation.content->>'content'
         FROM drafts draft
         JOIN draft_operations operation ON operation.draft_id = draft.draft_id
         WHERE draft.project_id = $1
           AND draft.resource_scope = 'org'
           AND draft.daemon_installation_id = 'server-project-authority-migration-v1'
           AND operation.action = 'create'
         ORDER BY operation.path",
    )
    .bind(&bootstrap.project_id)
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        replacements,
        vec![
            (
                "project/b.md".to_owned(),
                "Updated B".to_owned(),
                "# Updated B".to_owned(),
            ),
            (
                "project/d.md".to_owned(),
                "Created D".to_owned(),
                "# Created D".to_owned(),
            ),
            (
                "project/renamed-a.md".to_owned(),
                "Authority A".to_owned(),
                "# Authority A".to_owned(),
            ),
        ]
    );
    let update_replacement: (String, String, String) = sqlx::query_as(
        "SELECT operation.target_id,
                operation.content->>'description',
                operation.content->>'content'
         FROM drafts draft
         JOIN draft_operations operation ON operation.draft_id = draft.draft_id
         WHERE draft.project_id = $1
           AND draft.resource_scope = 'org'
           AND draft.daemon_installation_id = 'server-project-authority-migration-v1'
           AND operation.action = 'update'",
    )
    .bind(&bootstrap.project_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(
        update_replacement,
        (
            selected_org_id.clone(),
            "Project refinement".to_owned(),
            "# Refined selected memory".to_owned(),
        )
    );

    let project_entries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM refs ref
         JOIN commits commit ON commit.commit_id = ref.commit_id
         JOIN tree_entries entry ON entry.tree_id = commit.tree_id
         WHERE ref.scope = 'project'
           AND ref.project_id = $1
           AND ref.ref_name = 'refs/heads/main'
           AND entry.scope = 'project'
           AND entry.resource_kind = 'memory'",
    )
    .bind(&bootstrap.project_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(project_entries, 0);
    let selected_entries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM refs ref
         JOIN commits commit ON commit.commit_id = ref.commit_id
         JOIN tree_entries entry ON entry.tree_id = commit.tree_id
         WHERE ref.scope = 'project'
           AND ref.project_id = $1
           AND ref.ref_name = 'refs/heads/main'
           AND entry.item_id = $2
           AND entry.scope = 'org'",
    )
    .bind(&bootstrap.project_id)
    .bind(&selected_org_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(selected_entries, 1);

    let constraints: Vec<(String, bool)> = sqlx::query_as(
        "SELECT conname, convalidated
         FROM pg_constraint
         WHERE conname IN (
             'resources_no_active_project_authority',
             'drafts_no_active_project_authority'
         )
         ORDER BY conname",
    )
    .fetch_all(&postgres.pool)
    .await
    .unwrap();
    assert!(constraints.iter().all(|(_, validated)| *validated));

    let second_dry_run = migrate_project_authority(&postgres.pool, MigrationMode::DryRun)
        .await
        .unwrap();
    assert!(second_dry_run.ready);
    assert_eq!(second_dry_run.legacy_authority_count, 0);
    assert_eq!(second_dry_run.legacy_active_draft_count, 0);
    assert_eq!(second_dry_run.replacement_draft_count, 0);
    postgres.shutdown().await;
}

fn sha256(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}
