use axum::{routing::get, Router};
use shared::{config::ServiceConfig, health::health_check};
use streaming::{handlers, state::AppState};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("streaming");
    shared::tracing_setup::init(&config.service_name);

    let db = shared::dynamo::create_client(&config).await;

    let s3 = shared::aws::s3_client().await;

    let bucket = std::env::var("VIDEO_BUCKET").unwrap_or_else(|_| "rewind-videos".into());

    let state = AppState { db, s3, bucket };

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/videos/{id}/stream-url", get(handlers::stream_url))
        .route("/videos/{id}/thumbnail-url", get(handlers::thumbnail_url))
        .with_state(state)
        .layer(shared::cors::permissive());

    let app = shared::middleware::with_logging(app);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
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
