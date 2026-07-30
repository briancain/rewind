//! Cascade-cleanup reconciler — detection of `deleted` tombstones whose dependent data was never
//! reclaimed.
//!
//! The cleanup-side analogue of the stuck-`processing` transcode reconciler (`transcode/reconcile.rs`).
//! A soft-delete (`status = deleted`) is supposed to flow `videos` stream → EventBridge Pipe → SQS →
//! the `delete-cleanup` worker, which reclaims the video's dependent rows + S3 objects. Two failure
//! modes leave that cleanup half-done and are caught by **no** existing alarm. First, the
//! `videos-to-cleanup` Pipe can fail to enqueue the event at all — nothing ever reaches the queue, so
//! the DLQ-depth alarm stays empty. Second, cleanup can run but only partially complete and not be
//! redriven — the message was consumed and deleted on a "success" that wasn't complete, so again no
//! DLQ. Both silently orphan the dependent data.
//!
//! This sweep (a per-region CronJob) closes that gap: it scans the `videos` **Global Table** for
//! `deleted` tombstones old enough that cleanup should have finished, probes whether any dependent
//! data still exists, and emits `Rewind/Deletion UnreclaimedDeletions` (+ an alarm). MVP scope is
//! **detect + alarm only** — automated re-cleanup is deliberately deferred (mirrors the transcode
//! reconciler's deferred re-drive).
//!
//! Unlike the transcode reconciler — whose "stuck" decision is a pure timestamp check on the scanned
//! row alone — a `deleted` tombstone tells us **nothing** about whether cleanup ran: the worker never
//! writes the `videos` row (a `deleted → deleted` MODIFY would otherwise loop back through the Pipe's
//! `status == deleted` filter), so every tombstone — cleaned or not — survives until the TTL
//! finalizer purges it ~24h later. Detection therefore has two stages: a **pure** candidate filter
//! (`status = deleted` and old enough to not race in-flight cleanup) kept exhaustively unit-testable,
//! and a read-only **probe** of the dependent stores for each candidate.

use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client as CwClient;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_s3::Client as S3Client;
use chrono::{DateTime, Duration, Utc};
use shared::error::AppError;
use shared::tables::table;
use shared::video::VideoStatus;
use std::collections::HashMap;

/// Default grace window (minutes) between a soft-delete and when we expect cleanup to be done.
/// Cleanup itself finishes in seconds, but a redriven message can take several minutes to drain
/// through the cleanup queue's 120s visibility timeout × `maxReceiveCount` 5, so the threshold sits
/// well beyond that worst case — a tombstone younger than this is never flagged (cleanup may still be
/// legitimately in flight). Override via `DELETION_RECONCILE_THRESHOLD_MINS`.
pub const DEFAULT_RECONCILE_THRESHOLD_MINS: i64 = 30;

/// Resolve the grace threshold from `DELETION_RECONCILE_THRESHOLD_MINS` (whole minutes), falling back
/// to [`DEFAULT_RECONCILE_THRESHOLD_MINS`] when unset, empty, non-numeric, or non-positive.
pub fn threshold_from_env() -> Duration {
    let mins = std::env::var("DELETION_RECONCILE_THRESHOLD_MINS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|m| *m > 0)
        .unwrap_or(DEFAULT_RECONCILE_THRESHOLD_MINS);
    Duration::minutes(mins)
}

/// CloudWatch namespace + metric for the unreclaimed-deletion signal the alarm watches.
const METRIC_NAMESPACE: &str = "Rewind/Deletion";
const ORPHAN_METRIC: &str = "UnreclaimedDeletions";

/// A `deleted` tombstone old enough that cleanup should have finished — a candidate to probe for
/// leftover dependent data. (Whether it is *actually* orphaned is decided by the I/O probe.)
#[derive(Debug, Clone, PartialEq)]
pub struct OrphanCandidate {
    pub video_id: String,
    pub deleted_at: Option<String>,
}

/// Read a string (`S`) attribute from a DynamoDB item.
fn attr_s<'a>(item: &'a HashMap<String, AttributeValue>, key: &str) -> Option<&'a str> {
    item.get(key)
        .and_then(|av| av.as_s().ok())
        .map(String::as_str)
}

