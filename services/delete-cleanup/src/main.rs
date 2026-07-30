use axum::{routing::get, Router};
use delete_cleanup::state::AppState;
use shared::{config::ServiceConfig, health::health_check};

#[tokio::main]
async fn main() {
    let config = ServiceConfig::from_env("delete-cleanup");
    shared::tracing_setup::init(&config.service_name);

    // One-off reconcile mode: `delete-cleanup reconcile`. Run as a per-region Kubernetes CronJob (NOT
    // the long-running consumer) under a scoped read-only IRSA role — scans the videos Global Table
    // for `deleted` tombstones whose dependent data was never reclaimed and emits the
    // Rewind/Deletion UnreclaimedDeletions metric/alarm. Detect + alarm only (no re-cleanup). The
    // cleanup-side analogue of `transcode reconcile`.
    if std::env::args().nth(1).as_deref() == Some("reconcile") {
        run_reconcile(&config).await;
        return;
    }

    let db = shared::dynamo::create_client(&config).await;
    let s3 = shared::aws::s3_client().await;

    let video_bucket = std::env::var("VIDEO_BUCKET").unwrap_or_else(|_| "rewind-videos".into());
    let raw_bucket = std::env::var("RAW_BUCKET").unwrap_or_else(|_| "rewind-raw".into());

    // The consumer runs only when a queue is configured (cloud). Without it (local/dev) the pod is
    // health-only — there is no Pipe feeding a queue locally, so there is nothing to drain.
    match std::env::var("CLEANUP_QUEUE_URL").ok() {
        Some(queue_url) => {
            let sqs = shared::aws::sqs_client().await;

            // Edge invalidation is gated on CDN_DISTRIBUTION_ID: present in cloud (set by deploy.sh
            // from the `cdn` stack output), absent locally / before the cdn stack exists — in which
            // case the CloudFront client is not built and invalidation is skipped.
            let cdn_distribution_id = std::env::var("CDN_DISTRIBUTION_ID")
                .ok()
                .filter(|s| !s.trim().is_empty());
            let cloudfront = match &cdn_distribution_id {
                Some(_) => {
                    let aws_config = shared::aws::base_config().await;
                    Some(aws_sdk_cloudfront::Client::new(&aws_config))
                }
                None => {
                    tracing::info!("no CDN_DISTRIBUTION_ID set; CDN invalidation disabled");
                    None
                }
            };

            let state = AppState {
                db,
                s3,
                sqs,
                queue_url,
                video_bucket,
                raw_bucket,
                cloudfront,
                cdn_distribution_id,
            };
            tokio::spawn(async move { delete_cleanup::consumer::run(state).await });
            tracing::info!("delete-cleanup consumer enabled");
        }
        None => {
            tracing::info!("no CLEANUP_QUEUE_URL set; running health-only (consumer disabled)");
        }
    }

    // Health server for k8s liveness/readiness probes.
    let app = Router::new().route("/health", get(health_check));
    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("health server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// One-off reconcile sweep entrypoint (`delete-cleanup reconcile`). Builds the DynamoDB + S3 +
/// CloudWatch clients, resolves the region (for the metric dimension) and the grace threshold from
/// env, runs a single sweep, then exits — non-zero on a scan failure so the Job is marked failed.
/// Does NOT require `CLEANUP_QUEUE_URL` (it never touches the queue). Mirrors
/// `transcode::main::run_reconcile`.
async fn run_reconcile(config: &ServiceConfig) {
    let aws_config = shared::aws::base_config().await;
    let region = aws_config
        .region()
        .map(|r| r.as_ref().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let db = shared::dynamo::create_client(config).await;
    let s3 = shared::aws::s3_client().await;
    let cw = aws_sdk_cloudwatch::Client::new(&aws_config);

    let video_bucket = std::env::var("VIDEO_BUCKET").unwrap_or_else(|_| "rewind-videos".into());
    let raw_bucket = std::env::var("RAW_BUCKET").unwrap_or_else(|_| "rewind-raw".into());
    let threshold = delete_cleanup::reconcile::threshold_from_env();

    tracing::info!(
        region = %region,
        threshold_mins = threshold.num_minutes(),
        "starting delete-cleanup reconcile sweep"
    );

    match delete_cleanup::reconcile::run_sweep(
        &db,
        &s3,
        Some(&cw),
        &region,
        &video_bucket,
        &raw_bucket,
        threshold,
    )
    .await
    {
        Ok(orphans) => tracing::info!(orphans, "reconcile sweep complete"),
        Err(e) => {
            tracing::error!(error = %e, "reconcile sweep failed");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

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
