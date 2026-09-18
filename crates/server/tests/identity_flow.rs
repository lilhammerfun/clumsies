mod common;

#[tokio::test]
async fn oidc_identity_survives_an_email_claim_change() {
    let postgres = common::migrated_postgres().await;
    let bootstrap = common::initialize_installation(
        postgres.pool.clone(),
        "Acme Memory",
        "owner@example.com",
        "Owner",
        "stable-subject",
        "Default",
    )
    .await;

    let (_, first_token) = common::authenticated_router_as(
        postgres.pool.clone(),
        "owner@example.com",
        "stable-subject",
        "Owner",
    )
    .await;
    assert_eq!(first_token.user.user_id, bootstrap.user_id);
    assert_eq!(first_token.user.display_name.as_deref(), Some("Owner"));
    assert_eq!(
        first_token.user.avatar_url.as_deref(),
        Some("https://images.example.test/avatar.png")
    );

    let (_, second_token) = common::authenticated_router_as(
        postgres.pool.clone(),
        "owner-renamed@example.com",
        "stable-subject",
        "Owner",
    )
    .await;
    assert_eq!(second_token.user.user_id, bootstrap.user_id);
    assert_eq!(second_token.user.email, "owner@example.com");

    let identity_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM external_identities
         WHERE issuer = $1 AND subject = $2 AND user_id = $3",
    )
    .bind("https://identity.example.test")
    .bind("stable-subject")
    .bind(&bootstrap.user_id)
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(identity_count, 1);
    postgres.shutdown().await;
}
