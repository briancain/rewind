//! Env-driven live run, mirroring `transcode/tests/mc_validate.rs`: `#[ignore]` so it never runs in
//! the normal `cargo test`, executed explicitly against a running stack to validate the canary
//! harness end-to-end.
//!
//! Phase 2 (local integration against `./scripts/dev.sh`):
//!
//! ```bash
//! # shallow — fully exercisable locally (index may be empty, so don't require a hit):
//! CANARY_SEARCH_EXPECT_HIT=false \
//!   cargo test -p canary --test live_run live_shallow -- --ignored --nocapture
//!
//! # deep — full journey; cascade verification OFF locally (no Pipe/worker against DynamoDB-Local):
//! #   First ensure accounts exist:
//! CANARY_OWNER_EMAIL=canary-owner@canary.invalid  CANARY_OWNER_PASSWORD=CanaryOwner!1 \
//! CANARY_VIEWER_EMAIL=canary-viewer@canary.invalid CANARY_VIEWER_PASSWORD=CanaryViewer!1 \
//! DYNAMODB_ENDPOINT=http://localhost:8000 S3_ENDPOINT=http://localhost:4566 \
//! AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-west-2 \
//!   cargo test -p canary --test live_run live_setup -- --ignored --nocapture
//!
//! CANARY_VERIFY_CASCADE=false \
//! CANARY_OWNER_EMAIL=canary-owner@canary.invalid  CANARY_OWNER_PASSWORD=CanaryOwner!1 \
//! CANARY_VIEWER_EMAIL=canary-viewer@canary.invalid CANARY_VIEWER_PASSWORD=CanaryViewer!1 \
//! DYNAMODB_ENDPOINT=http://localhost:8000 S3_ENDPOINT=http://localhost:4566 \
//! AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-west-2 \
//!   cargo test -p canary --test live_run live_deep -- --ignored --nocapture
//! ```
//!
//! In the cloud the same binary runs as `service shallow|deep` with `CANARY_DOMAIN` set and
//! `CANARY_VERIFY_CASCADE` defaulting on.

use canary::client::RewindClient;
use canary::config::CanaryConfig;
use canary::{deep, setup, shallow};

#[tokio::test]
#[ignore = "requires a running stack; env-driven"]
async fn live_shallow() {
    let cfg = CanaryConfig::from_env();
    let client = RewindClient::new(cfg.endpoints.clone()).expect("client");
    let report = shallow::run(&client, &cfg).await;
    println!("\n{}", report.summary());
    assert!(report.passed(), "shallow run failed");
}

#[tokio::test]
#[ignore = "requires a running stack + owner/viewer creds; env-driven"]
async fn live_setup() {
    let cfg = CanaryConfig::from_env();
    let client = RewindClient::new(cfg.endpoints.clone()).expect("client");
    let svc = shared::config::ServiceConfig::from_env("canary");
    let db = shared::dynamo::create_client(&svc).await;
    setup::run(&client, &db, &cfg).await.expect("setup failed");
    println!("\nsetup complete");
}

#[tokio::test]
#[ignore = "requires a running stack + owner/viewer creds; env-driven"]
async fn live_deep() {
    let cfg = CanaryConfig::from_env();
    let client = RewindClient::new(cfg.endpoints.clone()).expect("client");
    let svc = shared::config::ServiceConfig::from_env("canary");
    let db = shared::dynamo::create_client(&svc).await;
    let s3 = shared::aws::s3_client().await;
    let report = deep::run(&client, &db, &s3, &cfg).await;
    println!("\n{}", report.summary());
    assert!(report.passed(), "deep run failed");
}
