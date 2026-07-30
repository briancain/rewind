use aws_sdk_dynamodb::types::AttributeValue;
use shared::video::VideoStatus;
use std::collections::HashMap;
use transcode::state::AppState;

async fn setup() -> AppState {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let sqs_endpoint = std::env::var("SQS_ENDPOINT").unwrap_or("http://localhost:4566".to_string());

    let config = shared::config::ServiceConfig::from_env("transcode");
    let db = shared::dynamo::create_client(&config).await;

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;

    let sqs_config = aws_sdk_sqs::config::Builder::from(&aws_config)
        .endpoint_url(&sqs_endpoint)
        .build();
    let sqs = aws_sdk_sqs::Client::from_conf(sqs_config);

    // Create a unique queue for this test
    let queue_name = format!("transcode-test-{}", uuid::Uuid::new_v4());
    let queue_resp = sqs
        .create_queue()
        .queue_name(&queue_name)
        .send()
        .await
        .unwrap();
    let queue_url = queue_resp.queue_url().unwrap().to_string();

    // Create videos table
    let _ = db
        .create_table()
        .table_name("test_videos")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("video_id")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    AppState {
        db,
        sqs,
        s3: aws_sdk_s3::Client::new(&aws_config),
        mediaconvert: None, // No MediaConvert in tests
        queue_url,
        output_bucket: "rewind-video-test".to_string(),
        mediaconvert_role: "arn:aws:iam::000000000000:role/test".to_string(),
        cdn_base_url: "https://cdn.example.com".to_string(),
        completion_queue_url: None,
    }
}

#[tokio::test]
async fn consumes_message_and_updates_video() {
    let state = setup().await;
    let video_id = format!("vid-{}", uuid::Uuid::new_v4());

    // Create a video record in DDB
    let mut item = HashMap::new();
    item.insert("video_id".to_string(), AttributeValue::S(video_id.clone()));
    item.insert("status".to_string(), AttributeValue::S("draft".to_string()));
    item.insert(
        "title".to_string(),
        AttributeValue::S("Test Video".to_string()),
    );
    shared::dynamo::put_item(&state.db, &shared::tables::table("videos"), item)
        .await
        .unwrap();

    // Send a transcode job message to SQS
    let msg = serde_json::json!({
        "video_id": video_id,
        "s3_key": format!("raw/{}/video.mp4", video_id),
        "bucket": "rewind-raw-test"
    });

    state
        .sqs
        .send_message()
        .queue_url(&state.queue_url)
        .message_body(msg.to_string())
        .send()
        .await
        .unwrap();

    // Run one poll cycle (with short wait)
    // We can't use the long-poll loop directly, so we'll replicate the logic
    let resp = state
        .sqs
        .receive_message()
        .queue_url(&state.queue_url)
        .max_number_of_messages(1)
        .wait_time_seconds(5)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.messages().len(), 1);
    let msg_body = resp.messages()[0].body().unwrap();
    let job: transcode::models::TranscodeJob = serde_json::from_str(msg_body).unwrap();
    assert_eq!(job.video_id, video_id);

    // Process the job using repo directly (since consumer::run is an infinite loop)
    transcode::repo::update_video_status(
        &state.db,
        &job.video_id,
        VideoStatus::Processing,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let manifest = format!("https://cdn.example.com/hls/{}/manifest.m3u8", video_id);
    let thumb = format!("https://cdn.example.com/thumbnails/{}/thumb.jpg", video_id);
    transcode::repo::update_video_status(
        &state.db,
        &job.video_id,
        VideoStatus::Published,
        Some(&manifest),
        Some(&thumb),
        None,
        Some(120.0),
    )
    .await
    .unwrap();

    // Verify DDB was updated
    let mut key = HashMap::new();
    key.insert("video_id".to_string(), AttributeValue::S(video_id.clone()));
    let item = shared::dynamo::get_item(&state.db, &shared::tables::table("videos"), key)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        item.get("status").and_then(|v| v.as_s().ok()).unwrap(),
        "published"
    );
    assert!(item
        .get("manifest_url")
        .and_then(|v| v.as_s().ok())
        .unwrap()
        .contains(&video_id));
    assert!(item
        .get("thumbnail_url")
        .and_then(|v| v.as_s().ok())
        .unwrap()
        .contains(&video_id));

    // Delete the message
    let receipt = resp.messages()[0].receipt_handle().unwrap();
    state
        .sqs
        .delete_message()
        .queue_url(&state.queue_url)
        .receipt_handle(receipt)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn invalid_message_does_not_crash() {
    let state = setup().await;

    // Send an invalid message
    state
        .sqs
        .send_message()
        .queue_url(&state.queue_url)
        .message_body("not valid json {{{")
        .send()
        .await
        .unwrap();

    // Receive it — parsing should fail gracefully
    let resp = state
        .sqs
        .receive_message()
        .queue_url(&state.queue_url)
        .max_number_of_messages(1)
        .wait_time_seconds(5)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.messages().len(), 1);
    let body = resp.messages()[0].body().unwrap();
    let result = serde_json::from_str::<transcode::models::TranscodeJob>(body);
    assert!(result.is_err()); // Graceful failure, no panic
}

