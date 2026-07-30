use axum::{
    routing::{get, post},
    Router,
};
use shared::{config::ServiceConfig, health::health_check};
use social::{handlers, state::AppState};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("social");
    shared::tracing_setup::init(&config.service_name);

    let db = shared::dynamo::create_client(&config).await;
    let state = AppState { db };

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/videos/{id}/like", post(handlers::like))
        .route("/videos/{id}/dislike", post(handlers::dislike))
        .route(
            "/videos/{id}/comments",
            post(handlers::add_comment).get(handlers::list_comments),
        )
        .route("/videos/{id}/view", post(handlers::record_view))
        .route("/videos/{id}/stats", get(handlers::get_stats))
        .route(
            "/videos/{vid}/comments/{cid}/like",
            post(handlers::like_comment),
        )
        .route(
            "/videos/{vid}/comments/{cid}/dislike",
            post(handlers::dislike_comment),
        )
        .route(
            "/videos/{vid}/comments/{cid}",
            axum::routing::delete(handlers::delete_comment),
        )
        .route("/videos/{id}/history", post(handlers::record_history))
        .route(
            "/history",
            get(handlers::list_history).delete(handlers::delete_history_entry),
        )
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
