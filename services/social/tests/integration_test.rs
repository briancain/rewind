use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, KeySchemaElement, KeyType, ProvisionedThroughput, ScalarAttributeType,
};

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn setup() -> (Router, String) {
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("DISABLE_SES", "1");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let config = shared::config::ServiceConfig::from_env("social");
    let db = shared::dynamo::create_client(&config).await;

    // Create tables (delete first to ensure clean state)
    for table in [
        "test_reactions",
        "test_comments",
        "test_video_stats",
        "test_sessions",
        "test_comment_reactions",
        "test_view_history",
    ] {
        let _ = db.delete_table().table_name(table).send().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // reactions: PK=video_id, SK=user_id
    let _ = db
        .create_table()
        .table_name("test_reactions")
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("user_id")
                .key_type(KeyType::Range)
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
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("user_id")
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

    // comments: PK=video_id, SK=comment_id
    let _ = db
        .create_table()
        .table_name("test_comments")
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("comment_id")
                .key_type(KeyType::Range)
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
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("comment_id")
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

    // video_stats: PK=video_id
    let _ = db
        .create_table()
        .table_name("test_video_stats")
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

    // sessions table for auth
    let _ = db
        .create_table()
        .table_name("test_sessions")
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("session_token")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("session_token")
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

    // Insert a test session
    let token = "test-session-token";
    let mut session = std::collections::HashMap::new();
    session.insert(
        "session_token".into(),
        aws_sdk_dynamodb::types::AttributeValue::S(token.into()),
    );
    session.insert(
        "user_id".into(),
        aws_sdk_dynamodb::types::AttributeValue::S("user-1".into()),
    );
    db.put_item()
        .table_name("test_sessions")
        .set_item(Some(session))
        .send()
        .await
        .unwrap();

    // Insert a second user session for ownership tests
    let token2 = "test-session-token-2";
    let mut session2 = std::collections::HashMap::new();
    session2.insert(
        "session_token".into(),
        aws_sdk_dynamodb::types::AttributeValue::S(token2.into()),
    );
    session2.insert(
        "user_id".into(),
        aws_sdk_dynamodb::types::AttributeValue::S("user-2".into()),
    );
    db.put_item()
        .table_name("test_sessions")
        .set_item(Some(session2))
        .send()
        .await
        .unwrap();

    // comment_reactions: PK=video_id, SK="{comment_id}#{user_id}"
    let _ = db
        .create_table()
        .table_name("test_comment_reactions")
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("video_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
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
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
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

    // view_history: PK=user_id, SK=watched_at
    let _ = db
        .create_table()
        .table_name("test_view_history")
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("user_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("watched_at")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("user_id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("watched_at")
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

    let state = social::state::AppState { db };
    let app = Router::new()
        .route(
            "/videos/{id}/like",
            axum::routing::post(social::handlers::like),
        )
        .route(
            "/videos/{id}/dislike",
            axum::routing::post(social::handlers::dislike),
        )
        .route(
            "/videos/{id}/comments",
            axum::routing::post(social::handlers::add_comment).get(social::handlers::list_comments),
        )
        .route(
            "/videos/{id}/view",
            axum::routing::post(social::handlers::record_view),
        )
        .route(
            "/videos/{id}/stats",
            axum::routing::get(social::handlers::get_stats),
        )
        .route(
            "/videos/{vid}/comments/{cid}/like",
            axum::routing::post(social::handlers::like_comment),
        )
        .route(
            "/videos/{vid}/comments/{cid}/dislike",
            axum::routing::post(social::handlers::dislike_comment),
        )
        .route(
            "/videos/{vid}/comments/{cid}",
            axum::routing::delete(social::handlers::delete_comment),
        )
        .route(
            "/videos/{id}/history",
            axum::routing::post(social::handlers::record_history),
        )
        .route(
            "/history",
            axum::routing::get(social::handlers::list_history)
                .delete(social::handlers::delete_history_entry),
        )
        .with_state(state);

    (app, token.to_string())
}

#[tokio::test]
async fn like_toggle_and_stats() {
    let (app, token) = setup().await;

    // Like a video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-1/like")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["action"], "added");

    // Check stats
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/vid-1/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats = body_json(resp).await;
    assert_eq!(stats["likes"], 1);
    assert_eq!(stats["dislikes"], 0);

    // Like again = unlike
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-1/like")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["action"], "removed");

    // Stats should be back to 0
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/vid-1/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats = body_json(resp).await;
    assert_eq!(stats["likes"], 0);
}

#[tokio::test]
async fn comments_and_view_count() {
    let (app, token) = setup().await;

    // Add a comment
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-2/comments")
                .header("authorization", format!("Bearer {}", token))
                .header("content-type", "application/json")
                .body(Body::from(json!({"text": "great video!"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let comment = body_json(resp).await;
    assert_eq!(comment["text"], "great video!");
    assert_eq!(comment["user_id"], "user-1");

    // List comments
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/vid-2/comments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["comments"].as_array().unwrap().len(), 1);

    // Record views
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-2/view")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-2/view")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Check stats
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/vid-2/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats = body_json(resp).await;
    assert_eq!(stats["views"], 2);
    assert_eq!(stats["comment_count"], 1);
}

#[tokio::test]
async fn like_requires_auth() {
    let (app, _) = setup().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-1/like")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn switch_like_to_dislike() {
    let (app, token) = setup().await;

    // Like
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-3/like")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["action"], "added");

    // Dislike (should switch)
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-3/dislike")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["action"], "added");

    // Stats: like=0, dislike=1
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/videos/vid-3/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats = body_json(resp).await;
    assert_eq!(stats["likes"], 0);
    assert_eq!(stats["dislikes"], 1);
}

#[tokio::test]
async fn comment_like_toggle() {
    let (app, token) = setup().await;

    // Add a comment
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-cl/comments")
                .header("authorization", format!("Bearer {}", token))
                .header("content-type", "application/json")
                .body(Body::from(json!({"text": "test"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let comment = body_json(resp).await;
    let cid = comment["comment_id"].as_str().unwrap();

    // Like it
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/videos/vid-cl/comments/{}/like", cid))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["action"], "added");

    // Like again = toggle off
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/videos/vid-cl/comments/{}/like", cid))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["action"], "removed");

    // Like then switch to dislike
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/videos/vid-cl/comments/{}/like", cid))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/videos/vid-cl/comments/{}/dislike", cid))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body_json(resp).await["action"], "switched");
}

#[tokio::test]
async fn delete_own_comment() {
    let (app, token) = setup().await;

    // Add a comment as user-1
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-dc/comments")
                .header("authorization", format!("Bearer {}", token))
                .header("content-type", "application/json")
                .body(Body::from(json!({"text": "my comment"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let comment = body_json(resp).await;
    let cid = comment["comment_id"].as_str().unwrap();

    // Delete it as user-1 (owner)
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/videos/vid-dc/comments/{}", cid))
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
                .uri("/videos/vid-dc/comments")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["comments"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn cannot_delete_others_comment() {
    let (app, token) = setup().await;

    // Add a comment as user-1
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-dc2/comments")
                .header("authorization", format!("Bearer {}", token))
                .header("content-type", "application/json")
                .body(Body::from(json!({"text": "user1 comment"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let comment = body_json(resp).await;
    let cid = comment["comment_id"].as_str().unwrap();

    // Try to delete as user-2
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/videos/vid-dc2/comments/{}", cid))
                .header("authorization", "Bearer test-session-token-2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // A non-owner deleting a comment is a 403 (authorization failure), not a 500. Previously the
    // repo returned an untyped String error that the handler flattened to 500; with typed AppError
    // it now maps to the correct client-error status.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn watch_history_record_list_and_delete() {
    let (app, token) = setup().await;

    // Record two history entries
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-h1/history")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/videos/vid-h2/history")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // List history — should be newest first
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/history")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["video_id"], "vid-h2"); // newest first
    assert_eq!(entries[1]["video_id"], "vid-h1");

    // Delete the first entry
    let watched_at = entries[0]["watched_at"].as_str().unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/history?watched_at={}",
                    urlencoding::encode(watched_at)
                ))
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify only one entry remains
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/history")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["video_id"], "vid-h1");
}

#[tokio::test]
async fn history_requires_auth() {
    let (app, _) = setup().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
