use axum::{
    routing::{get, post},
    Router,
};
use shared::config::ServiceConfig;
use upload::{handlers, state::AppState};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("upload");
    shared::tracing_setup::init(&config.service_name);

    let db = shared::dynamo::create_client(&config).await;
    let s3 = shared::aws::s3_client().await;
    let sqs = shared::aws::sqs_client().await;

    let bucket = std::env::var("S3_BUCKET").unwrap_or_else(|_| "rewind-raw".to_string());
    let queue_url = std::env::var("SQS_QUEUE_URL")
        .unwrap_or_else(|_| "http://localhost:4566/000000000000/transcode-jobs".to_string());

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
        .with_state(state)
        .layer(shared::cors::permissive());

    let app = shared::middleware::with_logging(app);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
