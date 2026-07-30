//! Deep tier: a full multi-actor journey run daily.
//!
//! 1. **Auth path (ephemeral user):** seed a one-time invite → register → login → `GET /me`, then
//!    delete the user at the end. Validates signup/login each run without accumulating accounts.
//! 2. **Content + social path:** seed a per-run **published + unlisted** video owned by `owner`
//!    (unlisted ⇒ no feed/search pollution, no transcode wait) → `viewer` fetches it, streams,
//!    comments, likes the video, likes the comment, records a view + history → assert stats.
//! 3. **Teardown = the real cascade-delete:** `owner` calls `DELETE /videos/{id}` (the product's own
//!    soft-delete), then the canary polls until every dependent resource is gone. If anything
//!    lingers past the timeout the canary fails — exactly the signal that the cascade regressed.
//!
//! The canary relies *only* on the product's deletion workflow and never hand-deletes its data, so
//! it doubles as the cascade's continuous validator. The cascade verification is gated by
//! `verify_cascade` (off locally, where the Pipe→SQS→worker pipeline doesn't exist).

use std::future::Future;
use std::time::{Duration, Instant};

use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_s3::Client as S3Client;
use serde_json::json;
use uuid::Uuid;

use crate::assertions::{assert_engaged_stats, expect_eq, expect_non_empty, expect_status};
use crate::client::RewindClient;
use crate::config::{Account, CanaryConfig};
use crate::models::{
    AuthResponse, CommentResponse, Profile, ReactionResponse, Stats, StreamUrl, VideoView,
};
use crate::report::RunReport;
use crate::seed;

/// Time a step, record its outcome into the report, and return the produced value on success
/// (`None` on failure — the failure is already recorded). Lets callers guard dependent steps with
/// `let Some(x) = step(...).await else { ... }`.
async fn step<T>(
    report: &mut RunReport,
    name: &str,
    fut: impl Future<Output = Result<T, String>>,
) -> Option<T> {
    let start = Instant::now();
    let res = fut.await;
    let recorded = res.as_ref().map(|_| ()).map_err(|e| e.clone());
    report.record(name, start.elapsed(), recorded);
    res.ok()
}

pub async fn run(
    client: &RewindClient,
    db: &DynamoClient,
    s3: &S3Client,
    cfg: &CanaryConfig,
) -> RunReport {
    let mut report = RunReport::new("deep");

    let (Some(owner), Some(viewer)) = (cfg.owner.clone(), cfg.viewer.clone()) else {
        report.record(
            "preconditions",
            Duration::ZERO,
            Err("CANARY_OWNER_EMAIL/PASSWORD and CANARY_VIEWER_EMAIL/PASSWORD must be set (run `canary setup`)".into()),
        );
        return report;
    };

    // --- Persistent accounts: log both in. Without these the journey can't run; nothing has been
    // created yet, so returning early leaks nothing. ---
    let Some(owner_auth) = step(&mut report, "login-owner", login(client, &owner)).await else {
        return report;
    };
    let Some(viewer_auth) = step(&mut report, "login-viewer", login(client, &viewer)).await else {
        return report;
    };

    // --- Auth path: ephemeral user (register → login → /me). Tracked so it can be cleaned up. ---
    let run_id = short_id();
    let ephemeral = run_ephemeral_auth(client, db, cfg, &run_id, &mut report).await;

    // --- Content + social path on a per-run unlisted video. ---
    let video_id = Uuid::new_v4().to_string();
    let manifest_url = cfg.manifest_url(&video_id);
    let title = format!("Canary Deep {run_id}");

    let seeded = step(
        &mut report,
        "seed-video",
        seed::seed_unlisted_video(db, &video_id, &owner_auth.user_id, &title, &manifest_url),
    )
    .await
    .is_some();

    if seeded {
        run_social_journey(client, cfg, &video_id, &viewer_auth.token, &mut report).await;
        // Teardown ALWAYS runs when the video was seeded, so a mid-journey failure still cleans up.
        run_teardown(
            client,
            db,
            s3,
            cfg,
            &video_id,
            &owner_auth.token,
            &viewer_auth.token,
            &mut report,
        )
        .await;
    }

    // --- Best-effort ephemeral-user cleanup (runs regardless of earlier outcomes). ---
    if let Some((eph_user_id, eph_token)) = ephemeral {
        cleanup_ephemeral(client, db, &eph_user_id, &eph_token, &mut report).await;
    }

    report
}

