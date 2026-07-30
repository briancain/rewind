//! Emits the `Rewind/Canary` custom metric (per-step success + latency, plus an overall
//! `CanarySuccess`) via `cloudwatch:PutMetricData`. Cloud-only: when `emit_metrics` is false (local
//! dev) the sink is a no-op so the canary never touches CloudWatch locally.
//!
//! The overall `CanarySuccess` metric (1 = all steps passed, 0 = any failed) is what the failure
//! alarm watches (`CanarySuccess < 1` → existing SNS topic).

use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
use aws_sdk_cloudwatch::Client as CwClient;

use crate::report::RunReport;

const NAMESPACE: &str = "Rewind/Canary";

/// A metrics destination. `Disabled` is used locally; `Cloudwatch` in the cluster.
pub enum MetricsSink {
    Disabled,
    Cloudwatch { client: CwClient, region: String },
}

impl MetricsSink {
    /// Build the sink. When `emit` is false, returns `Disabled` without constructing an AWS client.
    pub async fn new(emit: bool, region: &str) -> Self {
        if !emit {
            return MetricsSink::Disabled;
        }
        let shared = shared::aws::base_config().await;
        MetricsSink::Cloudwatch {
            client: CwClient::new(&shared),
            region: region.to_string(),
        }
    }

    /// Publish all metrics for a completed run. Best-effort: a CloudWatch error is logged but never
    /// fails the canary (the run's own pass/fail is the source of truth).
    pub async fn emit(&self, report: &RunReport) {
        let (client, region) = match self {
            MetricsSink::Disabled => {
                tracing::debug!("metrics disabled; skipping PutMetricData");
                return;
            }
            MetricsSink::Cloudwatch { client, region } => (client, region.as_str()),
        };

        let tier_dim = Dimension::builder()
            .name("Tier")
            .value(&report.tier)
            .build();
        let region_dim = Dimension::builder().name("Region").value(region).build();

        let mut data: Vec<MetricDatum> = Vec::new();
        for step in &report.steps {
            let step_dim = Dimension::builder().name("Step").value(&step.name).build();
            data.push(
                MetricDatum::builder()
                    .metric_name("StepSuccess")
                    .set_dimensions(Some(vec![
                        tier_dim.clone(),
                        region_dim.clone(),
                        step_dim.clone(),
                    ]))
                    .value(if step.ok { 1.0 } else { 0.0 })
                    .unit(StandardUnit::Count)
                    .build(),
            );
            data.push(
                MetricDatum::builder()
                    .metric_name("StepLatency")
                    .set_dimensions(Some(vec![tier_dim.clone(), region_dim.clone(), step_dim]))
                    .value(step.latency_ms as f64)
                    .unit(StandardUnit::Milliseconds)
                    .build(),
            );
        }

        // The overall success metric the alarm watches.
        data.push(
            MetricDatum::builder()
                .metric_name("CanarySuccess")
                .set_dimensions(Some(vec![tier_dim, region_dim]))
                .value(if report.passed() { 1.0 } else { 0.0 })
                .unit(StandardUnit::Count)
                .build(),
        );

        // PutMetricData accepts at most 1000 datums per call; we are far below that.
        match client
            .put_metric_data()
            .namespace(NAMESPACE)
            .set_metric_data(Some(data))
            .send()
            .await
        {
            Ok(_) => tracing::info!("emitted canary metrics to {NAMESPACE}"),
            Err(e) => tracing::error!(error = %e, "failed to put canary metrics"),
        }
    }
}