// A MediaConvert COMPLETE event (frame 0 + the ~25% capture) should publish the video and persist
// the SECOND frame as the thumbnail — never the black frame 0. Exercises parse_completion ->
// apply_outcome -> DynamoDB against DynamoDB Local. Mirrors the search sync_test pattern.
#[tokio::test]
async fn completion_event_publishes_video_with_poster_frame() {
    let state = setup().await;
    let video_id = format!("vid-{}", uuid::Uuid::new_v4());

    // Seed a video that is mid-transcode.
    let mut item = HashMap::new();
    item.insert("video_id".to_string(), AttributeValue::S(video_id.clone()));
    item.insert(
        "status".to_string(),
        AttributeValue::S("processing".to_string()),
    );
    shared::dynamo::put_item(&state.db, &shared::tables::table("videos"), item)
        .await
        .unwrap();

    // Synthetic EventBridge COMPLETE event with two frame-capture JPGs (frame 0 + ~25%).
    let event = format!(
        r#"{{
          "detail": {{
            "status": "COMPLETE",
            "userMetadata": {{ "video_id": "{video_id}" }},
            "outputGroupDetails": [
              {{
                "type": "HLS_GROUP",
                "outputDetails": [
                  {{ "outputFilePaths": ["s3://rewind-video-test/hls/{video_id}/clip.m3u8"], "durationInMs": 120000 }}
                ],
                "playlistFilePaths": ["s3://rewind-video-test/hls/{video_id}/clip.m3u8"]
              }},
              {{
                "type": "FILE_GROUP",
                "outputDetails": [
                  {{ "outputFilePaths": ["s3://rewind-video-test/mp4/{video_id}/video.mp4"], "durationInMs": 120000 }}
                ]
              }},
              {{
                "type": "FILE_GROUP",
                "outputDetails": [
                  {{ "outputFilePaths": [
                      "s3://rewind-video-test/thumbnails/{video_id}/clipthumb.0000000.jpg",
                      "s3://rewind-video-test/thumbnails/{video_id}/clipthumb.0000001.jpg"
                  ] }}
                ]
              }}
            ]
          }}
        }}"#
    );

    let outcome = transcode::completion::parse_completion(&event);
    transcode::completion::apply_outcome(&state, outcome)
        .await
        .unwrap();

    let mut key = HashMap::new();
    key.insert("video_id".to_string(), AttributeValue::S(video_id.clone()));
    let row = shared::dynamo::get_item(&state.db, &shared::tables::table("videos"), key)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        row.get("status").and_then(|v| v.as_s().ok()).unwrap(),
        "published"
    );
    // The poster frame must be the ~25% capture (frame 1), not the black frame 0.
    assert_eq!(
        row.get("thumbnail_url")
            .and_then(|v| v.as_s().ok())
            .unwrap(),
        &format!("thumbnails/{video_id}/clipthumb.0000001.jpg")
    );
    // Manifest stored as a CloudFront URL derived from the HLS playlist key.
    assert_eq!(
        row.get("manifest_url").and_then(|v| v.as_s().ok()).unwrap(),
        &format!("https://cdn.example.com/hls/{video_id}/clip.m3u8")
    );
    assert_eq!(
        row.get("s3_key").and_then(|v| v.as_s().ok()).unwrap(),
        &format!("mp4/{video_id}/video.mp4")
    );
}

// --- Resurrection guard ---
// A video can be deleted (soft-deleted to status="deleted") while it is still transcoding. A late
// MediaConvert completion event must NOT re-publish it. These tests exercise the conditional write
// in `update_video_status` directly against DynamoDB-Local (no SQS needed).

async fn videos_db() -> aws_sdk_dynamodb::Client {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");
    let config = shared::config::ServiceConfig::from_env("transcode");
    let db = shared::dynamo::create_client(&config).await;
    // Create the videos table if it isn't already present (ignore the error when it exists).
    let _ = db
        .create_table()
        .table_name("test_videos")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("video_id")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;
    db
}

