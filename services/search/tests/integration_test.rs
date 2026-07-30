use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn setup() -> Router {
    let opensearch_url =
        std::env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| "http://localhost:9200".into());

    // Delete the index if it exists (clean state)
    let http = reqwest::Client::new();
    let _ = http
        .delete(format!("{}/videos", opensearch_url))
        .send()
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let client = search::signing::SearchClient::new(&opensearch_url)
        .await
        .unwrap();

    // The /index and /search routes under test use only the OpenSearch client; the consumer is
    // left disabled. (Reindex is exercised directly via backfill::reindex in sync_test.rs.)
    let state = search::state::AppState {
        client,
        sqs: None,
        stream_queue_url: None,
    };

    Router::new()
        .route("/index", post(search::handlers::index_video))
        .route("/search", get(search::handlers::search))
        .with_state(state)
}

#[tokio::test]
async fn index_and_search() {
    let app = setup().await;

    // Index a video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "video_id": "vid-1",
                        "title": "Rust Programming Tutorial",
                        "description": "Learn Rust from scratch",
                        "tags": ["rust", "programming", "tutorial"],
                        "channel_id": "user-1"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Index another video
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "video_id": "vid-2",
                        "title": "Cooking with Gordon",
                        "description": "Amazing pasta recipe",
                        "tags": ["cooking", "pasta"],
                        "channel_id": "user-2"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // OpenSearch needs a refresh to make docs searchable
    let state_http = reqwest::Client::new();
    let refresh_url = format!(
        "{}/videos/_refresh",
        std::env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| "http://localhost:9200".into())
    );
    state_http.post(&refresh_url).send().await.unwrap();

    // Search for "rust"
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=rust")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["results"][0]["video_id"], "vid-1");

    // Search for "cooking"
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=cooking")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["results"][0]["video_id"], "vid-2");
}

#[tokio::test]
async fn search_no_results() {
    let app = setup().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/search?q=nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
}
