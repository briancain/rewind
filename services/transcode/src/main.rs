use axum::{routing::get, Router};
use shared::config::ServiceConfig;
use transcode::state::AppState;

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("transcode");
    shared::tracing_setup::init(&config.service_name);

    // One-off reconcile mode: `transcode reconcile`. Run as a per-region Kubernetes CronJob (NOT a
    // long-running server) under a scoped IRSA role — scans the videos Global Table for stranded
    // `processing` jobs and emits the StuckTranscodes metric/alarm. Detect + alarm only (no
    // re-drive). See scripts/redrive-transcode.sh.
    if std::env::args().nth(1).as_deref() == Some("reconcile") {
        run_reconcile(&config).await;
        return;
    }

    let aws_config = shared::aws::base_config().await;

    let db = shared::dynamo::create_client(&config).await;
    let sqs = shared::aws::sqs_client().await;
    let s3 = shared::aws::s3_client().await;

    let mediaconvert = if std::env::var("DISABLE_MEDIACONVERT").is_ok() {
        None
    } else {
        Some(aws_sdk_mediaconvert::Client::new(&aws_config))
    };

    let queue_url = std::env::var("SQS_QUEUE_URL")
        .unwrap_or_else(|_| "http://localhost:4566/000000000000/transcode-jobs".to_string());
    let output_bucket =
        std::env::var("OUTPUT_BUCKET").unwrap_or_else(|_| "rewind-videos".to_string());
    let mediaconvert_role = std::env::var("MEDIACONVERT_ROLE")
        .unwrap_or_else(|_| "arn:aws:iam::000000000000:role/MediaConvertRole".to_string());

    let cdn_base_url =
        std::env::var("CDN_BASE_URL").unwrap_or_else(|_| "http://localhost:8083".to_string());

    let completion_queue_url = std::env::var("COMPLETION_QUEUE_URL").ok();

    let state = AppState {
        db,
        sqs,
        s3,
        mediaconvert,
        queue_url,
        output_bucket,
        mediaconvert_role,
        cdn_base_url,
        completion_queue_url,
    };

    // Health server (liveness). The SQS consumers run as background tasks.
    let health_app = Router::new().route("/health", get(shared::health::health_check));
    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("health server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    // Transcode-jobs consumer (always) + MediaConvert completion consumer (cloud only).
    let jobs_state = state.clone();
    tokio::spawn(async move { transcode::consumer::run(jobs_state).await });
    if state.completion_queue_url.is_some() {
        let completion_state = state.clone();
        tokio::spawn(async move { transcode::completion::run(completion_state).await });
    }

    axum::serve(listener, health_app).await.unwrap();
}

/// One-off reconcile sweep entrypoint (`transcode reconcile`). Builds the DynamoDB + CloudWatch
/// clients, resolves the region (for the metric dimension) and the stuck threshold from env, runs a
/// single sweep, then exits — non-zero on a scan failure so the Job is marked failed.
async fn run_reconcile(config: &ServiceConfig) {
    let aws_config = shared::aws::base_config().await;
    let region = aws_config
        .region()
        .map(|r| r.as_ref().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let db = shared::dynamo::create_client(config).await;
    let cw = aws_sdk_cloudwatch::Client::new(&aws_config);
    let threshold = transcode::reconcile::threshold_from_env();

    tracing::info!(
        region = %region,
        threshold_mins = threshold.num_minutes(),
        "starting transcode reconcile sweep"
    );

    match transcode::reconcile::run_sweep(&db, Some(&cw), &region, threshold).await {
        Ok(stuck) => tracing::info!(stuck, "reconcile sweep complete"),
        Err(e) => {
            tracing::error!(error = %e, "reconcile sweep failed");
            std::process::exit(1);
        }
    }
}