/// Decide whether a single `videos` row is a cleanup candidate to probe.
///
/// Candidate iff the row is `deleted` AND has been so for longer than `threshold` (measured by
/// `deleted_at`, stamped at soft-delete time). A `deleted` row whose `deleted_at` is missing or
/// unparseable is itself anomalous, so it is treated as a candidate too — we want to probe it rather
/// than let a malformed tombstone hide leftover data. Every non-`deleted` status is never a
/// candidate.
///
/// Pure: `now` and `threshold` are injected so this is deterministic in tests. Mirrors
/// `transcode::reconcile::is_stuck`.
pub fn is_orphan_candidate(
    status: &str,
    deleted_at: Option<&str>,
    now: DateTime<Utc>,
    threshold: Duration,
) -> bool {
    if !matches!(status.parse::<VideoStatus>(), Ok(VideoStatus::Deleted)) {
        return false;
    }

    match deleted_at.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        Some(ts) => now.signed_duration_since(ts.with_timezone(&Utc)) > threshold,
        // Anomalous tombstone (no/!parseable deleted_at) — surface it for probing.
        None => true,
    }
}

/// Filter a batch of scanned `videos` rows to the tombstones worth probing. Pure (no AWS): the
/// caller supplies the scanned items, `now`, and `threshold`. A row missing a `status` attribute is
/// not a tombstone, so it is ignored. Mirrors `transcode::reconcile::find_stuck`.
pub fn find_candidates(
    items: &[HashMap<String, AttributeValue>],
    now: DateTime<Utc>,
    threshold: Duration,
) -> Vec<OrphanCandidate> {
    items
        .iter()
        .filter_map(|item| {
            let status = attr_s(item, "status")?;
            let deleted_at = attr_s(item, "deleted_at");
            is_orphan_candidate(status, deleted_at, now, threshold).then(|| OrphanCandidate {
                video_id: attr_s(item, "video_id").unwrap_or("<unknown>").to_string(),
                deleted_at: deleted_at.map(str::to_string),
            })
        })
        .collect()
}

/// Probe whether ANY dependent data still exists for `video_id`, across every store the cleanup
/// worker reclaims (the same keys as `cleanup.rs`, but existence-only and read-only). Short-circuits
/// on the first hit, so a fully-cleaned video costs one cheap miss per store and an orphaned one
/// returns as soon as a single leftover is found.
pub async fn has_remaining_dependents(
    db: &DynamoClient,
    s3: &S3Client,
    video_bucket: &str,
    raw_bucket: &str,
    video_id: &str,
) -> Result<bool, AppError> {
    // Social rows keyed (or GSI-keyed) by video_id.
    if query_has_any(db, &table("comments"), None, video_id).await?
        || query_has_any(db, &table("reactions"), None, video_id).await?
        || query_has_any(db, &table("comment_reactions"), None, video_id).await?
        || query_has_any(db, &table("view_history"), Some("video-id-index"), video_id).await?
        || query_has_any(
            db,
            &table("transcode_jobs"),
            Some("video-id-index"),
            video_id,
        )
        .await?
        || item_exists(db, &table("video_stats"), video_id).await?
    {
        return Ok(true);
    }

    // S3 objects under the video's prefixes (videos + raw buckets).
    for prefix in [
        format!("hls/{video_id}/"),
        format!("mp4/{video_id}/"),
        format!("thumbnails/{video_id}/"),
    ] {
        if prefix_has_any(s3, video_bucket, &prefix).await? {
            return Ok(true);
        }
    }
    if prefix_has_any(s3, raw_bucket, &format!("raw/{video_id}/")).await? {
        return Ok(true);
    }

    Ok(false)
}

/// `Query` (base table or GSI) on `video_id` with `Limit 1` — true iff at least one row exists.
async fn query_has_any(
    db: &DynamoClient,
    table_name: &str,
    index: Option<&str>,
    video_id: &str,
) -> Result<bool, AppError> {
    let mut req = db
        .query()
        .table_name(table_name)
        .key_condition_expression("video_id = :v")
        .expression_attribute_values(":v", AttributeValue::S(video_id.to_string()))
        .limit(1);
    if let Some(idx) = index {
        req = req.index_name(idx);
    }
    let resp = req.send().await.map_err(AppError::internal)?;
    Ok(resp.count() > 0)
}

