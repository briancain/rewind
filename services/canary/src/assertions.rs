//! Pure assertion + decision helpers. No I/O — every function here is deterministic and
//! unit-tested, so the canary's correctness logic is verified without a network or AWS.

use std::collections::HashSet;
use std::net::IpAddr;

use crate::models::{SearchResponse, Stats};

/// A canary assertion failure. Stringly-typed to match the convention used across the other
/// services' async paths (`search`, `delete-cleanup`).
pub type CanaryResult<T> = Result<T, String>;

/// Assert an HTTP status equals the expected value.
pub fn expect_status(actual: u16, expected: u16, ctx: &str) -> CanaryResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{ctx}: expected HTTP {expected}, got {actual}"))
    }
}

/// Assert an HTTP status is one of `allowed`.
pub fn expect_status_in(actual: u16, allowed: &[u16], ctx: &str) -> CanaryResult<()> {
    if allowed.contains(&actual) {
        Ok(())
    } else {
        Err(format!(
            "{ctx}: expected HTTP one of {allowed:?}, got {actual}"
        ))
    }
}

/// Assert a value is non-empty (e.g. a returned token or URL).
pub fn expect_non_empty(value: &str, ctx: &str) -> CanaryResult<()> {
    if value.trim().is_empty() {
        Err(format!("{ctx}: expected a non-empty value"))
    } else {
        Ok(())
    }
}

/// Assert two strings are equal.
pub fn expect_eq(actual: &str, expected: &str, ctx: &str) -> CanaryResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{ctx}: expected {expected:?}, got {actual:?}"))
    }
}

/// True if the search response contains a hit for the given video id.
pub fn search_contains(resp: &SearchResponse, video_id: &str) -> bool {
    resp.results.iter().any(|h| h.video_id == video_id)
}

/// Assert the search response is usable for the shallow tier: it must have a coherent shape, and —
/// when `expect_hit` is set — at least one result for the queried term.
pub fn assert_search(resp: &SearchResponse, expect_hit: bool, ctx: &str) -> CanaryResult<()> {
    if expect_hit && resp.results.is_empty() {
        return Err(format!(
            "{ctx}: expected at least one search result, got 0 (total={})",
            resp.total
        ));
    }
    Ok(())
}

/// Assert that the latency-routed public hostname resolved to the *same* ALB as this region's
/// region-pinned hostname — i.e. Route 53 latency routing kept this region's traffic in-region.
///
/// `latency_ips` are the freshly-resolved A-records of a latency-routed host (the wildcard/apex —
/// what a user in this region hits); `region_ips` are those of `<region>.<domain>`, a plain alias
/// to *this* region's ALB (the ground truth). When routing is correct both alias the same ALB, so
/// the sets intersect; a disjoint result means the latency record sent us to the other region's
/// ALB (a routing regression). Intersection — not equality — tolerates DNS returning rotated or
/// partial subsets of the ALB's address pool between the two lookups.
pub fn assert_routed_in_region(
    latency_ips: &HashSet<IpAddr>,
    region_ips: &HashSet<IpAddr>,
    ctx: &str,
) -> CanaryResult<()> {
    if latency_ips.is_empty() {
        return Err(format!("{ctx}: latency host resolved to no addresses"));
    }
    if region_ips.is_empty() {
        return Err(format!(
            "{ctx}: region-pinned host resolved to no addresses"
        ));
    }
    if latency_ips.is_disjoint(region_ips) {
        return Err(format!(
            "{ctx}: latency-routed host did not resolve to this region's ALB \
             (latency={latency_ips:?}, region-pinned={region_ips:?}) — possible mis-routing"
        ));
    }
    Ok(())
}

/// Assert the social stats reflect the deep tier's interactions (at least one like, one comment,
/// one view were recorded on the seeded video).
pub fn assert_engaged_stats(stats: &Stats, ctx: &str) -> CanaryResult<()> {
    let mut problems = Vec::new();
    if stats.likes < 1 {
        problems.push(format!("likes={} (<1)", stats.likes));
    }
    if stats.comment_count < 1 {
        problems.push(format!("comment_count={} (<1)", stats.comment_count));
    }
    if stats.views < 1 {
        problems.push(format!("views={} (<1)", stats.views));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{ctx}: {}", problems.join(", ")))
    }
}

/// Tally of a deleted video's remaining dependent resources. Every field must be zero (and the
/// stats row absent) for the cascade to be considered complete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub comments: usize,
    pub reactions: usize,
    pub comment_reactions: usize,
    pub view_history: usize,
    pub transcode_jobs: usize,
    pub stats_present: bool,
    pub videos_objects: usize,
    pub raw_objects: usize,
}

impl CleanupReport {
    /// True once nothing dependent on the video remains.
    pub fn is_clean(&self) -> bool {
        self.comments == 0
            && self.reactions == 0
            && self.comment_reactions == 0
            && self.view_history == 0
            && self.transcode_jobs == 0
            && !self.stats_present
            && self.videos_objects == 0
            && self.raw_objects == 0
    }

