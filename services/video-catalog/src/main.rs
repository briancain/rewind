use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};
use shared::config::ServiceConfig;
use video_catalog::{handlers, state::AppState};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("video-catalog");
    shared::tracing_setup::init(&config.service_name);

    let db = shared::dynamo::create_client(&config).await;
    let state = AppState { db };

    let app = Router::new()
        .route("/health", get(shared::health::health_check))
        .route("/videos", post(handlers::create_video))
        .route("/videos", get(handlers::list_videos))
        .route("/videos/feed", get(handlers::feed))
        .route("/videos/surf", get(handlers::surf))
        .route("/videos/{id}", get(handlers::get_video))
        .route("/videos/{id}", put(handlers::update_video))
        .route("/videos/{id}", delete(handlers::delete_video))
        .route("/videos/{id}/status", patch(handlers::update_status))
        .with_state(state)
        .layer(shared::cors::permissive());

    let app = shared::middleware::with_logging(app);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
