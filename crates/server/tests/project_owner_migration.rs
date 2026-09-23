//! Legacy project membership receives one protected owner without demoting maintainers.

mod common;

use server::app::project;
use server::app::project::dto::CreateProjectRequest;
use sqlx::Executor;

#[tokio::test]
async fn migration_prefers_creators_and_recovers_ownerless_projects() {
    let postgres = common::migrated_postgres().await;
    let pool = &postgres.pool;
    let installation = common::initialize_installation(
        pool.clone(),
        "Legacy roles",
        "owner@example.com",
        "Owner",
        "oidc-subject-owner",
        "Legacy",
    )
    .await;
    let owner = common::owner_principal(pool).await;
    sqlx::query("INSERT INTO users (user_id, email, role, status) VALUES ('creator', 'creator@example.com', 'member', 'active')")
        .execute(pool).await.unwrap();
    let creator = common::principal(pool, "creator").await;
    let created = project::create_project_from_request(
        pool,
        &creator,
        CreateProjectRequest {
            name: "Recorded creator".into(),
            description: None,
        },
        "creation-key",
    )
    .await
    .unwrap();
    let orphan = project::create_project(pool, &owner, "Orphan", "")
        .await
        .unwrap();
    let disabled = project::create_project(pool, &owner, "Disabled admin", "")
        .await
        .unwrap();

    // Restore the pre-upgrade constraints and legacy membership states.
    pool.execute("DROP INDEX project_members_one_owner_idx;
        UPDATE project_members SET role = 'admin';
        ALTER TABLE project_members DROP CONSTRAINT project_members_role_check;
        ALTER TABLE project_members ADD CONSTRAINT project_members_role_check CHECK (role IN ('admin', 'member'));").await.unwrap();
    sqlx::query("INSERT INTO project_members (project_id, user_id, role, joined_at) VALUES ($1, $2, 'admin', now() - interval '1 day')")
        .bind(&created.project_id).bind(&owner.user_id).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM project_members WHERE project_id = $1")
        .bind(&orphan)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (user_id, email, role, status) VALUES ('disabled', 'disabled@example.com', 'member', 'disabled')")
        .execute(pool).await.unwrap();
    sqlx::query("UPDATE project_members SET user_id = 'disabled' WHERE project_id = $1")
        .bind(&disabled)
        .execute(pool)
        .await
        .unwrap();
    pool.execute(include_str!(
        "../migrations/20260923000100_add_project_owners.sql"
    ))
    .await
    .unwrap();

    for (project_id, expected_owner) in [
        (&installation.project_id, &owner.user_id),
        (&created.project_id, &creator.user_id),
        (&orphan, &owner.user_id),
        (&disabled, &owner.user_id),
    ] {
        let owners: Vec<String> = sqlx::query_scalar(
            "SELECT user_id FROM project_members WHERE project_id = $1 AND role = 'owner'",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(owners, vec![expected_owner.clone()]);
    }
    let role: String = sqlx::query_scalar(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&created.project_id)
    .bind(&owner.user_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(role, "admin", "Other maintainers keep their role");
    let duplicate = sqlx::query(
        "UPDATE project_members SET role = 'owner' WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&created.project_id)
    .bind(&owner.user_id)
    .execute(pool)
    .await;
    assert!(duplicate.is_err(), "The database rejects a second owner");
    postgres.shutdown().await;
}