/// `GetItem` existence check for the single-item `video_stats` row (PK = video_id).
async fn item_exists(
    db: &DynamoClient,
    table_name: &str,
    video_id: &str,
) -> Result<bool, AppError> {
    let resp = db
        .get_item()
        .table_name(table_name)
        .key("video_id", AttributeValue::S(video_id.to_string()))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(resp.item.is_some())
}

/// `ListObjectsV2` with `MaxKeys 1` — true iff at least one object exists under `prefix`.
async fn prefix_has_any(s3: &S3Client, bucket: &str, prefix: &str) -> Result<bool, AppError> {
    let resp = s3
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .max_keys(1)
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(resp.key_count().unwrap_or(0) > 0)
}

/// Emit the `Rewind/Deletion UnreclaimedDeletions` count (dimensioned by `Region`) — emitted every
/// run, including `0`, so the alarm has a fresh datapoint each tick and can clear once the orphans
/// are reclaimed. Best-effort: a CloudWatch error is logged, never fatal (the sweep's job is
/// detection; a failed emit is simply retried next tick). Mirrors
/// `transcode::reconcile::emit_stuck_count`.
pub async fn emit_orphan_count(client: &CwClient, region: &str, count: usize) {
    let datum = MetricDatum::builder()
        .metric_name(ORPHAN_METRIC)
        .dimensions(Dimension::builder().name("Region").value(region).build())
        .value(count as f64)
        .unit(StandardUnit::Count)
        .build();
    match client
        .put_metric_data()
        .namespace(METRIC_NAMESPACE)
        .metric_data(datum)
        .send()
        .await
    {
        Ok(_) => tracing::info!(count, "emitted {METRIC_NAMESPACE}/{ORPHAN_METRIC} metric"),
        Err(e) => tracing::error!(error = %e, "failed to emit UnreclaimedDeletions metric"),
    }
}