/// Log in an account, returning its token + user_id.
async fn login(client: &RewindClient, acct: &Account) -> Result<AuthResponse, String> {
    let url = format!("{}/login", client.endpoints.identity);
    let resp = client
        .post(
            &url,
            None,
            Some(json!({"email": acct.email, "password": acct.password})),
        )
        .await?;
    expect_status(resp.status, 200, "identity /login")?;
    let auth: AuthResponse = resp.json()?;
    expect_non_empty(&auth.token, "login token")?;
    Ok(auth)
}

/// Seed an invite, register a fresh ephemeral user, log in, and verify `GET /me`. Returns the
/// ephemeral `(user_id, session_token)` once registered (so it can be cleaned up even if a later
/// sub-step fails).
async fn run_ephemeral_auth(
    client: &RewindClient,
    db: &DynamoClient,
    cfg: &CanaryConfig,
    run_id: &str,
    report: &mut RunReport,
) -> Option<(String, String)> {
    let invite = format!("CANARY-{run_id}");
    let email = cfg.ephemeral_email(run_id);
    let password = "CanaryEphemeral!1";

    step(report, "seed-invite", seed::seed_invite_code(db, &invite)).await?;

    let reg_url = format!("{}/register", client.endpoints.identity);
    let body = json!({
        "invite_code": invite,
        "email": email,
        "password": password,
        "display_name": format!("Canary {run_id}"),
    });
    let auth = step(report, "register-ephemeral", async {
        let resp = client.post(&reg_url, None, Some(body)).await?;
        expect_status(resp.status, 201, "identity /register")?;
        let auth: AuthResponse = resp.json()?;
        expect_non_empty(&auth.token, "register token")?;
        Ok(auth)
    })
    .await?;

    // Verify a fresh login works (independent of the registration token).
    let login_token = step(
        report,
        "login-ephemeral",
        login(
            client,
            &Account {
                email: email.clone(),
                password: password.to_string(),
            },
        ),
    )
    .await
    .map(|a| a.token);

    // GET /me with whichever token we have; assert it resolves to the registered user.
    let me_token = login_token.clone().unwrap_or_else(|| auth.token.clone());
    let me_url = format!("{}/me", client.endpoints.identity);
    let expected_id = auth.user_id.clone();
    step(report, "me-ephemeral", async {
        let resp = client.get(&me_url, Some(&me_token)).await?;
        expect_status(resp.status, 200, "identity /me")?;
        let profile: Profile = resp.json()?;
        expect_eq(&profile.user_id, &expected_id, "/me user_id")
    })
    .await;

    // Return the registration token (always valid) for logout, plus the user_id.
    Some((auth.user_id, login_token.unwrap_or(auth.token)))
}

/// Drive the viewer's interactions with the seeded video and assert the stats reflect them.
async fn run_social_journey(
    client: &RewindClient,
    _cfg: &CanaryConfig,
    video_id: &str,
    viewer_token: &str,
    report: &mut RunReport,
) {
    let ep = &client.endpoints;

    // Catalog: the video is fetchable by direct id (unlisted) and is published.
    step(report, "get-video", async {
        let url = format!("{}/videos/{video_id}", ep.catalog);
        let resp = client.get(&url, None).await?;
        expect_status(resp.status, 200, "catalog get video")?;
        let v: VideoView = resp.json()?;
        expect_eq(&v.video_id, video_id, "video_id")?;
        expect_eq(&v.status, "published", "video status")
    })
    .await;

    // Streaming: returns a URL (the manifest, echoed for unlisted) — proves the service path, not
    // real byte playback.
    step(report, "stream-url", async {
        let url = format!("{}/videos/{video_id}/stream-url", ep.streaming);
        let resp = client.get(&url, Some(viewer_token)).await?;
        expect_status(resp.status, 200, "streaming stream-url")?;
        let s: StreamUrl = resp.json()?;
        expect_non_empty(&s.url, "stream url")
    })
    .await;

    // Social: comment, then capture the comment_id for the comment-like step.
    let comment_id = step(report, "comment", async {
        let url = format!("{}/videos/{video_id}/comments", ep.social);
        let resp = client
            .post(
                &url,
                Some(viewer_token),
                Some(json!({"text": "canary was here"})),
            )
            .await?;
        expect_status(resp.status, 201, "social add comment")?;
        let c: CommentResponse = resp.json()?;
        expect_non_empty(&c.comment_id, "comment_id")?;
        Ok(c.comment_id)
    })
    .await;

    step(report, "like-video", async {
        let url = format!("{}/videos/{video_id}/like", ep.social);
        let resp = client.post(&url, Some(viewer_token), None).await?;
        expect_status(resp.status, 200, "social like")?;
        let r: ReactionResponse = resp.json()?;
        expect_eq(&r.action, "added", "like action")
    })
    .await;

    if let Some(cid) = comment_id {
        step(report, "like-comment", async {
            let url = format!("{}/videos/{video_id}/comments/{cid}/like", ep.social);
            let resp = client.post(&url, Some(viewer_token), None).await?;
            expect_status(resp.status, 200, "social like comment")
        })
        .await;
    }

    step(report, "record-view", async {
        let url = format!("{}/videos/{video_id}/view", ep.social);
        let resp = client.post(&url, None, None).await?;
        expect_status(resp.status, 204, "social record view")
    })
    .await;

    step(report, "record-history", async {
        let url = format!("{}/videos/{video_id}/history", ep.social);
        let resp = client.post(&url, Some(viewer_token), None).await?;
        expect_status(resp.status, 204, "social record history")
    })
    .await;

    step(report, "stats", async {
        let url = format!("{}/videos/{video_id}/stats", ep.social);
        let resp = client.get(&url, None).await?;
        expect_status(resp.status, 200, "social stats")?;
        let stats: Stats = resp.json()?;
        assert_engaged_stats(&stats, "social stats")
    })
    .await;
}

