//! OpenSearch client for the search service, built on the official `opensearch` crate.
//!
//! Auth is AWS SigV4 via the crate's `aws-auth` feature: we hand it our shared `SdkConfig`
//! (`shared::aws::base_config`), and the transport re-signs every request with the credential
//! provider (so IRSA credential refresh is handled for us) for service `es`. This replaces the
//! previous hand-rolled `reqwest` + `aws-sigv4` signer.
//!
//! Robustness (carried over from the hand-rolled client): a connect + overall request timeout on
//! the underlying HTTP client, and a bounded retry on transport errors. Every OpenSearch operation
//! the service issues is idempotent (search GET, `_doc/{video_id}` upsert/delete keyed by
//! `video_id`), so retrying a failed send is safe. Defaults are generous on purpose — the job is to
//! bound a pathological *hang* (which otherwise rides until the blackbox canary's 30s client
//! deadline), not to clip a healthy-but-slow query.

use std::future::Future;
use std::time::Duration;

use opensearch::auth::Credentials;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::OpenSearch;
use shared::error::AppError;
use url::Url;

/// Overall-request timeout for the OpenSearch HTTP client. Resolved from env so it can be tuned
/// in-cluster without a rebuild. This bounds the *whole* request (connect + read + body) — the
/// `opensearch` 2.x transport exposes only this single timeout, which is exactly what we need to
/// stop a stalled call from hanging until the blackbox canary's 30s deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSearchTimeouts {
    pub request: Duration,
}

impl OpenSearchTimeouts {
    pub fn from_env() -> Self {
        Self {
            request: Duration::from_millis(env_u64("OPENSEARCH_TIMEOUT_MS", 12_000)),
        }
    }
}

/// Bounded retry policy for transient (transport) failures talking to OpenSearch. Retries apply
/// only to transport errors (connect / read timeout / send) surfaced as `Err` from the transport;
/// an HTTP error *status* comes back as `Ok(Response)` and is left for the caller to interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_backoff: Duration,
}

impl RetryPolicy {
    pub fn from_env() -> Self {
        Self {
            max_retries: env_u64("OPENSEARCH_MAX_RETRIES", 1) as u32,
            base_backoff: Duration::from_millis(env_u64("OPENSEARCH_RETRY_BACKOFF_MS", 250)),
        }
    }

    /// Total attempts to make = 1 initial + `max_retries`.
    pub fn total_attempts(&self) -> u32 {
        self.max_retries + 1
    }

    /// Exponential backoff to wait after a given 0-based failed attempt before the next try:
    /// attempt 0 → base, attempt 1 → 2·base, attempt 2 → 4·base, … (saturating).
    pub fn backoff(&self, failed_attempt: u32) -> Duration {
        self.base_backoff
            .saturating_mul(2u32.saturating_pow(failed_attempt))
    }
}

/// Parse a `u64` from an env var, falling back to `default` when unset or unparseable.
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// The search service's OpenSearch client: the official `opensearch` client plus the bounded retry
/// policy. Cloneable so it can be shared by the HTTP handlers and the stream consumer.
#[derive(Clone)]
pub struct SearchClient {
    pub os: OpenSearch,
    retry: RetryPolicy,
}

impl SearchClient {
    /// Build the client for `opensearch_url`, wiring SigV4 auth (from the shared SDK config),
    /// connect + request timeouts, and the retry policy — all from the environment.
    pub async fn new(opensearch_url: &str) -> Result<Self, AppError> {
        let timeouts = OpenSearchTimeouts::from_env();

        let url = Url::parse(opensearch_url).map_err(|e| {
            AppError::Internal(format!(
                "invalid OPENSEARCH_ENDPOINT {opensearch_url:?}: {e}"
            ))
        })?;
        let conn_pool = SingleNodeConnectionPool::new(url);

        // SigV4 credentials from our shared SDK config (IRSA in-cluster, env locally). The transport
        // re-signs each request and refreshes credentials via the provider.
        let sdk_config = shared::aws::base_config().await;
        let credentials: Credentials = sdk_config
            .try_into()
            .map_err(|e| AppError::Internal(format!("opensearch aws-auth setup failed: {e}")))?;

        let transport = TransportBuilder::new(conn_pool)
            .auth(credentials)
            .service_name("es")
            .timeout(timeouts.request)
            .disable_proxy()
            .build()
            .map_err(|e| AppError::Internal(format!("opensearch transport build failed: {e}")))?;

        Ok(Self {
            os: OpenSearch::new(transport),
            retry: RetryPolicy::from_env(),
        })
    }

