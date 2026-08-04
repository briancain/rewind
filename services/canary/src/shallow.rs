//! Shallow tier: hourly, read-only, no writes, no teardown. Health endpoints +
//! public feed + a search query against the existing real videos + an error-contract probe
//! (malformed input must be 4xx, not 5xx). The cheap liveness/correctness signal — safe to run
//! frequently.

use std::time::Instant;

use crate::assertions::{
    assert_routed_in_region, assert_search, expect_client_error, expect_status,
};
use crate::client::RewindClient;
use crate::config::CanaryConfig;
use crate::dns::resolve_ips;
use crate::models::{Feed, SearchResponse};
use crate::report::RunReport;

pub async fn run(client: &RewindClient, cfg: &CanaryConfig) -> RunReport {
    let mut report = RunReport::new("shallow");

    // 1. Health of every public service.
    for (name, base) in client.endpoints.all() {
        let url = format!("{base}/health");
        let start = Instant::now();
        let result = match client.get(&url, None).await {
            Ok(resp) => expect_status(resp.status, 200, &format!("{name} /health")),
            Err(e) => Err(e),
        };
        report.record(format!("health:{name}"), start.elapsed(), result);
    }

    // 2. Public feed returns a well-formed list.
    {
        let url = format!("{}/videos/feed", client.endpoints.catalog);
        let start = Instant::now();
        let result: Result<(), String> = async {
            let resp = client.get(&url, None).await?;
            expect_status(resp.status, 200, "catalog /videos/feed")?;
            let _feed: Feed = resp.json()?;
            Ok(())
        }
        .await;
        report.record("feed", start.elapsed(), result);
    }

    // 3. Search query path (against the stable, pre-existing public videos).
    {
        let url = format!(
            "{}/search?q={}",
            client.endpoints.search,
            urlencode(&cfg.search_term)
        );
        let start = Instant::now();
        let result: Result<(), String> = async {
            let resp = client.get(&url, None).await?;
            expect_status(resp.status, 200, "search /search")?;
            let body: SearchResponse = resp.json()?;
            assert_search(&body, cfg.search_expect_hit, "search /search")
        }
        .await;
        report.record("search", start.elapsed(), result);
    }

    // 4. Error-contract probe: malformed input must come back 4xx, never 5xx. Two shapes on two
    //    services — a blank JSON body field and a blank query param — both of which land in a
    //    DynamoDB key position, which is where an unvalidated empty string turns into a 500 and
    //    pollutes the per-service 5xx alarms. Writes nothing (both requests fail validation before
    //    any mutation), so it stays safe for the read-only tier.
    {
        let start = Instant::now();
        let result: Result<(), String> = async {
            let login = client
                .post(
                    &format!("{}/login", client.endpoints.identity),
                    None,
                    Some(serde_json::json!({ "email": "", "password": "x" })),
                )
                .await?;
            expect_client_error(login.status, "identity /login (blank email)")?;

            let list = client
                .get(
                    &format!("{}/videos?channel_id=", client.endpoints.catalog),
                    None,
                )
                .await?;
            expect_client_error(list.status, "catalog /videos (blank channel_id)")
        }
        .await;
        report.record("input-validation", start.elapsed(), result);
    }

    // 5. Region routing (cloud-only): from inside this region, the latency-routed public host must
    //    resolve to THIS region's own ALB — i.e. the same address pool as the region-pinned host
    //    `<region>.<domain>`. A disjoint result means Route 53 latency routing sent this region's
    //    traffic to the other region's ALB (a regression). Skipped locally (no public latency
    //    records under dev.sh) and when CANARY_CHECK_REGION_ROUTING is off.
    if cfg.check_region_routing {
        if let (Some(latency_host), Some(region_host)) =
            (cfg.latency_host(), cfg.region_pinned_host())
        {
            let start = Instant::now();
            let result: Result<(), String> = async {
                let latency_ips = resolve_ips(&latency_host).await?;
                let region_ips = resolve_ips(&region_host).await?;
                assert_routed_in_region(
                    &latency_ips,
                    &region_ips,
                    &format!("region-routing ({latency_host} -> {})", cfg.region),
                )
            }
            .await;
            report.record("region-routing", start.elapsed(), result);
        }
    }

    report
}

/// Minimal percent-encoding for a query-string value (spaces and the handful of reserved chars the
/// canary's configurable search term might contain). Avoids pulling in a URL-encoding dependency.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::urlencode;

    #[test]
    fn urlencode_passes_unreserved() {
        assert_eq!(urlencode("Rewind-video_1.0~"), "Rewind-video_1.0~");
    }

    #[test]
    fn urlencode_escapes_space_and_specials() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }
}
