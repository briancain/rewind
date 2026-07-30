//! `setup` subcommand: one-time registration of the persistent `owner` and
//! `viewer` accounts the `deep` tier uses. Idempotent — if the account already exists (login
//! succeeds) it is left as-is; otherwise an invite is seeded and the account registered.
//!
//! Run once by an operator (its output credentials are stored in a k8s Secret the CronJob mounts);
//! never scheduled. Credentials come from the same `CANARY_OWNER_*` / `CANARY_VIEWER_*` env the
//! `deep` tier reads.

use aws_sdk_dynamodb::Client as DynamoClient;
use serde_json::json;

use crate::assertions::expect_status;
use crate::client::RewindClient;
use crate::config::{Account, CanaryConfig};
use crate::seed;

/// Ensure both persistent accounts exist. Returns an error string if either could not be ensured.
pub async fn run(
    client: &RewindClient,
    db: &DynamoClient,
    cfg: &CanaryConfig,
) -> Result<(), String> {
    let owner = cfg
        .owner
        .clone()
        .ok_or("CANARY_OWNER_EMAIL/CANARY_OWNER_PASSWORD must be set")?;
    let viewer = cfg
        .viewer
        .clone()
        .ok_or("CANARY_VIEWER_EMAIL/CANARY_VIEWER_PASSWORD must be set")?;

    ensure_account(client, db, &owner, "Canary Owner").await?;
    ensure_account(client, db, &viewer, "Canary Viewer").await?;
    tracing::info!("canary setup complete: owner + viewer accounts ensured");
    Ok(())
}

/// Ensure one account exists: try logging in first; on 401 seed an invite and register.
async fn ensure_account(
    client: &RewindClient,
    db: &DynamoClient,
    acct: &Account,
    display_name: &str,
) -> Result<(), String> {
    let login_url = format!("{}/login", client.endpoints.identity);
    let resp = client
        .post(
            &login_url,
            None,
            Some(json!({"email": acct.email, "password": acct.password})),
        )
        .await?;

    match resp.status {
        200 => {
            tracing::info!(email = %acct.email, "account already exists");
            Ok(())
        }
        401 => {
            tracing::info!(email = %acct.email, "account not found; registering");
            let invite = format!("CANARY-SETUP-{}", uuid::Uuid::new_v4().simple());
            seed::seed_invite_code(db, &invite).await?;

            let reg_url = format!("{}/register", client.endpoints.identity);
            let body = json!({
                "invite_code": invite,
                "email": acct.email,
                "password": acct.password,
                "display_name": display_name,
            });
            let reg = client.post(&reg_url, None, Some(body)).await?;
            expect_status(reg.status, 201, &format!("register {}", acct.email))
        }
        other => Err(format!(
            "unexpected status {other} logging in {} during setup",
            acct.email
        )),
    }
}
