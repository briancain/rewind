//! Thin DNS I/O for the region-routing check. Resolution goes through the
//! pod's own resolver + the region's NAT egress, so what this resolves is exactly what a user in
//! this region gets — which is the whole point of validating latency routing from in-region.
//!
//! Kept deliberately thin (like [`crate::client`]): it only performs the lookup and returns the
//! address set; the pure [`crate::assertions::assert_routed_in_region`] decides pass/fail.

use std::collections::HashSet;
use std::net::IpAddr;

/// Resolve a hostname to its set of IP addresses (A/AAAA records), via the system resolver.
///
/// Port 443 is appended only because `lookup_host` takes a socket address; it does not connect.
/// A resolution failure (NXDOMAIN, transient resolver error) is an `Err` so the calling step fails
/// loudly rather than silently treating an unresolvable host as "no addresses".
pub async fn resolve_ips(host: &str) -> Result<HashSet<IpAddr>, String> {
    let addrs = tokio::net::lookup_host((host, 443))
        .await
        .map_err(|e| format!("DNS resolution of {host} failed: {e}"))?;
    Ok(addrs.map(|sa| sa.ip()).collect())
}
