//! Construction of AWS SDK clients that honor LocalStack-style endpoint overrides for local dev.
//!
//! These centralize the "build a client, branch on a `*_ENDPOINT` env var" block that was
//! previously copy-pasted across the upload, transcode, and search services.
//! In the cloud the env vars are unset, so the default regional endpoints are used; locally
//! `SQS_ENDPOINT` / `S3_ENDPOINT` point the clients at LocalStack.

use aws_config::SdkConfig;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;
use aws_smithy_types::timeout::TimeoutConfig;
use std::time::Duration;

/// Default timeouts applied to every AWS SDK client built from [`base_config`].
///
/// The AWS Rust SDK already enables standard retries (3 attempts) by default, but it does **not**
/// set an operation (read) timeout — so a connection that opens and then stalls has no SDK-level
/// bound and can hang a request-serving handler or an SQS worker indefinitely. These add a
/// per-attempt bound (each try, retried by the SDK), an overall bound (the whole retry sequence),
/// and a connect bound. Values are deliberately generous: in practice our calls
/// (DynamoDB/S3/SQS/SES/MediaConvert/CloudWatch control-plane ops) are sub-second, so a 10s attempt
/// timeout only ever trips on a genuine hang — it won't clip healthy-but-slow calls.
pub fn default_timeout_config() -> TimeoutConfig {
    TimeoutConfig::builder()
        .connect_timeout(Duration::from_secs(5))
        .operation_attempt_timeout(Duration::from_secs(10))
        .operation_timeout(Duration::from_secs(30))
        .build()
}

/// Load the shared AWS SDK config with [`default_timeout_config`] applied. Every SDK client
/// constructor builds from this so timeouts (and the SDK's default retries) are consistent across
/// all services.
pub async fn base_config() -> SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .timeout_config(default_timeout_config())
        .load()
        .await
}

/// Timeouts for the SQS client specifically. Our SQS consumers receive with **20s long-polling**
/// (`wait_time_seconds(20)`), which is a single intentionally-long operation — so the per-attempt
/// timeout MUST exceed the long-poll wait, otherwise every idle poll is cut short and retried
/// (which is exactly the regression this fixes). Short ops on this client (delete/send) are
/// unaffected by the looser bound. The other clients keep the tight [`default_timeout_config`].
pub fn sqs_timeout_config() -> TimeoutConfig {
    TimeoutConfig::builder()
        .connect_timeout(Duration::from_secs(5))
        .operation_attempt_timeout(Duration::from_secs(30))
        .operation_timeout(Duration::from_secs(60))
        .build()
}

/// The endpoint override for a client, if `var` is set to a non-empty value. An empty string is
/// treated as unset (so an accidentally-blank env var doesn't produce a broken endpoint URL).
fn endpoint_override(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.trim().is_empty())
}

/// Build an SQS client, honoring `SQS_ENDPOINT` (LocalStack) when set. Uses [`sqs_timeout_config`]
/// (long-poll-safe) rather than the default tight timeouts, since the consumers long-poll for 20s.
pub async fn sqs_client() -> SqsClient {
    let shared = base_config().await;
    let mut builder =
        aws_sdk_sqs::config::Builder::from(&shared).timeout_config(sqs_timeout_config());
    if let Some(endpoint) = endpoint_override("SQS_ENDPOINT") {
        builder = builder.endpoint_url(endpoint);
    }
    SqsClient::from_conf(builder.build())
}

/// Build an S3 client, honoring `S3_ENDPOINT` (LocalStack) when set. The override also forces
/// path-style addressing, which LocalStack requires (virtual-hosted bucket subdomains don't
/// resolve against `localhost`).
pub async fn s3_client() -> S3Client {
    let shared = base_config().await;
    let mut builder = aws_sdk_s3::config::Builder::from(&shared);
    if let Some(endpoint) = endpoint_override("S3_ENDPOINT") {
        builder = builder.endpoint_url(endpoint).force_path_style(true);
    }
    S3Client::from_conf(builder.build())
}

#[cfg(test)]
mod tests {
    use super::{default_timeout_config, endpoint_override};
    use std::time::Duration;

    #[test]
    fn default_timeout_config_bounds_attempt_overall_and_connect() {
        let t = default_timeout_config();
        assert_eq!(t.connect_timeout(), Some(Duration::from_secs(5)));
        assert_eq!(t.operation_attempt_timeout(), Some(Duration::from_secs(10)));
        assert_eq!(t.operation_timeout(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn sqs_timeout_config_exceeds_the_20s_long_poll() {
        // The consumers receive with wait_time_seconds(20); the per-attempt timeout MUST be larger
        // or every idle long-poll is cut short and retried.
        let t = super::sqs_timeout_config();
        let attempt = t.operation_attempt_timeout().unwrap();
        assert!(
            attempt > Duration::from_secs(20),
            "SQS attempt timeout {attempt:?} must exceed the 20s long-poll wait"
        );
        assert!(t.operation_timeout().unwrap() >= attempt);
    }

    #[test]
    fn endpoint_override_returns_value_when_set() {
        let var = "REWIND_TEST_ENDPOINT_SET";
        std::env::set_var(var, "http://localhost:4566");
        assert_eq!(endpoint_override(var), Some("http://localhost:4566".into()));
        std::env::remove_var(var);
    }

    #[test]
    fn endpoint_override_none_when_unset() {
        let var = "REWIND_TEST_ENDPOINT_UNSET";
        std::env::remove_var(var);
        assert_eq!(endpoint_override(var), None);
    }

    #[test]
    fn endpoint_override_treats_blank_as_unset() {
        let var = "REWIND_TEST_ENDPOINT_BLANK";
        std::env::set_var(var, "   ");
        assert_eq!(endpoint_override(var), None);
        std::env::remove_var(var);
    }
}
