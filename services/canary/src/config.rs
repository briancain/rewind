//! Canary configuration, resolved entirely from the environment so the same image runs anywhere
//! (cloud CronJob, local `dev.sh`). The endpoint resolution and naming helpers are pure and
//! unit-tested; nothing here performs I/O.

use std::time::Duration;

/// Base URLs for each backend service the canary calls.
///
/// In the cloud each service is a public subdomain of `${DOMAIN}` (the exact subdomains the
/// frontend and ALB ingress use — see `scripts/deploy.sh`). Locally they are the fixed `dev.sh`
/// ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub identity: String,
    pub catalog: String,
    pub upload: String,
    pub streaming: String,
    pub social: String,
    pub search: String,
}

impl Endpoints {
    /// Cloud endpoints: `https://{service}.{domain}`. A trailing slash on `domain` is tolerated.
    pub fn cloud(domain: &str) -> Self {
        let d = domain.trim().trim_end_matches('/');
        Self {
            identity: format!("https://identity.{d}"),
            catalog: format!("https://catalog.{d}"),
            upload: format!("https://upload.{d}"),
            streaming: format!("https://streaming.{d}"),
            social: format!("https://social.{d}"),
            search: format!("https://search.{d}"),
        }
    }

    /// Local `dev.sh` endpoints (fixed localhost ports).
    pub fn local() -> Self {
        Self {
            identity: "http://localhost:8080".into(),
            catalog: "http://localhost:8081".into(),
            upload: "http://localhost:8082".into(),
            streaming: "http://localhost:8083".into(),
            social: "http://localhost:8084".into(),
            search: "http://localhost:8085".into(),
        }
    }

    /// All six base URLs, for iterating health checks.
    pub fn all(&self) -> [(&'static str, &str); 6] {
        [
            ("identity", &self.identity),
            ("catalog", &self.catalog),
            ("upload", &self.upload),
            ("streaming", &self.streaming),
            ("social", &self.social),
            ("search", &self.search),
        ]
    }
}

/// Credentials for one of the persistent canary accounts.
#[derive(Debug, Clone)]
pub struct Account {
    pub email: String,
    pub password: String,
}

/// Fully-resolved canary configuration.
#[derive(Debug, Clone)]
pub struct CanaryConfig {
    pub endpoints: Endpoints,
    /// Present only in cloud mode; used to build the seeded video's `manifest_url` and ephemeral
    /// email addresses.
    pub domain: Option<String>,
    pub region: String,
    /// Base for the seeded video's HLS manifest URL (e.g. `https://cdn.${DOMAIN}`). The streaming
    /// service returns this verbatim for unlisted videos, so the exact host is unimportant for the
    /// canary's assertion (it checks the service returns 200 + a URL, not real playback).
    pub cdn_base: String,
    pub owner: Option<Account>,
    pub viewer: Option<Account>,
    /// Whether the `deep` tier asserts the cascade reclaimed all dependent data. Cloud-only
    /// (the Pipe→SQS→worker pipeline doesn't exist against DynamoDB Local/LocalStack), so this is
    /// turned off for local integration via `CANARY_VERIFY_CASCADE=false`.
    pub verify_cascade: bool,
    pub cascade_timeout: Duration,
    pub cascade_poll_interval: Duration,
    /// The term the `shallow` search step queries.
    pub search_term: String,
    /// Whether `shallow` requires the search query to return at least one hit. True in cloud (real
    /// videos exist); false locally where the index may be empty.
    pub search_expect_hit: bool,
    /// Whether to emit the `Rewind/Canary` CloudWatch metric. Cloud-only.
    pub emit_metrics: bool,
    /// Whether `shallow` validates Route 53 latency routing keeps this region's traffic in-region
    /// (resolve a latency-routed host + the region-pinned host and assert they hit the same ALB).
    /// Cloud-only — there are no public latency records under local `dev.sh`. Override via
    /// `CANARY_CHECK_REGION_ROUTING`.
    pub check_region_routing: bool,
    /// S3 buckets used for the `deep` cascade media-cleanup verification.
    pub video_bucket: String,
    pub raw_bucket: String,
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn account_from_env(email_key: &str, password_key: &str) -> Option<Account> {
    let email = std::env::var(email_key).ok().filter(|s| !s.is_empty())?;
    let password = std::env::var(password_key).ok().filter(|s| !s.is_empty())?;
    Some(Account { email, password })
}

impl CanaryConfig {
    /// Resolve config from the environment. `CANARY_DOMAIN` (set in cloud) selects cloud endpoints;
    /// absent → local `dev.sh` endpoints.
    pub fn from_env() -> Self {
        let domain = std::env::var("CANARY_DOMAIN")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let endpoints = match &domain {
            Some(d) => Endpoints::cloud(d),
            None => Endpoints::local(),
        };

        let region = env_or("CANARY_REGION", &env_or("AWS_REGION", "us-west-2"));

        // In cloud the seeded manifest points at the CDN subdomain; locally any URL works (the
        // streaming service echoes it back for unlisted videos).
        let cdn_base = match (std::env::var("CANARY_CDN_BASE").ok(), &domain) {
            (Some(b), _) if !b.trim().is_empty() => b,
            (_, Some(d)) => format!("https://cdn.{}", d.trim().trim_end_matches('/')),
            (_, None) => "http://localhost/cdn".to_string(),
        };

        CanaryConfig {
            endpoints,
            domain,
            region,
            cdn_base,
            owner: account_from_env("CANARY_OWNER_EMAIL", "CANARY_OWNER_PASSWORD"),
            viewer: account_from_env("CANARY_VIEWER_EMAIL", "CANARY_VIEWER_PASSWORD"),
            verify_cascade: env_bool("CANARY_VERIFY_CASCADE", true),
            cascade_timeout: Duration::from_secs(
                env_or("CANARY_CASCADE_TIMEOUT_SECS", "120")
                    .parse()
                    .unwrap_or(120),
            ),
            cascade_poll_interval: Duration::from_secs(
                env_or("CANARY_CASCADE_POLL_SECS", "5").parse().unwrap_or(5),
            ),
            search_term: env_or("CANARY_SEARCH_TERM", "video"),
            search_expect_hit: env_bool("CANARY_SEARCH_EXPECT_HIT", domain_is_some_default()),
            emit_metrics: env_bool("CANARY_EMIT_METRICS", false),
            check_region_routing: env_bool("CANARY_CHECK_REGION_ROUTING", domain_is_some_default()),
            video_bucket: env_or("VIDEO_BUCKET", "rewind-videos"),
            raw_bucket: env_or("RAW_BUCKET", "rewind-raw"),
        }
    }

