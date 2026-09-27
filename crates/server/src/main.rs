//! Server and maintenance command entry points.

/// Dispatch normal server startup or the explicit, fingerprint-guarded maintenance command.
///
/// # Errors
/// Propagates invalid maintenance arguments, startup failures, or guarded migration failures.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => server::run().await,
        [command, user_id] if command == "recover-owner" => {
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() {
                return Err(std::io::Error::other("owner recovery must run in an interactive terminal; credentials must not enter service logs").into());
            }
            let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1)
                .connect(&std::env::var("DATABASE_URL")?).await?;
            let result = server::app::auth::AuthService::unconfigured(pool).recover_owner(user_id).await?;
            println!("One-time recovery credential (expires {}):\n{}", result.expires_at, result.token);
            Ok(())
        },
        [command] if command == "migrate-project-authority" => {
            server::run_project_authority_migration(None).await
        }
        [command, dry_run]
            if command == "migrate-project-authority" && dry_run == "--dry-run" =>
        {
            server::run_project_authority_migration(None).await
        }
        [command, apply, expected_flag, expected_plan_hash]
            if command == "migrate-project-authority"
                && apply == "--apply"
                && expected_flag == "--expected-plan-hash" =>
        {
            server::run_project_authority_migration(Some(expected_plan_hash)).await
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: clumsies-server [recover-owner USER_ID | migrate-project-authority [--dry-run | --apply --expected-plan-hash HASH]]",
        )
        .into()),
    }
}