/// Run one reconcile sweep: scan the `videos` (Global) table, filter to old-enough `deleted`
/// tombstones, probe each for leftover dependent data, log every orphan, and emit the count.
/// **Detect + alarm only** — no row is mutated and no cleanup is re-issued (automated re-cleanup is
/// deferred). When `cw` is `None` (e.g. the integration test) the metric emit is skipped. Returns the
/// number of orphaned tombstones found.
pub async fn run_sweep(
    db: &DynamoClient,
    s3: &S3Client,
    cw: Option<&CwClient>,
    region: &str,
    video_bucket: &str,
    raw_bucket: &str,
    threshold: Duration,
) -> Result<usize, AppError> {
    let items = shared::dynamo::scan_all(db, &table("videos"))
        .await
        .map_err(AppError::internal)?;
    let candidates = find_candidates(&items, Utc::now(), threshold);

    let mut orphans = 0usize;
    for c in &candidates {
        if has_remaining_dependents(db, s3, video_bucket, raw_bucket, &c.video_id).await? {
            orphans += 1;
            tracing::warn!(
                video_id = %c.video_id,
                deleted_at = ?c.deleted_at,
                "unreclaimed deletion detected (deleted tombstone still has dependent data)"
            );
        }
    }

    if orphans == 0 {
        tracing::info!(
            scanned = items.len(),
            candidates = candidates.len(),
            "reconcile sweep: no unreclaimed deletions"
        );
    } else {
        tracing::warn!(
            scanned = items.len(),
            candidates = candidates.len(),
            orphans,
            "reconcile sweep: unreclaimed deletions detected — see CloudWatch alarm"
        );
    }

    if let Some(cw) = cw {
        emit_orphan_count(cw, region, orphans).await;
    }
    Ok(orphans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pairs: &[(&str, &str)]) -> HashMap<String, AttributeValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), AttributeValue::S(v.to_string())))
            .collect()
    }

    fn now() -> DateTime<Utc> {
        "2026-06-24T08:00:00Z".parse().unwrap()
    }

    fn ago(mins: i64) -> String {
        (now() - Duration::minutes(mins)).to_rfc3339()
    }

    fn threshold() -> Duration {
        Duration::minutes(DEFAULT_RECONCILE_THRESHOLD_MINS)
    }

    #[test]
    fn deleted_older_than_threshold_is_candidate() {
        assert!(is_orphan_candidate(
            "deleted",
            Some(&ago(31)),
            now(),
            threshold()
        ));
        assert!(is_orphan_candidate(
            "deleted",
            Some(&ago(600)),
            now(),
            threshold()
        ));
    }

    #[test]
    fn deleted_within_threshold_is_not_candidate() {
        // Cleanup may still be legitimately in flight — don't race it.
        assert!(!is_orphan_candidate(
            "deleted",
            Some(&ago(1)),
            now(),
            threshold()
        ));
        assert!(!is_orphan_candidate(
            "deleted",
            Some(&ago(29)),
            now(),
            threshold()
        ));
    }

    #[test]
    fn deleted_exactly_at_threshold_is_not_candidate() {
        // Strictly greater-than, so the boundary is not (yet) a candidate.
        assert!(!is_orphan_candidate(
            "deleted",
            Some(&ago(30)),
            now(),
            threshold()
        ));
    }

    #[test]
    fn non_deleted_statuses_are_never_candidates() {
        for status in ["draft", "processing", "published", "failed"] {
            assert!(
                !is_orphan_candidate(status, Some(&ago(600)), now(), threshold()),
                "{status} should never be a candidate"
            );
        }
    }

    #[test]
    fn unknown_status_is_not_candidate() {
        assert!(!is_orphan_candidate(
            "ready",
            Some(&ago(600)),
            now(),
            threshold()
        ));
        assert!(!is_orphan_candidate(
            "",
            Some(&ago(600)),
            now(),
            threshold()
        ));
    }

    #[test]
    fn deleted_with_missing_timestamp_is_flagged() {
        assert!(is_orphan_candidate("deleted", None, now(), threshold()));
    }

    #[test]
    fn deleted_with_unparseable_timestamp_is_flagged() {
        assert!(is_orphan_candidate(
            "deleted",
            Some("not-a-date"),
            now(),
            threshold()
        ));
        assert!(is_orphan_candidate("deleted", Some(""), now(), threshold()));
    }

    #[test]
    fn timezone_offsets_are_normalized_to_utc() {
        // 09:00 +02:00 == 07:00Z, i.e. 60 minutes ago — past threshold, candidate.
        assert!(is_orphan_candidate(
            "deleted",
            Some("2026-06-24T09:00:00+02:00"),
            now(),
            threshold()
        ));
        // 09:50 +02:00 == 07:50Z, i.e. 10 minutes ago — within threshold, not a candidate.
        assert!(!is_orphan_candidate(
            "deleted",
            Some("2026-06-24T09:50:00+02:00"),
            now(),
            threshold()
        ));
    }

    #[test]
    fn threshold_from_env_defaults_when_unset() {
        std::env::remove_var("DELETION_RECONCILE_THRESHOLD_MINS");
        assert_eq!(
            threshold_from_env(),
            Duration::minutes(DEFAULT_RECONCILE_THRESHOLD_MINS)
        );
    }

    #[test]
    fn find_candidates_returns_only_old_enough_tombstones() {
        let items = vec![
            item(&[
                ("video_id", "orphan-1"),
                ("status", "deleted"),
                ("deleted_at", &ago(120)),
            ]),
            item(&[
                ("video_id", "fresh-delete"),
                ("status", "deleted"),
                ("deleted_at", &ago(5)),
            ]),
            item(&[
                ("video_id", "live"),
                ("status", "published"),
                ("deleted_at", &ago(120)),
            ]),
            item(&[("video_id", "processing"), ("status", "processing")]),
        ];

        let candidates = find_candidates(&items, now(), threshold());
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].video_id, "orphan-1");
        assert_eq!(candidates[0].deleted_at, Some(ago(120)));
    }

    #[test]
    fn find_candidates_flags_deleted_row_missing_deleted_at() {
        let items = vec![item(&[("video_id", "no-ts"), ("status", "deleted")])];
        let candidates = find_candidates(&items, now(), threshold());
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].video_id, "no-ts");
        assert_eq!(candidates[0].deleted_at, None);
    }

    #[test]
    fn find_candidates_ignores_rows_without_a_status_attribute() {
        let items = vec![item(&[("video_id", "weird"), ("deleted_at", &ago(120))])];
        assert!(find_candidates(&items, now(), threshold()).is_empty());
    }

    #[test]
    fn find_candidates_empty_input_is_empty() {
        assert!(find_candidates(&[], now(), threshold()).is_empty());
    }
}
