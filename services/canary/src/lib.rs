//! Cloud integration canary.
//!
//! A blackbox binary that exercises the *real, user-facing* platform end-to-end against the public
//! `*.${DOMAIN}` endpoints (or, for local integration, the `./scripts/dev.sh` stack on localhost).
//! It runs in two tiers dispatched on argv (like `search reindex`):
//!
//! * `shallow` — read-only liveness/correctness: health, public feed, a search query.
//! * `deep`    — a full multi-actor journey that seeds a per-run *unlisted* video, drives the
//!   social/streaming flows, then deletes it through the real cascade-delete and verifies every
//!   dependent resource is reclaimed. This doubles as the cascade's continuous validator.
//! * `setup`   — one-time registration of the persistent `owner`/`viewer` accounts.
//!
//! Pure request/assertion/decision logic lives in [`assertions`], [`config`] and [`report`] and is
//! unit-tested with no AWS or network. The live, env-driven run is exercised by an `#[ignore]`
//! integration test (mirroring `transcode/tests/mc_validate.rs`).

pub mod assertions;
pub mod client;
pub mod config;
pub mod deep;
pub mod dns;
pub mod metrics;
pub mod models;
pub mod report;
pub mod seed;
pub mod setup;
pub mod shallow;
