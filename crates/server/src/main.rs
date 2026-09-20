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
            "usage: clumsies-server [migrate-project-authority [--dry-run | --apply --expected-plan-hash HASH]]",
        )
        .into()),
    }
}
