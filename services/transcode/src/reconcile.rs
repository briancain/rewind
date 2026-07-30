//! Stuck-`processing` transcode reconciler — DETECTION (pure logic).
//!
//! A video sits in `status = processing` from job submission until a MediaConvert completion event
//! flips it to `published`/`failed`. If that event is never delivered (a lost EventBridge delivery,
//! a never-emitted event, or — the multi-region motivator — the whole region dying mid-transcode),
//! nothing flips it out of `processing` and the upload silently never plays. No DLQ catches this,
//! because nothing *failed* (the job message was already consumed).
//!
//! This module is the pure, AWS-free classifier used by the `reconcile` sweep (run as a per-region
//! CronJob). MVP scope is **detect + alarm only** — the sweep emits a metric/alarm on each stuck
//! row; automated re-drive is deliberately deferred. Keeping the decision here pure makes it
//! exhaustively unit-testable without DynamoDB.

use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client as CwClient;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{DateTime, Duration, Utc};
use shared::video::VideoStatus;
use std::collections::HashMap;

/// Default "stuck" threshold (minutes) — well beyond the maximum expected MediaConvert encode time,
/// so a healthy in-flight job is never flagged. Override via `RECONCILE_STUCK_THRESHOLD_MINS`.
pub const DEFAULT_STUCK_THRESHOLD_MINS: i64 = 60;

/// Resolve the stuck threshold from `RECONCILE_STUCK_THRESHOLD_MINS` (whole minutes), falling back
/// to [`DEFAULT_STUCK_THRESHOLD_MINS`] when unset, empty, non-numeric, or non-positive.
pub fn threshold_from_env() -> Duration {
    let mins = std::env::var("RECONCILE_STUCK_THRESHOLD_MINS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|m| *m > 0)
        .unwrap_or(DEFAULT_STUCK_THRESHOLD_MINS);
    Duration::minutes(mins)
}

/// Decide whether a single `videos` row represents a stranded transcode.
///
/// Stuck iff the row is `processing` AND it has been so for longer than `threshold` (measured by
/// `updated_at`, which the transcode pipeline stamps on every status write). A `processing` row
/// whose `updated_at` is missing or unparseable is itself anomalous, so it is flagged too — we want
/// to know about it rather than let a malformed row hide a stranded job. Every non-`processing`
/// status (draft/published/failed/deleted, or any unknown value) is never stuck.
///
/// Pure: `now` and `threshold` are injected so this is deterministic in tests.
pub fn is_stuck(
    status: &str,
    updated_at: Option<&str>,
    now: DateTime<Utc>,
    threshold: Duration,
) -> bool {
    // Only an in-flight transcode can be "stuck". Parse strictly so a typo'd/unknown status is
    // treated as not-stuck rather than accidentally matching.
    if !matches!(status.parse::<VideoStatus>(), Ok(VideoStatus::Processing)) {
        return false;
    }

    match updated_at.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        Some(ts) => now.signed_duration_since(ts.with_timezone(&Utc)) > threshold,
        // Anomalous `processing` row (no/!parseable timestamp) — surface it.
        None => true,
    }
}

/// CloudWatch namespace + metric for the stuck-transcode signal the alarm watches.
const METRIC_NAMESPACE: &str = "Rewind/Transcode";
const STUCK_METRIC: &str = "StuckTranscodes";

/// One stranded transcode the sweep flagged.
#[derive(Debug, Clone, PartialEq)]
pub struct StuckVideo {
    pub video_id: String,
    pub updated_at: Option<String>,
}

/// Read a string (`S`) attribute from a DynamoDB item.
fn attr_s<'a>(item: &'a HashMap<String, AttributeValue>, key: &str) -> Option<&'a str> {
    item.get(key)
        .and_then(|av| av.as_s().ok())
        .map(String::as_str)
}

/// Classify a batch of scanned `videos` rows, returning the stranded ones. Pure (no AWS): the
/// caller supplies the scanned items, `now`, and `threshold`, so this is exercised both by unit
/// tests and by the LocalStack/DynamoDB-Local integration test (which scans real rows and feeds
/// them here). A row missing a `status` attribute is not a transcode in flight, so it is ignored.
pub fn find_stuck(
    items: &[HashMap<String, AttributeValue>],
    now: DateTime<Utc>,
    threshold: Duration,
) -> Vec<StuckVideo> {
    items
        .iter()
        .filter_map(|item| {
            let status = attr_s(item, "status")?;
            let updated_at = attr_s(item, "updated_at");
            is_stuck(status, updated_at, now, threshold).then(|| StuckVideo {
                video_id: attr_s(item, "video_id").unwrap_or("<unknown>").to_string(),
                updated_at: updated_at.map(str::to_string),
            })
        })
        .collect()
}

/// Emit the `Rewind/Transcode StuckTranscodes` count (dimensioned by `Region`) — emitted every run,
/// including `0`, so the alarm has a fresh datapoint each tick and can clear once jobs unstick.
/// Best-effort: a CloudWatch error is logged, never fatal (the sweep's job is detection, and a
/// failed emit will simply be retried next tick).
pub async fn emit_stuck_count(client: &CwClient, region: &str, count: usize) {
    let datum = MetricDatum::builder()
        .metric_name(STUCK_METRIC)
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
        Ok(_) => tracing::info!(count, "emitted {METRIC_NAMESPACE}/{STUCK_METRIC} metric"),
        Err(e) => tracing::error!(error = %e, "failed to emit StuckTranscodes metric"),
    }
}

