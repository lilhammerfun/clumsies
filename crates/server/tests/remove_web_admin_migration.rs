//! Migration removing retired browser-admin state while preserving active identities.

mod common;

#[tokio::test]
async fn web_admin_credentials_are_revoked_and_native_setup_transactions_are_allowed() {
    let postgres = common::postgres_without_migrations().await;
    for migration in [
        include_str!("../migrations/20260708000100_create_server_schema.sql"),
        include_str!("../migrations/20260716000100_add_server_installation_setup.sql"),
        include_str!("../migrations/20260716000200_add_web_admin_sessions.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&postgres.pool)
            .await
            .unwrap();
    }

    sqlx::raw_sql(
        "INSERT INTO orgs (org_id, name) VALUES ('org_legacy', 'Legacy');
         INSERT INTO users (user_id, email, role, status)
         VALUES ('usr_legacy', 'legacy@example.com', 'owner', 'active');
         INSERT INTO auth_sessions (session_id, user_id, org_id, csrf_token)
         VALUES ('ses_web', 'usr_legacy', 'org_legacy', 'csrf');
         INSERT INTO access_tokens (
             token_id, session_id, user_id, kind, token_hash, expires_at
         ) VALUES (
             'tok_web', 'ses_web', 'usr_legacy', 'web_session', 'legacy-hash', now() + interval '1 hour'
         );
         INSERT INTO setup_sessions (
             session_id, token_hash, csrf_token_hash, configuration_saved_at, expires_at
         ) VALUES (
             'setup_legacy', 'setup-token-hash', 'setup-csrf-hash', now(), now() + interval '1 hour'
         );
         INSERT INTO oidc_login_transactions (
             transaction_id, provider_state_hash, nonce, provider_pkce_verifier,
             client_kind, client_redirect_uri, client_state, client_code_challenge,
             return_to, expires_at, flow, setup_session_id
         ) VALUES
             (
                 'login_setup_web', 'state-setup-web', 'nonce', 'verifier',
                 'web_admin', 'https://server.test/admin/setup/callback', NULL, NULL,
                 NULL, now() + interval '10 minutes', 'installation_setup', 'setup_legacy'
             ),
             (
                 'login_admin_web', 'state-admin-web', 'nonce', 'verifier',
                 'web_admin', '/admin', NULL, NULL,
                 '/admin', now() + interval '10 minutes', 'web_admin_login', NULL
             );",
    )
    .execute(&postgres.pool)
    .await
    .unwrap();

    sqlx::raw_sql(include_str!(
        "../migrations/20260829000100_remove_web_admin.sql"
    ))
    .execute(&postgres.pool)
    .await
    .unwrap();

    let web_logins = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM oidc_login_transactions WHERE client_kind = 'web_admin'",
    )
    .fetch_one(&postgres.pool)
    .await
    .unwrap();
    assert_eq!(web_logins, 0);
    assert!(
        sqlx::query_scalar::<_, Option<time::OffsetDateTime>>(
            "SELECT revoked_at FROM access_tokens WHERE token_id = 'tok_web'",
        )
        .fetch_one(&postgres.pool)
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        sqlx::query_scalar::<_, Option<time::OffsetDateTime>>(
            "SELECT revoked_at FROM auth_sessions WHERE session_id = 'ses_web'",
        )
        .fetch_one(&postgres.pool)
        .await
        .unwrap()
        .is_some()
    );

    sqlx::query(
        "INSERT INTO oidc_login_transactions (
            transaction_id, provider_state_hash, nonce, provider_pkce_verifier,
            client_kind, client_redirect_uri, client_state, client_code_challenge,
            expires_at, flow, setup_session_id
         ) VALUES (
            'login_setup_native', 'state-setup-native', 'nonce', 'verifier',
            'desktop', 'http://127.0.0.1:49152/callback', 'client-state', $1,
            now() + interval '10 minutes', 'installation_setup', 'setup_legacy'
         )",
    )
    .bind("a".repeat(43))
    .execute(&postgres.pool)
    .await
    .unwrap();

    let rejected_web_login = sqlx::query(
        "INSERT INTO oidc_login_transactions (
            transaction_id, provider_state_hash, nonce, provider_pkce_verifier,
            client_kind, client_redirect_uri, client_code_challenge,
            expires_at, flow
         ) VALUES (
            'login_web_rejected', 'state-web-rejected', 'nonce', 'verifier',
            'web_admin', '/admin', NULL, now() + interval '10 minutes', 'web_admin_login'
         )",
    )
    .execute(&postgres.pool)
    .await;
    assert!(rejected_web_login.is_err());
    postgres.shutdown().await;
}