async fn seed_video(db: &aws_sdk_dynamodb::Client, video_id: &str, status: &str) {
    db.put_item()
        .table_name("test_videos")
        .item(
            "video_id",
            aws_sdk_dynamodb::types::AttributeValue::S(video_id.to_string()),
        )
        .item(
            "status",
            aws_sdk_dynamodb::types::AttributeValue::S(status.to_string()),
        )
        .item(
            "channel_id",
            aws_sdk_dynamodb::types::AttributeValue::S("u1".to_string()),
        )
        .send()
        .await
        .unwrap();
}

async fn status_of(db: &aws_sdk_dynamodb::Client, video_id: &str) -> String {
    let resp = db
        .get_item()
        .table_name("test_videos")
        .key(
            "video_id",
            aws_sdk_dynamodb::types::AttributeValue::S(video_id.to_string()),
        )
        .send()
        .await
        .unwrap();
    resp.item()
        .unwrap()
        .get("status")
        .unwrap()
        .as_s()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn update_status_skips_deleted_video() {
    let db = videos_db().await;
    let video_id = format!("resguard-deleted-{}", uuid::Uuid::new_v4());
    seed_video(&db, &video_id, "deleted").await;

    // A late completion tries to publish a video that was deleted while transcoding.
    transcode::repo::update_video_status(
        &db,
        &video_id,
        VideoStatus::Published,
        Some("https://cdn.example.com/hls/x/master.m3u8"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        status_of(&db, &video_id).await,
        "deleted",
        "resurrection guard must not republish a soft-deleted video"
    );
}

#[tokio::test]
async fn update_status_publishes_live_video() {
    let db = videos_db().await;
    let video_id = format!("resguard-live-{}", uuid::Uuid::new_v4());
    seed_video(&db, &video_id, "processing").await;

    transcode::repo::update_video_status(
        &db,
        &video_id,
        VideoStatus::Published,
        Some("https://cdn.example.com/hls/x/master.m3u8"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        status_of(&db, &video_id).await,
        "published",
        "a non-deleted video must still publish normally"
    );
}

// --- stuck-`processing` reconciler (detect) ---
// Seeds processing/published rows with explicit `updated_at` timestamps into DynamoDB-Local, scans
// the table the way the sweep does (`scan_all`), and asserts the pure `find_stuck` decision flags
// exactly the stranded row. No CloudWatch (detect path only).

async fn seed_video_with_updated_at(
    db: &aws_sdk_dynamodb::Client,
    video_id: &str,
    status: &str,
    updated_at: &str,
) {
    db.put_item()
        .table_name("test_videos")
        .item("video_id", AttributeValue::S(video_id.to_string()))
        .item("status", AttributeValue::S(status.to_string()))
        .item("updated_at", AttributeValue::S(updated_at.to_string()))
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn reconcile_sweep_flags_only_the_stranded_processing_row() {
    use chrono::{Duration, Utc};

    let db = videos_db().await;
    let now = Utc::now();
    let run = uuid::Uuid::new_v4();

    let stuck_id = format!("reconcile-stuck-{run}");
    let fresh_id = format!("reconcile-fresh-{run}");
    let published_id = format!("reconcile-published-{run}");

    // Stranded: processing for 2 hours.
    seed_video_with_updated_at(
        &db,
        &stuck_id,
        "processing",
        &(now - Duration::minutes(120)).to_rfc3339(),
    )
    .await;
    // Healthy in-flight: processing for 5 minutes.
    seed_video_with_updated_at(
        &db,
        &fresh_id,
        "processing",
        &(now - Duration::minutes(5)).to_rfc3339(),
    )
    .await;
    // Long-finished: published 2 hours ago — must never be flagged.
    seed_video_with_updated_at(
        &db,
        &published_id,
        "published",
        &(now - Duration::minutes(120)).to_rfc3339(),
    )
    .await;

    // Scan the table exactly as run_sweep does, then apply the pure decision.
    let items = shared::dynamo::scan_all(&db, &shared::tables::table("videos"))
        .await
        .unwrap();
    let stuck_ids: std::collections::HashSet<String> =
        transcode::reconcile::find_stuck(&items, now, Duration::minutes(60))
            .into_iter()
            .map(|s| s.video_id)
            .collect();

    // Scoped to this run's IDs (the shared test table holds rows from other tests).
    assert!(
        stuck_ids.contains(&stuck_id),
        "the 2h-old processing row must be flagged stuck"
    );
    assert!(
        !stuck_ids.contains(&fresh_id),
        "a 5m-old processing row must not be flagged"
    );
    assert!(
        !stuck_ids.contains(&published_id),
        "a published row must never be flagged"
    );
}