/// Run one reconcile sweep: scan the `videos` (Global) table, flag stranded transcodes, log each,
/// and emit the stuck-count metric. **Detect + alarm only** — no row is mutated and nothing is
/// re-enqueued (automated re-drive is deferred). When `cw` is `None`
/// (e.g. the integration test) the metric emit is skipped. Returns the number of stuck videos.
pub async fn run_sweep(
    db: &aws_sdk_dynamodb::Client,
    cw: Option<&CwClient>,
    region: &str,
    threshold: Duration,
) -> Result<usize, aws_sdk_dynamodb::Error> {
    let items = shared::dynamo::scan_all(db, &shared::tables::table("videos")).await?;
    let stuck = find_stuck(&items, Utc::now(), threshold);

    if stuck.is_empty() {
        tracing::info!(
            scanned = items.len(),
            "reconcile sweep: no stuck transcodes"
        );
    } else {
        for s in &stuck {
            tracing::warn!(
                video_id = %s.video_id,
                updated_at = ?s.updated_at,
                "stuck transcode detected (status=processing past threshold)"
            );
        }
        tracing::warn!(
            scanned = items.len(),
            stuck = stuck.len(),
            "reconcile sweep: stuck transcodes detected — see CloudWatch alarm"
        );
    }

    if let Some(cw) = cw {
        emit_stuck_count(cw, region, stuck.len()).await;
    }
    Ok(stuck.len())
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
        "2026-06-18T08:00:00Z".parse().unwrap()
    }

    fn ago(mins: i64) -> String {
        (now() - Duration::minutes(mins)).to_rfc3339()
    }

    fn threshold() -> Duration {
        Duration::minutes(DEFAULT_STUCK_THRESHOLD_MINS)
    }

    #[test]
    fn processing_older_than_threshold_is_stuck() {
        assert!(is_stuck("processing", Some(&ago(61)), now(), threshold()));
        assert!(is_stuck("processing", Some(&ago(600)), now(), threshold()));
    }

    #[test]
    fn processing_within_threshold_is_not_stuck() {
        assert!(!is_stuck("processing", Some(&ago(1)), now(), threshold()));
        assert!(!is_stuck("processing", Some(&ago(59)), now(), threshold()));
    }

    #[test]
    fn processing_exactly_at_threshold_is_not_stuck() {
        // Strictly greater-than, so the boundary is not (yet) stuck.
        assert!(!is_stuck("processing", Some(&ago(60)), now(), threshold()));
    }

    #[test]
    fn non_processing_statuses_are_never_stuck() {
        for status in ["draft", "published", "failed", "deleted"] {
            assert!(
                !is_stuck(status, Some(&ago(600)), now(), threshold()),
                "{status} should never be stuck"
            );
        }
    }

    #[test]
    fn unknown_status_is_not_stuck() {
        assert!(!is_stuck("ready", Some(&ago(600)), now(), threshold()));
        assert!(!is_stuck("", Some(&ago(600)), now(), threshold()));
    }

    #[test]
    fn processing_with_missing_timestamp_is_flagged() {
        assert!(is_stuck("processing", None, now(), threshold()));
    }

    #[test]
    fn processing_with_unparseable_timestamp_is_flagged() {
        assert!(is_stuck(
            "processing",
            Some("not-a-date"),
            now(),
            threshold()
        ));
        assert!(is_stuck("processing", Some(""), now(), threshold()));
    }

    #[test]
    fn timezone_offsets_are_normalized_to_utc() {
        // 09:30 +02:00 == 07:30Z, i.e. 30 minutes ago — within threshold, not stuck.
        assert!(!is_stuck(
            "processing",
            Some("2026-06-18T09:30:00+02:00"),
            now(),
            threshold()
        ));
        // 08:30 +02:00 == 06:30Z, i.e. 90 minutes ago — stuck.
        assert!(is_stuck(
            "processing",
            Some("2026-06-18T08:30:00+02:00"),
            now(),
            threshold()
        ));
    }

    #[test]
    fn threshold_from_env_defaults_when_unset_or_invalid() {
        // Note: relies on the var being unset in the test environment.
        std::env::remove_var("RECONCILE_STUCK_THRESHOLD_MINS");
        assert_eq!(
            threshold_from_env(),
            Duration::minutes(DEFAULT_STUCK_THRESHOLD_MINS)
        );
    }

    #[test]
    fn find_stuck_returns_only_stranded_processing_rows() {
        let items = vec![
            item(&[
                ("video_id", "stuck-1"),
                ("status", "processing"),
                ("updated_at", &ago(120)),
            ]),
            item(&[
                ("video_id", "fresh"),
                ("status", "processing"),
                ("updated_at", &ago(5)),
            ]),
            item(&[
                ("video_id", "done"),
                ("status", "published"),
                ("updated_at", &ago(120)),
            ]),
            item(&[
                ("video_id", "gone"),
                ("status", "deleted"),
                ("updated_at", &ago(120)),
            ]),
        ];

        let stuck = find_stuck(&items, now(), threshold());
        assert_eq!(stuck.len(), 1);
        assert_eq!(stuck[0].video_id, "stuck-1");
        assert_eq!(stuck[0].updated_at, Some(ago(120)));
    }

    #[test]
    fn find_stuck_flags_processing_row_missing_updated_at() {
        let items = vec![item(&[("video_id", "no-ts"), ("status", "processing")])];
        let stuck = find_stuck(&items, now(), threshold());
        assert_eq!(stuck.len(), 1);
        assert_eq!(stuck[0].video_id, "no-ts");
        assert_eq!(stuck[0].updated_at, None);
    }

    #[test]
    fn find_stuck_ignores_rows_without_a_status_attribute() {
        let items = vec![item(&[("video_id", "weird"), ("updated_at", &ago(120))])];
        assert!(find_stuck(&items, now(), threshold()).is_empty());
    }

    #[test]
    fn find_stuck_empty_input_is_empty() {
        assert!(find_stuck(&[], now(), threshold()).is_empty());
    }
}
