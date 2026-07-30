//! Integration tests for the search index sync pipeline:
//! - the SQS consumer applying synthetic EventBridge-Pipes stream messages, and
//! - the /reindex backfill scanning the videos table.
//!
//! Requires the local stack (LocalStack SQS, DynamoDB Local, OpenSearch), exactly like the CI
//! `rust` job. Document presence is checked via OpenSearch GET _doc/{id}, which is realtime and so
//! needs no index refresh.

use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, KeySchemaElement, KeyType, ProvisionedThroughput,
    ScalarAttributeType,
};
use search::signing::SearchClient;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

fn opensearch_url() -> String {
    std::env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| "http://localhost:9200".into())
}

/// Drop the `videos` index so each test starts from a clean state.
async fn clean_index(opensearch_url: &str) {
    let http = reqwest::Client::new();
    let _ = http
        .delete(format!("{}/videos", opensearch_url))
        .send()
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// True if the document exists in the index. GET by id is realtime (no refresh needed).
async fn doc_exists(opensearch_url: &str, id: &str) -> bool {
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/videos/_doc/{}", opensearch_url, id))
        .send()
        .await
        .unwrap();
    resp.status().is_success()
}

/// A DynamoDB image in attribute-value JSON, as it appears in a stream record / scan item.
fn image(video_id: &str, status: &str, visibility: &str) -> Value {
    json!({
        "video_id": {"S": video_id},
        "title": {"S": format!("Title {video_id}")},
        "description": {"S": "desc"},
        "tags": {"L": [{"S": "tag1"}]},
        "channel_id": {"S": "chan-1"},
        "genre": {"S": "tech"},
        "status": {"S": status},
        "visibility": {"S": visibility},
        "created_at": {"S": "2026-06-11T00:00:00Z"}
    })
}

/// Build a synthetic EventBridge-Pipes message body for a DynamoDB stream record.
fn pipe_message(event: &str, video_id: &str, status: &str, visibility: &str) -> String {
    let mut dynamodb = json!({ "Keys": { "video_id": {"S": video_id} } });
    if event == "REMOVE" {
        dynamodb["OldImage"] = image(video_id, status, visibility);
    } else {
        dynamodb["NewImage"] = image(video_id, status, visibility);
    }
    json!({ "eventName": event, "eventSource": "aws:dynamodb", "dynamodb": dynamodb }).to_string()
}

async fn sqs_client() -> aws_sdk_sqs::Client {
    let endpoint = std::env::var("SQS_ENDPOINT").unwrap_or_else(|_| "http://localhost:4566".into());
    let cfg = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let scfg = aws_sdk_sqs::config::Builder::from(&cfg)
        .endpoint_url(endpoint)
        .build();
    aws_sdk_sqs::Client::from_conf(scfg)
}

/// Create a unique standard queue for a test. (The consumer's poll/process logic is identical for
/// standard and FIFO queues; FIFO ordering is an infra concern validated by Terraform.)
async fn make_queue(sqs: &aws_sdk_sqs::Client) -> String {
    let name = format!("search-it-{}", Uuid::new_v4());
    sqs.create_queue()
        .queue_name(&name)
        .send()
        .await
        .unwrap()
        .queue_url()
        .unwrap()
        .to_string()
}

async fn send(sqs: &aws_sdk_sqs::Client, queue_url: &str, body: String) {
    sqs.send_message()
        .queue_url(queue_url)
        .message_body(body)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn consumer_indexes_then_removes_on_visibility_change() {
    let opensearch_url = opensearch_url();
    clean_index(&opensearch_url).await;
    let client = SearchClient::new(&opensearch_url).await.unwrap();
    let sqs = sqs_client().await;
    let queue_url = make_queue(&sqs).await;

    // INSERT a published+public video -> it should be indexed.
    send(
        &sqs,
        &queue_url,
        pipe_message("INSERT", "sync-a", "published", "public"),
    )
    .await;
    let n = search::consumer::poll_once(&sqs, &queue_url, &client)
        .await
        .unwrap();
    assert!(n >= 1, "expected to receive the insert message");
    assert!(
        doc_exists(&opensearch_url, "sync-a").await,
        "published+public video should be indexed"
    );

    // MODIFY it to private -> it should be removed (the visibility-drift fix).
    send(
        &sqs,
        &queue_url,
        pipe_message("MODIFY", "sync-a", "published", "private"),
    )
    .await;
    search::consumer::poll_once(&sqs, &queue_url, &client)
        .await
        .unwrap();
    assert!(
        !doc_exists(&opensearch_url, "sync-a").await,
        "video flipped to private should be removed from the index"
    );
}

#[tokio::test]
async fn consumer_draft_not_indexed_and_remove_deletes() {
    let opensearch_url = opensearch_url();
    clean_index(&opensearch_url).await;
    let client = SearchClient::new(&opensearch_url).await.unwrap();
    let sqs = sqs_client().await;
    let queue_url = make_queue(&sqs).await;

    // A brand-new draft must not be indexed.
    send(
        &sqs,
        &queue_url,
        pipe_message("INSERT", "sync-draft", "draft", "public"),
    )
    .await;
    search::consumer::poll_once(&sqs, &queue_url, &client)
        .await
        .unwrap();
    assert!(
        !doc_exists(&opensearch_url, "sync-draft").await,
        "draft video should not be indexed"
    );

    // Publish another video, then REMOVE it -> it should be gone.
    send(
        &sqs,
        &queue_url,
        pipe_message("INSERT", "sync-b", "published", "public"),
    )
    .await;
    search::consumer::poll_once(&sqs, &queue_url, &client)
        .await
        .unwrap();
    assert!(
        doc_exists(&opensearch_url, "sync-b").await,
        "should be indexed"
    );

    send(
        &sqs,
        &queue_url,
        pipe_message("REMOVE", "sync-b", "published", "public"),
    )
    .await;
    search::consumer::poll_once(&sqs, &queue_url, &client)
        .await
        .unwrap();
    assert!(
        !doc_exists(&opensearch_url, "sync-b").await,
        "deleted video should be removed from the index"
    );
}

// --- /reindex backfill ---

async fn dynamo_client() -> aws_sdk_dynamodb::Client {
    let endpoint =
        std::env::var("DYNAMODB_ENDPOINT").unwrap_or_else(|_| "http://localhost:8000".into());
    let cfg = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let dcfg = aws_sdk_dynamodb::config::Builder::from(&cfg)
        .endpoint_url(endpoint)
        .build();
    aws_sdk_dynamodb::Client::from_conf(dcfg)
}

async fn create_videos_table(db: &aws_sdk_dynamodb::Client, table: &str) {
    let _ = db
        .create_table()
        .table_name(table)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("video_id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;
}

async fn put_video(
    db: &aws_sdk_dynamodb::Client,
    table: &str,
    video_id: &str,
    status: &str,
    visibility: &str,
) {
    let mut item = HashMap::new();
    item.insert("video_id".to_string(), AttributeValue::S(video_id.into()));
    item.insert("status".to_string(), AttributeValue::S(status.into()));
    item.insert(
        "visibility".to_string(),
        AttributeValue::S(visibility.into()),
    );
    item.insert(
        "title".to_string(),
        AttributeValue::S(format!("Title {video_id}")),
    );
    item.insert(
        "tags".to_string(),
        AttributeValue::L(vec![AttributeValue::S("tag1".into())]),
    );
    item.insert("channel_id".to_string(), AttributeValue::S("chan-1".into()));
    db.put_item()
        .table_name(table)
        .set_item(Some(item))
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn reindex_backfills_only_public_published() {
    let opensearch_url = opensearch_url();
    clean_index(&opensearch_url).await;
    let client = SearchClient::new(&opensearch_url).await.unwrap();

    // Isolate this test with a unique table prefix (reindex scans tables::table("videos")).
    let prefix = format!("searchit-{}-", Uuid::new_v4());
    std::env::set_var("TABLE_PREFIX", &prefix);
    let table = format!("{prefix}videos");

    let db = dynamo_client().await;
    create_videos_table(&db, &table).await;

    // Mixed table: two eligible, two not.
    put_video(&db, &table, "pub-1", "published", "public").await;
    put_video(&db, &table, "pub-2", "published", "public").await;
    put_video(&db, &table, "priv-1", "published", "private").await;
    put_video(&db, &table, "draft-1", "draft", "public").await;

    let report = search::backfill::reindex(&db, &client).await.unwrap();

    assert_eq!(report.scanned, 4, "scanned all items");
    assert_eq!(report.indexed, 2, "only public+published indexed");
    assert_eq!(report.deleted, 2, "private + draft reconciled as deletes");

    assert!(doc_exists(&opensearch_url, "pub-1").await);
    assert!(doc_exists(&opensearch_url, "pub-2").await);
    assert!(!doc_exists(&opensearch_url, "priv-1").await);
    assert!(!doc_exists(&opensearch_url, "draft-1").await);

    std::env::remove_var("TABLE_PREFIX");
}
