use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use serde_json::Value;
use tower::ServiceExt;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, KeySchemaElement, KeyType, ProvisionedThroughput,
    ScalarAttributeType,
};

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn setup() -> Router {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("S3_ENDPOINT", "http://localhost:4566");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let config = shared::config::ServiceConfig::from_env("streaming");
    let db = shared::dynamo::create_client(&config).await;

    let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://localhost:4566")
        .load()
        .await;
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(&s3_config)
            .force_path_style(true)
            .build(),
    );

    // Create videos table
    let _ = db.delete_table().table_name("test_videos").send().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let _ = db
        .create_table()
        .table_name("test_videos")
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

    // Insert a test video with s3_key
    let mut item = std::collections::HashMap::new();
    item.insert("video_id".into(), AttributeValue::S("vid-1".into()));
    item.insert(
        "s3_key".into(),
        AttributeValue::S("uploads/vid-1/original.mp4".into()),
    );
    item.insert("status".into(), AttributeValue::S("ready".into()));
    db.put_item()
        .table_name("test_videos")
        .set_item(Some(item))
        .send()
        .await
        .unwrap();

    // Create S3 bucket
    let _ = s3
        .create_bucket()
        .bucket("rewind-videos")
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::UsWest2)
                .build(),
        )
        .send()
        .await;

    // Upload a dummy object
    s3.put_object()
        .bucket("rewind-videos")
        .key("uploads/vid-1/original.mp4")
        .body(aws_sdk_s3::primitives::ByteStream::from_static(
            b"fake video data",
        ))
        .send()
        .await
        .unwrap();

    let state = streaming::state::AppState {
        db,
        s3,
        bucket: "rewind-videos".into(),
    };

    Router::new()
        .route(
            "/videos/{id}/stream-url",
            get(streaming::handlers::stream_url),
        )
        .with_state(state)
}

#[tokio::test]
async fn stream_url_returns_presigned_url() {
    let app = setup().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/videos/vid-1/stream-url")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["video_id"], "vid-1");
    assert!(body["url"].as_str().unwrap().contains("rewind-videos"));
    assert!(body["url"].as_str().unwrap().contains("X-Amz-Signature"));
    assert_eq!(body["expires_in_secs"], 3600);
}

#[tokio::test]
async fn stream_url_not_found() {
    let app = setup().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/videos/nonexistent/stream-url")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