    /// Human-readable list of what is still lingering (for the failure message / logs).
    pub fn remaining(&self) -> String {
        let mut parts = Vec::new();
        if self.comments > 0 {
            parts.push(format!("comments={}", self.comments));
        }
        if self.reactions > 0 {
            parts.push(format!("reactions={}", self.reactions));
        }
        if self.comment_reactions > 0 {
            parts.push(format!("comment_reactions={}", self.comment_reactions));
        }
        if self.view_history > 0 {
            parts.push(format!("view_history={}", self.view_history));
        }
        if self.transcode_jobs > 0 {
            parts.push(format!("transcode_jobs={}", self.transcode_jobs));
        }
        if self.stats_present {
            parts.push("video_stats=present".to_string());
        }
        if self.videos_objects > 0 {
            parts.push(format!("videos_s3={}", self.videos_objects));
        }
        if self.raw_objects > 0 {
            parts.push(format!("raw_s3={}", self.raw_objects));
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SearchHit, SearchResponse};

    #[test]
    fn expect_status_ok_and_err() {
        assert!(expect_status(200, 200, "ctx").is_ok());
        let err = expect_status(500, 200, "register").unwrap_err();
        assert!(err.contains("expected HTTP 200"));
        assert!(err.contains("got 500"));
    }

    #[test]
    fn expect_status_in_matches_any() {
        assert!(expect_status_in(204, &[200, 204], "ctx").is_ok());
        assert!(expect_status_in(403, &[200, 204], "ctx").is_err());
    }

    #[test]
    fn expect_non_empty_rejects_blank() {
        assert!(expect_non_empty("token", "ctx").is_ok());
        assert!(expect_non_empty("   ", "stream url").is_err());
    }

    #[test]
    fn expect_eq_reports_mismatch() {
        assert!(expect_eq("a", "a", "ctx").is_ok());
        assert!(expect_eq("a", "b", "user_id").is_err());
    }

    fn search_resp(ids: &[&str]) -> SearchResponse {
        SearchResponse {
            results: ids
                .iter()
                .map(|id| SearchHit {
                    video_id: (*id).to_string(),
                    title: String::new(),
                })
                .collect(),
            total: ids.len() as u64,
        }
    }

    #[test]
    fn search_contains_finds_id() {
        let resp = search_resp(&["v1", "v2"]);
        assert!(search_contains(&resp, "v2"));
        assert!(!search_contains(&resp, "v3"));
    }

    #[test]
    fn assert_search_requires_hit_only_when_expected() {
        let empty = search_resp(&[]);
        assert!(assert_search(&empty, true, "search").is_err());
        assert!(assert_search(&empty, false, "search").is_ok());
        let one = search_resp(&["v1"]);
        assert!(assert_search(&one, true, "search").is_ok());
    }

    #[test]
    fn routed_in_region_passes_when_ips_intersect() {
        use std::collections::HashSet;
        use std::net::IpAddr;
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // Latency host returned a rotated/partial subset that still overlaps the region's ALB pool.
        let latency: HashSet<IpAddr> = [ip("10.0.1.5"), ip("10.0.2.9")].into_iter().collect();
        let region: HashSet<IpAddr> = [ip("10.0.2.9"), ip("10.0.3.1")].into_iter().collect();
        assert!(assert_routed_in_region(&latency, &region, "region-routing").is_ok());
    }

    #[test]
    fn routed_in_region_fails_when_disjoint() {
        use std::collections::HashSet;
        use std::net::IpAddr;
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // Latency host resolved entirely to the *other* region's ALB.
        let latency: HashSet<IpAddr> = [ip("52.10.0.1")].into_iter().collect();
        let region: HashSet<IpAddr> = [ip("3.20.0.1")].into_iter().collect();
        let err = assert_routed_in_region(&latency, &region, "region-routing").unwrap_err();
        assert!(err.contains("did not resolve to this region's ALB"));
    }

    #[test]
    fn routed_in_region_fails_on_empty_resolution() {
        use std::collections::HashSet;
        use std::net::IpAddr;
        let some: HashSet<IpAddr> = ["10.0.0.1".parse().unwrap()].into_iter().collect();
        let empty: HashSet<IpAddr> = HashSet::new();
        assert!(assert_routed_in_region(&empty, &some, "region-routing")
            .unwrap_err()
            .contains("latency host resolved to no addresses"));
        assert!(assert_routed_in_region(&some, &empty, "region-routing")
            .unwrap_err()
            .contains("region-pinned host resolved to no addresses"));
    }

    #[test]
    fn assert_engaged_stats_requires_all_three() {
        let good = Stats {
            likes: 1,
            dislikes: 0,
            views: 1,
            comment_count: 1,
        };
        assert!(assert_engaged_stats(&good, "stats").is_ok());

        let no_likes = Stats {
            likes: 0,
            dislikes: 0,
            views: 5,
            comment_count: 2,
        };
        let err = assert_engaged_stats(&no_likes, "stats").unwrap_err();
        assert!(err.contains("likes=0"));
    }

    #[test]
    fn cleanup_report_default_is_clean() {
        assert!(CleanupReport::default().is_clean());
        assert_eq!(CleanupReport::default().remaining(), "none");
    }

    #[test]
    fn cleanup_report_dirty_when_anything_remains() {
        let r = CleanupReport {
            comments: 2,
            stats_present: true,
            ..Default::default()
        };
        assert!(!r.is_clean());
        let remaining = r.remaining();
        assert!(remaining.contains("comments=2"));
        assert!(remaining.contains("video_stats=present"));
    }

    #[test]
    fn cleanup_report_clean_with_zero_counts_and_absent_stats() {
        let r = CleanupReport {
            stats_present: false,
            ..Default::default()
        };
        assert!(r.is_clean());
    }
}
