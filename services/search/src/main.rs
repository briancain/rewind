use axum::{
    routing::{get, post},
    Router,
};
use search::{backfill, handlers, state::AppState};
use shared::{config::ServiceConfig, health::health_check};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("search");
    shared::tracing_setup::init(&config.service_name);

    let opensearch_url =
        std::env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| "http://localhost:9200".into());

    // One-off reindex mode: `service reindex`. Run as an in-cluster Kubernetes Job (NOT a public
    // endpoint) using the search ServiceAccount's IRSA role for DynamoDB scan + OpenSearch access.
    // See scripts/reindex.sh.
    if std::env::args().nth(1).as_deref() == Some("reindex") {
        run_reindex(&config, &opensearch_url).await;
        return;
    }

    let client = match search::signing::SearchClient::new(&opensearch_url).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to build OpenSearch client");
            std::process::exit(1);
        }
    };

    // Stream consumer is enabled only when a queue is configured (cloud). Local/HTTP-only when not.
    let stream_queue_url = std::env::var("STREAM_QUEUE_URL").ok();
    let sqs = if stream_queue_url.is_some() {
        Some(shared::aws::sqs_client().await)
    } else {
        None
    };

    let state = AppState {
        client: client.clone(),
        sqs: sqs.clone(),
        stream_queue_url: stream_queue_url.clone(),
    };

    // /search and /health are the only routes exposed in cloud. /index is the local-dev indexing
    // shim (used by transcode locally) and is registered ONLY when the stream consumer is not
    // active — so in cloud the route does not exist at all and cannot be exposed via the ingress.
    let mut app = Router::new()
        .route("/health", get(health_check))
        .route("/search", get(handlers::search));
    if stream_queue_url.is_none() {
        app = app.route("/index", post(handlers::index_video));
    }
    let app = app.with_state(state).layer(shared::cors::permissive());

    let app = shared::middleware::with_logging(app);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    // Run the HTTP server, and (when configured) the stream consumer, concurrently.
    match (sqs, stream_queue_url) {
        (Some(sqs), Some(queue_url)) => {
            tracing::info!("search starting with stream consumer enabled");
            tokio::select! {
                r = axum::serve(listener, app) => { r.unwrap(); }
                _ = search::consumer::run(sqs, queue_url, client) => {}
            }
        }
        _ => {
            tracing::info!("search starting in HTTP-only mode (no STREAM_QUEUE_URL)");
            axum::serve(listener, app).await.unwrap();
        }
    }
}

/// One-off backfill: scan the videos table and reconcile OpenSearch, then exit. Invoked as a
/// Kubernetes Job (`service reindex`) under the search ServiceAccount's IRSA role. Exits non-zero
/// on failure so the Job is marked failed.
async fn run_reindex(config: &ServiceConfig, opensearch_url: &str) {
    tracing::info!("running reindex backfill");
    let client = match search::signing::SearchClient::new(opensearch_url).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to build OpenSearch client");
            std::process::exit(1);
        }
    };
    let db = shared::dynamo::create_client(config).await;
    match backfill::reindex(&db, &client).await {
        Ok(report) => tracing::info!(?report, "reindex complete"),
        Err(e) => {
            tracing::error!(error = %e, "reindex failed");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_returns_200() {
        let app = Router::new().route("/health", get(health_check));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
