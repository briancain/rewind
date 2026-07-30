//! Integration tests for the delete-cleanup worker.
//!
//! Mirrors `search/tests/sync_test.rs`: drives synthetic EventBridge-Pipes stream messages through
//! the SQS consumer against the local stack (DynamoDB Local + LocalStack S3/SQS). The Pipe trigger
//! and its `status == "deleted"` filter are infrastructure (no Pipes in LocalStack) and are
//! validated in cloud; here we prove the worker reclaims exactly the right data, only on a
//! soft-delete, and idempotently.
//!
//! Requires the local stack (the same one the CI `rust` job and `cargo test --all` use).

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ProvisionedThroughput, ScalarAttributeType,
};
use aws_sdk_s3::primitives::ByteStream;
use delete_cleanup::state::AppState;
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

const VIDEO_BUCKET: &str = "test-cleanup-videos";
const RAW_BUCKET: &str = "test-cleanup-raw";

// --- setup -------------------------------------------------------------------------------------

async fn setup() -> AppState {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("S3_ENDPOINT", "http://localhost:4566");
    std::env::set_var("SQS_ENDPOINT", "http://localhost:4566");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let config = shared::config::ServiceConfig::from_env("delete-cleanup");
    let db = shared::dynamo::create_client(&config).await;
    let s3 = shared::aws::s3_client().await;
    let sqs = shared::aws::sqs_client().await;

    create_tables(&db).await;
    create_bucket(&s3, VIDEO_BUCKET).await;
    create_bucket(&s3, RAW_BUCKET).await;
    let queue_url = create_fifo_queue(&sqs).await;

    AppState {
        db,
        s3,
        sqs,
        queue_url,
        video_bucket: VIDEO_BUCKET.to_string(),
        raw_bucket: RAW_BUCKET.to_string(),
        cloudfront: None,
        cdn_distribution_id: None,
    }
}

fn pt() -> ProvisionedThroughput {
    ProvisionedThroughput::builder()
        .read_capacity_units(5)
        .write_capacity_units(5)
        .build()
        .unwrap()
}

fn attr(name: &str) -> AttributeDefinition {
    AttributeDefinition::builder()
        .attribute_name(name)
        .attribute_type(ScalarAttributeType::S)
        .build()
        .unwrap()
}

fn key(name: &str, kind: KeyType) -> KeySchemaElement {
    KeySchemaElement::builder()
        .attribute_name(name)
        .key_type(kind)
        .build()
        .unwrap()
}

/// Create a hash (+ optional range) table. Ignores errors so repeated runs are fine.
async fn create_kv_table(
    db: &aws_sdk_dynamodb::Client,
    name: &str,
    hash: &str,
    range: Option<&str>,
) {
    let mut b = db
        .create_table()
        .table_name(name)
        .key_schema(key(hash, KeyType::Hash))
        .attribute_definitions(attr(hash))
        .provisioned_throughput(pt());
    if let Some(r) = range {
        b = b
            .key_schema(key(r, KeyType::Range))
            .attribute_definitions(attr(r));
    }
    let _ = b.send().await;
}

async fn create_tables(db: &aws_sdk_dynamodb::Client) {
    // Delete first so this test's schema (notably the video-id-index GSIs) always wins, regardless
    // of tables left by other suites in the shared DynamoDB Local (mirrors the social test).
    for t in [
        "test_comments",
        "test_reactions",
        "test_comment_reactions",
        "test_video_stats",
        "test_view_history",
        "test_transcode_jobs",
    ] {
        let _ = db.delete_table().table_name(t).send().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Social tables keyed by video_id (matches the cascade schema).
    create_kv_table(db, "test_comments", "video_id", Some("comment_id")).await;
    create_kv_table(db, "test_reactions", "video_id", Some("user_id")).await;
    create_kv_table(db, "test_comment_reactions", "video_id", Some("sk")).await;
    create_kv_table(db, "test_video_stats", "video_id", None).await;

    // view_history: PK=user_id, SK=watched_at, + video-id-index GSI (KEYS_ONLY).
    let _ = db
        .create_table()
        .table_name("test_view_history")
        .key_schema(key("user_id", KeyType::Hash))
        .key_schema(key("watched_at", KeyType::Range))
        .attribute_definitions(attr("user_id"))
        .attribute_definitions(attr("watched_at"))
        .attribute_definitions(attr("video_id"))
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("video-id-index")
                .key_schema(key("video_id", KeyType::Hash))
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::KeysOnly)
                        .build(),
                )
                .provisioned_throughput(pt())
                .build()
                .unwrap(),
        )
        .provisioned_throughput(pt())
        .send()
        .await;

    // transcode_jobs: PK=job_id, + video-id-index GSI (ALL).
    let _ = db
        .create_table()
        .table_name("test_transcode_jobs")
        .key_schema(key("job_id", KeyType::Hash))
        .attribute_definitions(attr("job_id"))
        .attribute_definitions(attr("video_id"))
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("video-id-index")
                .key_schema(key("video_id", KeyType::Hash))
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .provisioned_throughput(pt())
                .build()
                .unwrap(),
        )
        .provisioned_throughput(pt())
        .send()
        .await;

    for t in [
        "test_comments",
        "test_reactions",
        "test_comment_reactions",
        "test_video_stats",
        "test_view_history",
        "test_transcode_jobs",
    ] {
        wait_active(db, t).await;
    }
}

