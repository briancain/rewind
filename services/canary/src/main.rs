//! Canary entrypoint. Dispatches on argv (`shallow` | `deep` | `setup`), mirroring how the search
//! service dispatches `reindex`. The container `CMD` is `service`, so the CronJob/Job passes the
//! subcommand as an arg (e.g. `["service", "deep"]`). Exits non-zero when the run fails, so a
//! Kubernetes Job/CronJob is marked failed and the CloudWatch failure alarm fires.

use canary::client::RewindClient;
use canary::config::CanaryConfig;
use canary::metrics::MetricsSink;
use canary::report::RunReport;
use canary::{deep, setup, shallow};

#[tokio::main]
async fn main() {
    shared::tracing_setup::init("canary");

    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "shallow".to_string());
    let cfg = CanaryConfig::from_env();
    tracing::info!(
        mode = %mode,
        cloud = cfg.domain.is_some(),
        verify_cascade = cfg.verify_cascade,
        "starting canary"
    );

    let client = match RewindClient::new(cfg.endpoints.clone()) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to build HTTP client");
            std::process::exit(2);
        }
    };

    match mode.as_str() {
        "shallow" => {
            let report = shallow::run(&client, &cfg).await;
            finish(report, &cfg).await;
        }
        "deep" => {
            let (db, s3) = aws_clients().await;
            let report = deep::run(&client, &db, &s3, &cfg).await;
            finish(report, &cfg).await;
        }
        "setup" => {
            let (db, _s3) = aws_clients().await;
            match setup::run(&client, &db, &cfg).await {
                Ok(()) => {
                    tracing::info!("setup succeeded");
                }
                Err(e) => {
                    tracing::error!(error = %e, "setup failed");
                    std::process::exit(1);
                }
            }
        }
        other => {
            tracing::error!(mode = %other, "unknown mode (expected: shallow | deep | setup)");
            std::process::exit(2);
        }
    }
}

/// Build the DynamoDB + S3 clients, honoring LocalStack/DynamoDB-Local endpoint overrides for local
/// dev (via the shared helpers).
async fn aws_clients() -> (aws_sdk_dynamodb::Client, aws_sdk_s3::Client) {
    let svc_config = shared::config::ServiceConfig::from_env("canary");
    let db = shared::dynamo::create_client(&svc_config).await;
    let s3 = shared::aws::s3_client().await;
    (db, s3)
}

/// Emit metrics, print the summary, and exit non-zero on failure.
async fn finish(report: RunReport, cfg: &CanaryConfig) {
    let sink = MetricsSink::new(cfg.emit_metrics, &cfg.region).await;
    sink.emit(&report).await;

    let summary = report.summary();
    if report.passed() {
        tracing::info!("\n{summary}");
    } else {
        tracing::error!("\n{summary}");
        std::process::exit(1);
    }
}