/// Delete the video via the real cascade, confirm the soft-delete is observable, then (in cloud)
/// poll until every dependent resource is reclaimed.
#[allow(clippy::too_many_arguments)]
async fn run_teardown(
    client: &RewindClient,
    db: &DynamoClient,
    s3: &S3Client,
    cfg: &CanaryConfig,
    video_id: &str,
    owner_token: &str,
    viewer_token: &str,
    report: &mut RunReport,
) {
    let ep = &client.endpoints;

    step(report, "delete-video", async {
        let url = format!("{}/videos/{video_id}", ep.catalog);
        let resp = client.delete(&url, Some(owner_token)).await?;
        expect_status(resp.status, 204, "catalog delete video")
    })
    .await;

    // The soft-delete is immediately observable (no worker needed): catalog and streaming both 404.
    step(report, "soft-delete-observable", async {
        let get_url = format!("{}/videos/{video_id}", ep.catalog);
        let get = client.get(&get_url, None).await?;
        expect_status(get.status, 404, "catalog get after delete")?;

        let stream_url = format!("{}/videos/{video_id}/stream-url", ep.streaming);
        let stream = client.get(&stream_url, Some(viewer_token)).await?;
        expect_status(stream.status, 404, "streaming after delete")
    })
    .await;

    // The cascade itself (Pipe → SQS → delete-cleanup worker) is cloud-only.
    if cfg.verify_cascade {
        step(
            report,
            "cascade-cleanup",
            poll_cascade(db, s3, video_id, cfg),
        )
        .await;
    } else {
        report.record("cascade-cleanup", Duration::ZERO, Ok(()));
        tracing::warn!(
            "cascade verification skipped (CANARY_VERIFY_CASCADE=false) — no Pipe/worker locally"
        );
    }
}

/// Poll the dependent stores until the cascade has reclaimed everything, or fail at the timeout.
async fn poll_cascade(
    db: &DynamoClient,
    s3: &S3Client,
    video_id: &str,
    cfg: &CanaryConfig,
) -> Result<(), String> {
    let deadline = Instant::now() + cfg.cascade_timeout;
    loop {
        let probe =
            seed::probe_cleanup(db, s3, video_id, &cfg.video_bucket, &cfg.raw_bucket).await?;
        if probe.is_clean() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "cascade incomplete after {:?}: remaining {}",
                cfg.cascade_timeout,
                probe.remaining()
            ));
        }
        tokio::time::sleep(cfg.cascade_poll_interval).await;
    }
}

/// Log out the ephemeral session and delete the ephemeral user row. Best-effort but recorded, so a
/// cleanup regression is visible.
async fn cleanup_ephemeral(
    client: &RewindClient,
    db: &DynamoClient,
    user_id: &str,
    token: &str,
    report: &mut RunReport,
) {
    step(report, "logout-ephemeral", async {
        let url = format!("{}/logout", client.endpoints.identity);
        let resp = client.post(&url, Some(token), None).await?;
        expect_status(resp.status, 204, "identity logout")
    })
    .await;

    step(
        report,
        "delete-ephemeral-user",
        seed::delete_user(db, user_id),
    )
    .await;
}

/// A short, collision-resistant run id for naming ephemeral resources.
fn short_id() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::short_id;

    #[test]
    fn short_id_is_12_hex_chars() {
        let id = short_id();
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn short_ids_differ() {
        assert_ne!(short_id(), short_id());
    }
}