async fn wait_active(db: &aws_sdk_dynamodb::Client, table: &str) {
    for _ in 0..50 {
        if let Ok(resp) = db.describe_table().table_name(table).send().await {
            if let Some(t) = resp.table() {
                let table_ready = t.table_status().map(|s| s.as_str()) == Some("ACTIVE");
                let gsis_ready = t
                    .global_secondary_indexes()
                    .iter()
                    .all(|g| g.index_status().map(|s| s.as_str()) == Some("ACTIVE"));
                if table_ready && gsis_ready {
                    return;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("table {table} did not become ACTIVE");
}

async fn create_bucket(s3: &aws_sdk_s3::Client, name: &str) {
    let _ = s3
        .create_bucket()
        .bucket(name)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::UsWest2)
                .build(),
        )
        .send()
        .await;
}

async fn create_fifo_queue(sqs: &aws_sdk_sqs::Client) -> String {
    let name = format!("delete-cleanup-it-{}.fifo", Uuid::new_v4());
    sqs.create_queue()
        .queue_name(&name)
        .attributes(aws_sdk_sqs::types::QueueAttributeName::FifoQueue, "true")
        .attributes(
            aws_sdk_sqs::types::QueueAttributeName::ContentBasedDeduplication,
            "true",
        )
        .send()
        .await
        .unwrap()
        .queue_url()
        .unwrap()
        .to_string()
}

// --- seeding -----------------------------------------------------------------------------------

fn s(v: &str) -> AttributeValue {
    AttributeValue::S(v.to_string())
}

async fn put(db: &aws_sdk_dynamodb::Client, table: &str, item: HashMap<String, AttributeValue>) {
    db.put_item()
        .table_name(table)
        .set_item(Some(item))
        .send()
        .await
        .unwrap();
}

/// Seed every dependent row + S3 object for `video_id` (two of each row type where the table holds
/// multiple rows per video, to exercise the batch path).
async fn seed_video(state: &AppState, video_id: &str) {
    let db = &state.db;

    for cid in ["c1", "c2"] {
        put(
            db,
            "test_comments",
            HashMap::from([
                ("video_id".into(), s(video_id)),
                ("comment_id".into(), s(cid)),
                ("user_id".into(), s("u1")),
                ("text".into(), s("hi")),
            ]),
        )
        .await;
    }
    for uid in ["u1", "u2"] {
        put(
            db,
            "test_reactions",
            HashMap::from([
                ("video_id".into(), s(video_id)),
                ("user_id".into(), s(uid)),
                ("reaction".into(), s("like")),
            ]),
        )
        .await;
    }
    for sk in ["c1#u1", "c1#u2"] {
        put(
            db,
            "test_comment_reactions",
            HashMap::from([
                ("video_id".into(), s(video_id)),
                ("sk".into(), s(sk)),
                ("reaction_type".into(), s("like")),
            ]),
        )
        .await;
    }
    put(
        db,
        "test_video_stats",
        HashMap::from([
            ("video_id".into(), s(video_id)),
            ("views".into(), AttributeValue::N("42".into())),
        ]),
    )
    .await;
    for (uid, ts) in [
        ("u1", "2026-06-15T00:00:00Z"),
        ("u2", "2026-06-15T01:00:00Z"),
    ] {
        put(
            db,
            "test_view_history",
            HashMap::from([
                ("user_id".into(), s(uid)),
                ("watched_at".into(), s(ts)),
                ("video_id".into(), s(video_id)),
            ]),
        )
        .await;
    }
    put(
        db,
        "test_transcode_jobs",
        HashMap::from([
            ("job_id".into(), s(&format!("job-{video_id}"))),
            ("video_id".into(), s(video_id)),
            ("status".into(), s("done")),
        ]),
    )
    .await;

    // S3 objects across all four prefixes (two under hls/ to exercise multi-object delete).
    for (bucket, key) in [
        (VIDEO_BUCKET, format!("hls/{video_id}/master.m3u8")),
        (VIDEO_BUCKET, format!("hls/{video_id}/seg00001.ts")),
        (VIDEO_BUCKET, format!("mp4/{video_id}/video.mp4")),
        (VIDEO_BUCKET, format!("thumbnails/{video_id}/thumb.jpg")),
        (RAW_BUCKET, format!("raw/{video_id}/input.mp4")),
    ] {
        state
            .s3
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(ByteStream::from_static(b"data"))
            .send()
            .await
            .unwrap();
    }
}

// --- assertions --------------------------------------------------------------------------------

async fn count_rows(
    db: &aws_sdk_dynamodb::Client,
    table: &str,
    index: Option<&str>,
    video_id: &str,
) -> usize {
    let mut req = db
        .query()
        .table_name(table)
        .key_condition_expression("video_id = :v")
        .expression_attribute_values(":v", s(video_id));
    if let Some(idx) = index {
        req = req.index_name(idx);
    }
    req.send().await.unwrap().items().len()
}

async fn stats_exists(db: &aws_sdk_dynamodb::Client, video_id: &str) -> bool {
    db.get_item()
        .table_name("test_video_stats")
        .key("video_id", s(video_id))
        .send()
        .await
        .unwrap()
        .item
        .is_some()
}

async fn s3_count(s3: &aws_sdk_s3::Client, bucket: &str, prefix: &str) -> usize {
    s3.list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .send()
        .await
        .unwrap()
        .contents()
        .len()
}

/// Total dependent rows + objects for a video across every store.
async fn total_dependents(state: &AppState, video_id: &str) -> usize {
    let db = &state.db;
    let mut n = 0;
    n += count_rows(db, "test_comments", None, video_id).await;
    n += count_rows(db, "test_reactions", None, video_id).await;
    n += count_rows(db, "test_comment_reactions", None, video_id).await;
    n += count_rows(db, "test_view_history", Some("video-id-index"), video_id).await;
    n += count_rows(db, "test_transcode_jobs", Some("video-id-index"), video_id).await;
    n += usize::from(stats_exists(db, video_id).await);
    n += s3_count(&state.s3, VIDEO_BUCKET, &format!("hls/{video_id}/")).await;
    n += s3_count(&state.s3, VIDEO_BUCKET, &format!("mp4/{video_id}/")).await;
    n += s3_count(&state.s3, VIDEO_BUCKET, &format!("thumbnails/{video_id}/")).await;
    n += s3_count(&state.s3, RAW_BUCKET, &format!("raw/{video_id}/")).await;
    n
}

fn soft_delete_message(video_id: &str) -> String {
    json!({
        "eventName": "MODIFY",
        "eventSource": "aws:dynamodb",
        "dynamodb": {
            "Keys": { "video_id": {"S": video_id} },
            "NewImage": {
                "video_id": {"S": video_id},
                "status": {"S": "deleted"},
                "channel_id": {"S": "chan-1"}
            }
        }
    })
    .to_string()
}

fn published_message(video_id: &str) -> String {
    json!({
        "eventName": "MODIFY",
        "eventSource": "aws:dynamodb",
        "dynamodb": {
            "Keys": { "video_id": {"S": video_id} },
            "NewImage": {
                "video_id": {"S": video_id},
                "status": {"S": "published"},
                "channel_id": {"S": "chan-1"}
            }
        }
    })
    .to_string()
}

async fn enqueue(state: &AppState, video_id: &str, body: String) {
    state
        .sqs
        .send_message()
        .queue_url(&state.queue_url)
        .message_body(body)
        .message_group_id(video_id)
        .send()
        .await
        .unwrap();
}

// --- tests -------------------------------------------------------------------------------------

#[tokio::test]
async fn soft_delete_reclaims_all_dependent_data_idempotently() {
    let state = setup().await;
    let video_id = format!("vid-{}", Uuid::new_v4());

    seed_video(&state, &video_id).await;
    // 2 comments + 2 reactions + 2 comment_reactions + 2 view_history + 1 transcode_job + 1 stats
    // + (2 hls + 1 mp4 + 1 thumb + 1 raw) = 15 dependents.
    assert_eq!(
        total_dependents(&state, &video_id).await,
        15,
        "seed should create all dependent rows + objects"
    );

    enqueue(&state, &video_id, soft_delete_message(&video_id)).await;
    let n = delete_cleanup::consumer::poll_once(&state).await.unwrap();
    assert!(n >= 1, "expected to receive the soft-delete message");

    assert_eq!(
        total_dependents(&state, &video_id).await,
        0,
        "every dependent row + object must be reclaimed"
    );

    // Idempotency: re-running cleanup on an already-clean video is a no-op (no error, still empty).
    delete_cleanup::cleanup::cleanup_video(&state, &video_id)
        .await
        .expect("idempotent cleanup must not error");
    assert_eq!(total_dependents(&state, &video_id).await, 0);
}

#[tokio::test]
async fn non_delete_event_leaves_data_intact() {
    let state = setup().await;
    let video_id = format!("vid-{}", Uuid::new_v4());

    seed_video(&state, &video_id).await;
    assert_eq!(total_dependents(&state, &video_id).await, 15);

    // A publish/edit event must NOT trigger any deletion.
    enqueue(&state, &video_id, published_message(&video_id)).await;
    delete_cleanup::consumer::poll_once(&state).await.unwrap();

    assert_eq!(
        total_dependents(&state, &video_id).await,
        15,
        "a non-delete event must leave all data intact"
    );
}

// --- cascade-cleanup reconciler (detect) ---
// Seeds `deleted` tombstones (some with leftover dependents, some cleaned, one too-fresh) into the
// videos table, then asserts the pure `find_candidates` filter and the read-only
// `has_remaining_dependents` probe agree on which deletions were never reclaimed. Membership is
// scoped to this run's UUIDs because the shared test table holds rows from other suites (mirrors the
// transcode reconcile test). No CloudWatch (detect path only).

/// Idempotently create the videos table (PK = video_id) in the shared local DynamoDB. The
/// delete-cleanup worker doesn't need it, but the reconcile sweep scans it.
async fn create_videos_table(db: &aws_sdk_dynamodb::Client) {
    create_kv_table(db, "test_videos", "video_id", None).await;
    wait_active(db, "test_videos").await;
}

/// Put a `deleted` tombstone row with the given `deleted_at` into the videos table.
async fn put_video_tombstone(state: &AppState, video_id: &str, deleted_at: &str) {
    put(
        &state.db,
        "test_videos",
        HashMap::from([
            ("video_id".into(), s(video_id)),
            ("status".into(), s("deleted")),
            ("deleted_at".into(), s(deleted_at)),
        ]),
    )
    .await;
}

#[tokio::test]
async fn reconcile_sweep_detects_only_unreclaimed_tombstones() {
    use chrono::{Duration, Utc};

    let state = setup().await;
    create_videos_table(&state.db).await;
    let now = Utc::now();
    let run = Uuid::new_v4();

    // Orphan: an old tombstone whose dependent data was never reclaimed.
    let orphan = format!("recon-orphan-{run}");
    seed_video(&state, &orphan).await;
    put_video_tombstone(
        &state,
        &orphan,
        &(now - Duration::minutes(120)).to_rfc3339(),
    )
    .await;

    // Cleaned: an old tombstone with no dependents left (cleanup succeeded).
    let clean = format!("recon-clean-{run}");
    put_video_tombstone(&state, &clean, &(now - Duration::minutes(120)).to_rfc3339()).await;

    // Fresh: a recent tombstone WITH dependents — cleanup may still be legitimately in flight, so it
    // must not even be a candidate.
    let fresh = format!("recon-fresh-{run}");
    seed_video(&state, &fresh).await;
    put_video_tombstone(&state, &fresh, &(now - Duration::minutes(1)).to_rfc3339()).await;

    // 1) Pure candidate filter over a real scan, scoped to this run's IDs (the shared test table
    //    holds rows from other suites). Both old tombstones are candidates; the probe decides which
    //    is actually orphaned. The fresh one is excluded by the threshold.
    let items = shared::dynamo::scan_all(&state.db, &shared::tables::table("videos"))
        .await
        .unwrap();
    let candidates: std::collections::HashSet<String> =
        delete_cleanup::reconcile::find_candidates(&items, now, Duration::minutes(30))
            .into_iter()
            .map(|c| c.video_id)
            .collect();
    assert!(
        candidates.contains(&orphan),
        "a 2h-old tombstone must be a candidate"
    );
    assert!(
        candidates.contains(&clean),
        "a 2h-old tombstone is a candidate regardless of cleanup (the probe decides)"
    );
    assert!(
        !candidates.contains(&fresh),
        "a 1m-old tombstone must not be a candidate (don't race in-flight cleanup)"
    );

    // 2) The read-only probe distinguishes the orphan (leftovers remain) from the cleaned tombstone.
    assert!(
        delete_cleanup::reconcile::has_remaining_dependents(
            &state.db,
            &state.s3,
            &state.video_bucket,
            &state.raw_bucket,
            &orphan,
        )
        .await
        .unwrap(),
        "the orphan still has dependent rows + objects"
    );
    assert!(
        !delete_cleanup::reconcile::has_remaining_dependents(
            &state.db,
            &state.s3,
            &state.video_bucket,
            &state.raw_bucket,
            &clean,
        )
        .await
        .unwrap(),
        "the cleaned tombstone has no dependent data left"
    );
}
