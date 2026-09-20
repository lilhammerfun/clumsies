//! Resource operations enforce authorization even when HTTP middleware is absent.

use server::app::project;
use server::app::project::dto::UpdateProjectRequest;
use server::error::ServerError;

mod common;

#[tokio::test]
async fn project_service_checks_membership_before_writing_or_auditing() {
    let postgres = common::migrated_postgres().await;
    let pool = &postgres.pool;
    let installation = common::initialize_installation(
        pool.clone(),
        "Service authorization",
        "owner@example.com",
        "Owner",
        "service-owner",
        "Original project",
    )
    .await;
    sqlx::query("INSERT INTO users (user_id, email, role, status) VALUES ('usr_service_member', 'member@example.com', 'member', 'active')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, 'usr_service_member', 'member')")
        .bind(&installation.project_id).execute(pool).await.unwrap();
    let member = common::principal(pool, "usr_service_member").await;
    let before: (String, i64) =
        sqlx::query_as("SELECT name, revision FROM projects WHERE project_id = $1")
            .bind(&installation.project_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(pool)
        .await
        .unwrap();

    let error = project::update_admin_project(
        pool,
        &member,
        &installation.project_id,
        before.1,
        UpdateProjectRequest {
            name: Some("Unauthorized change".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ServerError::Forbidden(_)));
    let error = project::delete_admin_project(pool, &member, &installation.project_id, before.1)
        .await
        .unwrap_err();
    assert!(matches!(error, ServerError::Forbidden(_)));
    let after: (String, i64) =
        sqlx::query_as("SELECT name, revision FROM projects WHERE project_id = $1")
            .bind(&installation.project_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_events")
            .fetch_one(pool)
            .await
            .unwrap(),
        audit_count
    );

    // Project administrators retain permission without an organization-admin role.
    sqlx::query("UPDATE project_members SET role = 'admin' WHERE project_id = $1 AND user_id = $2")
        .bind(&installation.project_id)
        .bind(&member.user_id)
        .execute(pool)
        .await
        .unwrap();
    let updated = project::update_admin_project(
        pool,
        &member,
        &installation.project_id,
        before.1,
        UpdateProjectRequest {
            name: Some("Authorized change".to_owned()),
            description: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.name, "Authorized change");
    assert_eq!(updated.revision, before.1 + 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_events")
            .fetch_one(pool)
            .await
            .unwrap(),
        audit_count + 1
    );
    postgres.shutdown().await;
}
