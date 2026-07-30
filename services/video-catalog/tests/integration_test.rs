use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{delete, get, patch, post, put},
    Router,
};
use serde_json::{json, Value};
use std::future::Future;
use tower::ServiceExt;
use video_catalog::{handlers, state::AppState};

async fn poll_until<F, Fut, T>(timeout: std::time::Duration, mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = std::time::Instant::now();
    loop {
        if let Some(result) = f().await {
            return Some(result);
        }
        if start.elapsed() > timeout {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn setup() -> (Router, String) {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let config = shared::config::ServiceConfig::from_env("video-catalog");
    let db = shared::dynamo::create_client(&config).await;

    // Delete videos table if it exists (ensures GSIs are created fresh)
    let _ = db.delete_table().table_name("test_videos").send().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

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
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("channel_id")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("status")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .global_secondary_indexes(
            aws_sdk_dynamodb::types::GlobalSecondaryIndex::builder()
                .index_name("channel-index")
                .key_schema(
                    aws_sdk_dynamodb::types::KeySchemaElement::builder()
                        .attribute_name("channel_id")
                        .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .projection(
                    aws_sdk_dynamodb::types::Projection::builder()
                        .projection_type(aws_sdk_dynamodb::types::ProjectionType::All)
                        .build(),
                )
                .provisioned_throughput(
                    aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                        .read_capacity_units(5)
                        .write_capacity_units(5)
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        )
        .global_secondary_indexes(
            aws_sdk_dynamodb::types::GlobalSecondaryIndex::builder()
                .index_name("status-index")
                .key_schema(
                    aws_sdk_dynamodb::types::KeySchemaElement::builder()
                        .attribute_name("status")
                        .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .projection(
                    aws_sdk_dynamodb::types::Projection::builder()
                        .projection_type(aws_sdk_dynamodb::types::ProjectionType::All)
                        .build(),
                )
                .provisioned_throughput(
                    aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                        .read_capacity_units(5)
                        .write_capacity_units(5)
                        .build()
                        .unwrap(),
                )
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

    // Wait for table and GSIs to become active
    loop {
        let desc = db
            .describe_table()
            .table_name("test_videos")
            .send()
            .await
            .unwrap();
        let table = desc.table().unwrap();
        let table_active =
            table.table_status() == Some(&aws_sdk_dynamodb::types::TableStatus::Active);
        let gsis_active = table
            .global_secondary_indexes()
            .iter()
            .all(|gsi| gsi.index_status() == Some(&aws_sdk_dynamodb::types::IndexStatus::Active));
        if table_active && gsis_active {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Create sessions table (for auth)
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

    // Create a fake session for testing
    let user_id = format!("user-{}", uuid::Uuid::new_v4());
    let token = format!("tok-{}", uuid::Uuid::new_v4());
    let mut session = std::collections::HashMap::new();
    session.insert(
        "session_token".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S(token.clone()),
    );
    session.insert(
        "user_id".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()),
    );
    shared::dynamo::put_item(&db, &shared::tables::table("sessions"), session)
        .await
        .unwrap();

    let state = AppState { db };
    let app = Router::new()
        .route("/videos", post(handlers::create_video))
        .route("/videos", get(handlers::list_videos))
        .route("/videos/feed", get(handlers::feed))
        .route("/videos/surf", get(handlers::surf))
        .route("/videos/{id}", get(handlers::get_video))
        .route("/videos/{id}", put(handlers::update_video))
        .route("/videos/{id}", delete(handlers::delete_video))
        .route("/videos/{id}/status", patch(handlers::update_status))
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
async fn full_crud_flow() {
    let (app, token) = setup().await;

    // Create video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "title": "My Video",
                        "description": "A test video",
                        "genre": "tech",
                        "tags": ["rust", "tutorial"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    let video_id = body["video_id"].as_str().unwrap().to_string();
    assert_eq!(body["title"], "My Video");
    assert_eq!(body["status"], "draft");

    // Get video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/videos/{}", video_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["title"], "My Video");

    // Update video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/videos/{}", video_id))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(json!({"title": "Updated Title"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Publish
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/videos/{}/status", video_id))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(json!({"status": "published"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Feed should contain the video (poll until GSI propagates)
    let feed_videos = poll_until(std::time::Duration::from_secs(5), || {
        let app = app.clone();
        let video_id = video_id.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/videos/feed")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if resp.status() != StatusCode::OK {
                return None;
            }
            let body = body_json(resp).await;
            let videos = body["videos"].as_array()?.clone();
            if videos.iter().any(|v| v["video_id"] == video_id) {
                Some(videos)
            } else {
                None
            }
        }
    })
    .await
    .expect("feed should contain the published video within timeout");
    assert!(!feed_videos.is_empty());
}

#[tokio::test]
async fn owner_enforcement() {
    let (app, _token) = setup().await;

    // Create a different user's session
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    let config = shared::config::ServiceConfig::from_env("video-catalog");
    let db = shared::dynamo::create_client(&config).await;

    let other_token = format!("tok-other-{}", uuid::Uuid::new_v4());
    let mut session = std::collections::HashMap::new();
    session.insert(
        "session_token".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S(other_token.clone()),
    );
    session.insert(
        "user_id".to_string(),
        aws_sdk_dynamodb::types::AttributeValue::S("other-user".to_string()),
    );
    shared::dynamo::put_item(&db, &shared::tables::table("sessions"), session)
        .await
        .unwrap();

    // Create video as first user
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", _token))
                .body(Body::from(
                    json!({"title": "Owner Test", "description": "x", "genre": "x", "tags": []})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    let video_id = body["video_id"].as_str().unwrap().to_string();

    // Try to update as other user — should fail
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/videos/{}", video_id))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", other_token))
                .body(Body::from(json!({"title": "Hacked"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn surf_returns_deterministic_results() {
    let (app, token) = setup().await;

    // Create and publish 3 videos
    for i in 0..3 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/videos")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::from(
                        json!({"title": format!("Surf {}", i), "description": "x", "genre": "x", "tags": []}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp).await;
        let vid = body["video_id"].as_str().unwrap().to_string();

        app.clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/videos/{}/status", vid))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", token))
                    .body(Body::from(json!({"status": "published"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Poll until surf returns a result (GSI propagation)
    poll_until(std::time::Duration::from_secs(5), || {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/videos/surf?seed=42&offset=0")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if resp.status() == StatusCode::OK {
                Some(())
            } else {
                None
            }
        }
    })
    .await
    .expect("surf should return a video within timeout");

    // Same seed + offset should return same video
    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/surf?seed=42&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body1 = body_json(resp1).await;

    let resp2 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/surf?seed=42&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body2 = body_json(resp2).await;

    assert_eq!(body1["video_id"], body2["video_id"]);
}

#[tokio::test]
async fn unauthenticated_create_fails() {
    let (app, _) = setup().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title": "No Auth", "description": "x", "genre": "x", "tags": []})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_video_owner_only() {
    let (app, token) = setup().await;

    // Create a video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    serde_json::json!({"title":"to delete","description":"x","genre":"test","tags":[]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    let video_id = body["video_id"].as_str().unwrap().to_string();

    // Unauthenticated delete fails
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/videos/{}", video_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Owner delete succeeds
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/videos/{}", video_id))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify it's gone
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/videos/{}", video_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_video_rejects_empty_title() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::from(
                    json!({
                        "title": "   ",
                        "description": "desc",
                        "genre": "test",
                        "tags": []
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