    /// Send an OpenSearch request, retrying transport errors per [`RetryPolicy`]. `make` builds a
    /// fresh send future on each attempt (the request builder is consumed by `send`). An HTTP error
    /// status is returned as `Ok(Response)` (not retried); only a transport `Err` is retried.
    pub async fn send_with_retry<F, Fut>(&self, mut make: F) -> Result<Response, AppError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Response, opensearch::Error>>,
    {
        let attempts = self.retry.total_attempts();
        let mut last_err: Option<opensearch::Error> = None;

        for attempt in 0..attempts {
            match make().await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if attempt + 1 >= attempts {
                        last_err = Some(e);
                    } else {
                        let delay = self.retry.backoff(attempt);
                        tracing::warn!(
                            attempt = attempt + 1,
                            attempts,
                            delay_ms = delay.as_millis(),
                            error = %e,
                            "OpenSearch request failed; retrying"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(AppError::Internal(format!(
            "OpenSearch request failed after {attempts} attempt(s): {}",
            last_err.map(|e| e.to_string()).unwrap_or_default()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_attempts_is_one_plus_retries() {
        let p = RetryPolicy {
            max_retries: 0,
            base_backoff: Duration::from_millis(100),
        };
        assert_eq!(p.total_attempts(), 1);

        let p = RetryPolicy {
            max_retries: 2,
            base_backoff: Duration::from_millis(100),
        };
        assert_eq!(p.total_attempts(), 3);
    }

    #[test]
    fn backoff_doubles_per_attempt() {
        let p = RetryPolicy {
            max_retries: 3,
            base_backoff: Duration::from_millis(250),
        };
        assert_eq!(p.backoff(0), Duration::from_millis(250));
        assert_eq!(p.backoff(1), Duration::from_millis(500));
        assert_eq!(p.backoff(2), Duration::from_millis(1000));
        assert_eq!(p.backoff(3), Duration::from_millis(2000));
    }

    #[test]
    fn backoff_saturates_and_does_not_panic_on_large_attempt() {
        let p = RetryPolicy {
            max_retries: 1,
            base_backoff: Duration::from_millis(250),
        };
        let _ = p.backoff(64);
    }

    // Owns the OPENSEARCH_* env keys (no other test touches them), so set/clear here is race-free.
    #[test]
    fn config_reads_env_overrides_and_falls_back_to_defaults() {
        for k in [
            "OPENSEARCH_TIMEOUT_MS",
            "OPENSEARCH_MAX_RETRIES",
            "OPENSEARCH_RETRY_BACKOFF_MS",
        ] {
            std::env::remove_var(k);
        }
        let t = OpenSearchTimeouts::from_env();
        assert_eq!(t.request, Duration::from_millis(12_000));
        let r = RetryPolicy::from_env();
        assert_eq!(r.max_retries, 1);
        assert_eq!(r.base_backoff, Duration::from_millis(250));

        std::env::set_var("OPENSEARCH_TIMEOUT_MS", "9000");
        std::env::set_var("OPENSEARCH_MAX_RETRIES", "2");
        std::env::set_var("OPENSEARCH_RETRY_BACKOFF_MS", "100");
        let t = OpenSearchTimeouts::from_env();
        assert_eq!(t.request, Duration::from_millis(9000));
        let r = RetryPolicy::from_env();
        assert_eq!(r.max_retries, 2);
        assert_eq!(r.base_backoff, Duration::from_millis(100));

        std::env::set_var("OPENSEARCH_MAX_RETRIES", "not-a-number");
        assert_eq!(RetryPolicy::from_env().max_retries, 1);

        for k in [
            "OPENSEARCH_TIMEOUT_MS",
            "OPENSEARCH_MAX_RETRIES",
            "OPENSEARCH_RETRY_BACKOFF_MS",
        ] {
            std::env::remove_var(k);
        }
    }
}