    /// Build the seeded video's manifest URL for a given video id.
    pub fn manifest_url(&self, video_id: &str) -> String {
        format!(
            "{}/hls/{}/master.m3u8",
            self.cdn_base.trim_end_matches('/'),
            video_id
        )
    }

    /// The email domain for ephemeral auth users. SES is disabled, so deliverability is irrelevant;
    /// `.invalid` guarantees the address can never route anywhere real.
    pub fn ephemeral_email(&self, run_id: &str) -> String {
        format!("canary-{run_id}@canary.invalid")
    }

    /// This region's region-pinned hostname (`<region>.<domain>`) — a plain A-alias to *this*
    /// region's ALB (NOT latency-routed), used as the ground truth for the routing check. `None`
    /// in local mode (no public DNS). Mirrors the Terraform `aws_route53_record.region_pinned`.
    pub fn region_pinned_host(&self) -> Option<String> {
        self.domain
            .as_ref()
            .map(|d| format!("{}.{}", self.region, d.trim().trim_end_matches('/')))
    }

    /// A latency-routed host to validate routing against: the `catalog` service subdomain, which
    /// the canary's other steps also traverse (covered by the `*.<domain>` latency record). `None`
    /// in local mode. Returns the bare host (no scheme) for DNS resolution.
    pub fn latency_host(&self) -> Option<String> {
        self.domain
            .as_ref()
            .map(|d| format!("catalog.{}", d.trim().trim_end_matches('/')))
    }
}

/// Default for `CANARY_SEARCH_EXPECT_HIT` depends on whether we're in cloud mode; this reads the
/// same env var `from_env` keys off so the default is consistent.
fn domain_is_some_default() -> bool {
    std::env::var("CANARY_DOMAIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_endpoints_use_service_subdomains() {
        let ep = Endpoints::cloud("watch.example.dev");
        assert_eq!(ep.identity, "https://identity.watch.example.dev");
        assert_eq!(ep.catalog, "https://catalog.watch.example.dev");
        assert_eq!(ep.upload, "https://upload.watch.example.dev");
        assert_eq!(ep.streaming, "https://streaming.watch.example.dev");
        assert_eq!(ep.social, "https://social.watch.example.dev");
        assert_eq!(ep.search, "https://search.watch.example.dev");
    }

    #[test]
    fn cloud_endpoints_tolerate_trailing_slash_and_whitespace() {
        let ep = Endpoints::cloud("  watch.example.dev/  ");
        assert_eq!(ep.identity, "https://identity.watch.example.dev");
    }

    #[test]
    fn local_endpoints_use_dev_ports() {
        let ep = Endpoints::local();
        assert_eq!(ep.identity, "http://localhost:8080");
        assert_eq!(ep.catalog, "http://localhost:8081");
        assert_eq!(ep.search, "http://localhost:8085");
    }

    #[test]
    fn all_lists_six_services() {
        assert_eq!(Endpoints::local().all().len(), 6);
    }

    #[test]
    fn manifest_url_is_built_under_cdn_base() {
        let cfg = CanaryConfig {
            endpoints: Endpoints::local(),
            domain: Some("watch.example.dev".into()),
            region: "us-west-2".into(),
            cdn_base: "https://cdn.watch.example.dev".into(),
            owner: None,
            viewer: None,
            verify_cascade: true,
            cascade_timeout: Duration::from_secs(120),
            cascade_poll_interval: Duration::from_secs(5),
            search_term: "video".into(),
            search_expect_hit: true,
            emit_metrics: true,
            check_region_routing: true,
            video_bucket: "rewind-videos".into(),
            raw_bucket: "rewind-raw".into(),
        };
        assert_eq!(
            cfg.manifest_url("abc"),
            "https://cdn.watch.example.dev/hls/abc/master.m3u8"
        );
    }

    #[test]
    fn ephemeral_email_is_unroutable() {
        let cfg = CanaryConfig {
            endpoints: Endpoints::local(),
            domain: None,
            region: "us-west-2".into(),
            cdn_base: "http://localhost/cdn".into(),
            owner: None,
            viewer: None,
            verify_cascade: false,
            cascade_timeout: Duration::from_secs(1),
            cascade_poll_interval: Duration::from_secs(1),
            search_term: "video".into(),
            search_expect_hit: false,
            emit_metrics: false,
            check_region_routing: false,
            video_bucket: "rewind-videos".into(),
            raw_bucket: "rewind-raw".into(),
        };
        let email = cfg.ephemeral_email("run123");
        assert_eq!(email, "canary-run123@canary.invalid");
        assert!(email.ends_with(".invalid"));
    }

    /// Build a config for the routing-host tests (only `domain` + `region` matter here).
    fn cfg_for_hosts(domain: Option<&str>, region: &str) -> CanaryConfig {
        CanaryConfig {
            endpoints: Endpoints::local(),
            domain: domain.map(|d| d.to_string()),
            region: region.to_string(),
            cdn_base: "http://localhost/cdn".into(),
            owner: None,
            viewer: None,
            verify_cascade: false,
            cascade_timeout: Duration::from_secs(1),
            cascade_poll_interval: Duration::from_secs(1),
            search_term: "video".into(),
            search_expect_hit: false,
            emit_metrics: false,
            check_region_routing: false,
            video_bucket: "rewind-videos".into(),
            raw_bucket: "rewind-raw".into(),
        }
    }

    #[test]
    fn region_pinned_and_latency_hosts_derive_from_domain_and_region() {
        let cfg = cfg_for_hosts(Some("watch.example.dev"), "us-east-2");
        assert_eq!(
            cfg.region_pinned_host().as_deref(),
            Some("us-east-2.watch.example.dev")
        );
        assert_eq!(
            cfg.latency_host().as_deref(),
            Some("catalog.watch.example.dev")
        );
    }

    #[test]
    fn routing_hosts_tolerate_trailing_slash() {
        let cfg = cfg_for_hosts(Some("watch.example.dev/"), "us-west-2");
        assert_eq!(
            cfg.region_pinned_host().as_deref(),
            Some("us-west-2.watch.example.dev")
        );
    }

    #[test]
    fn routing_hosts_are_none_in_local_mode() {
        let cfg = cfg_for_hosts(None, "us-west-2");
        assert!(cfg.region_pinned_host().is_none());
        assert!(cfg.latency_host().is_none());
    }
}
