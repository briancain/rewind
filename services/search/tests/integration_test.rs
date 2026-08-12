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

async fn index_doc(app: &Router, doc: Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/index")
                .header("content-type", "application/json")
                .body(Body::from(doc.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn refresh_index() {
    let http = reqwest::Client::new();
    let url = format!(
        "{}/videos/_refresh",
        std::env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| "http://localhost:9200".into())
    );
    http.post(&url).send().await.unwrap();
}

/// The clickable-hashtag path: `?tag=` filters on the exact tag (case-insensitively), excluding
/// videos that merely mention the term in their title/description, and returns them newest-first.
#[tokio::test]
async fn tag_search_is_exact_case_insensitive_and_newest_first() {
    let app = setup().await;

    // Tagged "cats" (lowercase), oldest.
    index_doc(
        &app,
        json!({
            "video_id": "cat-old",
            "title": "Kittens playing",
            "description": "fluffy",
            "tags": ["cats"],
            "channel_id": "user-1",
            "created_at": "2026-01-01T00:00:00Z"
        }),
    )
    .await;

    // Tagged "Cats" (mixed case) — must still match; newest of the two tagged videos.
    index_doc(
        &app,
        json!({
            "video_id": "cat-new",
            "title": "More cute animals",
            "description": "adorable",
            "tags": ["Cats", "cute"],
            "channel_id": "user-2",
            "created_at": "2026-03-01T00:00:00Z"
        }),
    )
    .await;

    // NOT tagged cats — only mentions "cats" in the title. Must be excluded from a tag filter
    // (this is the key difference from the free-text `?q=` path).
    index_doc(
        &app,
        json!({
            "video_id": "dog-mention",
            "title": "Why cats are better than dogs",
            "description": "a debate",
            "tags": ["dogs"],
            "channel_id": "user-3",
            "created_at": "2026-02-01T00:00:00Z"
        }),
    )
    .await;

    refresh_index().await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?tag=cats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;

    // Exactly the two tagged videos — the title-mention is excluded, and mixed case is grouped.
    assert_eq!(body["total"], 2, "expected exactly the two tagged videos");
    let ids: Vec<&str> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["video_id"].as_str().unwrap())
        .collect();
    // Newest-first ordering: cat-new (Mar) before cat-old (Jan).
    assert_eq!(ids, vec!["cat-new", "cat-old"]);
    assert!(!ids.contains(&"dog-mention"));
}

/// A tag with no videos returns an empty result (not a 4xx/5xx).
#[tokio::test]
async fn tag_search_no_match_is_empty() {
    let app = setup().await;

    index_doc(
        &app,
        json!({
            "video_id": "v1",
            "title": "Something",
            "tags": ["rust"],
            "channel_id": "user-1",
            "created_at": "2026-01-01T00:00:00Z"
        }),
    )
    .await;
    refresh_index().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/search?tag=nonexistent")
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
