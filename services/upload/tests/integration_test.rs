use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use upload::{handlers, state::AppState};

async fn setup() -> (Router, String) {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let s3_endpoint = std::env::var("S3_ENDPOINT").unwrap_or("http://localhost:4566".to_string());
    let sqs_endpoint = std::env::var("SQS_ENDPOINT").unwrap_or("http://localhost:4566".to_string());

    let config = shared::config::ServiceConfig::from_env("upload");
    let db = shared::dynamo::create_client(&config).await;

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;

    let s3_config = aws_sdk_s3::config::Builder::from(&aws_config)
        .endpoint_url(&s3_endpoint)
        .force_path_style(true)
        .build();
    let s3 = aws_sdk_s3::Client::from_conf(s3_config);

    let sqs_config = aws_sdk_sqs::config::Builder::from(&aws_config)
        .endpoint_url(&sqs_endpoint)
        .build();
    let sqs = aws_sdk_sqs::Client::from_conf(sqs_config);

    let bucket = "rewind-raw-test".to_string();
    let queue_name = format!("transcode-jobs-{}", uuid::Uuid::new_v4());

    // Create S3 bucket
    let bucket_result = s3
        .create_bucket()
        .bucket(&bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::UsWest2)
                .build(),
        )
        .send()
        .await;
    if let Err(e) = &bucket_result {
        let msg = format!("{:?}", e);
        if !msg.contains("BucketAlreadyOwnedByYou") && !msg.contains("BucketAlreadyExists") {
            panic!("Failed to create bucket: {}", msg);
        }
    }

    // Create SQS queue
    let queue_resp = sqs
        .create_queue()
        .queue_name(&queue_name)
        .send()
        .await
        .unwrap();
    let queue_url = queue_resp.queue_url().unwrap().to_string();

    // Create sessions table + fake session
    let _ = db
        .create_table()
        .table_name("test_sessions")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("session_token")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("session_token")
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

    let token = format!("tok-{}", uuid::Uuid::new_v4());
    let mut session = std::collections::HashMap::new();
    session.insert(
        "session_token".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S(token.clone()),
    );
    session.insert(
        "user_id".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S("test-user".to_string()),
    );
    shared::dynamo::put_item(&db, &shared::tables::table("sessions"), session)
        .await
        .unwrap();

    let state = AppState {
        db,
        s3,
        sqs,
        bucket,
        queue_url,
    };

    let app = Router::new()
        .route("/health", get(shared::health::health_check))
        .route("/uploads/initiate", post(handlers::initiate))
        .route("/uploads/complete", post(handlers::complete))
        .with_state(state);

    (app, token)
}

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn initiate_returns_presigned_urls() {
    let (app, token) = setup().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": "vid-123",
                        "filename": "test.mp4",
                        "content_type": "video/mp4",
                        "part_count": 3
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(!body["upload_id"].as_str().unwrap().is_empty());
    let urls = body["presigned_urls"].as_array().unwrap();
    assert_eq!(urls.len(), 3);
    // Each URL should be a valid presigned URL
    for url in urls {
        assert!(url.as_str().unwrap().contains("X-Amz-Signature"));
    }
}

#[tokio::test]
async fn initiate_requires_auth() {
    let (app, _) = setup().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "video_id": "vid-123",
                        "filename": "test.mp4",
                        "content_type": "video/mp4",
                        "part_count": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn full_upload_flow() {
    let (app, token) = setup().await;
    let video_id = format!("vid-{}", uuid::Uuid::new_v4());

    // 1. Initiate
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": video_id,
                        "filename": "video.mp4",
                        "content_type": "video/mp4",
                        "part_count": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let upload_id = body["upload_id"].as_str().unwrap().to_string();
    let s3_key = body["s3_key"].as_str().unwrap().to_string();
    let presigned_url = body["presigned_urls"][0].as_str().unwrap();

    // 2. Upload a part using the presigned URL
    let client = reqwest::Client::new();
    let upload_resp = client
        .put(presigned_url)
        .body(b"fake video content".to_vec())
        .send()
        .await
        .unwrap();

    assert!(upload_resp.status().is_success());
    let etag = upload_resp
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // 3. Complete
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/complete")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": video_id,
                        "upload_id": upload_id,
                        "s3_key": s3_key,
                        "parts": [{"part_number": 1, "etag": etag}]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(body["s3_key"].as_str().unwrap().contains(&video_id));
}

#[tokio::test]
async fn initiate_rejects_empty_video_id() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": "",
                        "filename": "test.mp4",
                        "content_type": "video/mp4",
                        "part_count": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn initiate_rejects_zero_parts() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": "vid-1",
                        "filename": "test.mp4",
                        "content_type": "video/mp4",
                        "part_count": 0
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn initiate_rejects_non_video_content_type() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/initiate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": "vid-1",
                        "filename": "test.exe",
                        "content_type": "application/octet-stream",
                        "part_count": 1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn complete_rejects_empty_parts() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/uploads/complete")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "video_id": "vid-1",
                        "upload_id": "uid-1",
                        "s3_key": "raw/vid-1/test.mp4",
                        "parts": []
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
