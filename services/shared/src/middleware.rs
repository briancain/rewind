use axum::{
    http::{HeaderMap, Request},
    Router,
};
use tower_http::{
    classify::ServerErrorsFailureClass,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Span;

/// The client address as observed by the ALB, for attributing a request to its source.
///
/// The ALB *appends* the TCP peer address to any client-supplied `X-Forwarded-For`
/// (`routing.http.xff_header_processing.mode = append`), so the **last** entry is the address the
/// load balancer actually saw and the only one that can be trusted — every earlier entry is
/// attacker-supplied and must never be used for attribution. Nothing fronts the ALBs (DESIGN §10.1
/// — Route 53 → ALB direct; the only CloudFront is the content CDN over the videos bucket), so the
/// ALB-observed peer is the real client.
///
/// Pure, so the trust boundary is unit-tested rather than assumed.
pub fn client_ip(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .rsplit(',')
        .map(str::trim)
        .find(|entry| !entry.is_empty())
}

/// Wraps a Router with production logging middleware:
/// - Request ID generation (x-request-id)
/// - Request ID propagation to responses
/// - Request/response tracing (method, path, client IP, status, latency)
pub fn with_logging<S: Clone + Send + Sync + 'static>(app: Router<S>) -> Router<S> {
    app.layer(PropagateRequestIdLayer::x_request_id())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|req: &Request<_>| {
                    let request_id = req
                        .headers()
                        .get("x-request-id")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("-");
                    // On the span, so it is attached to the response line AND to any error logged
                    // within the request — every 4xx/5xx becomes attributable to a source address
                    // for the full log-group retention, instead of only via WAF's sampled,
                    // 3-hour `GetSampledRequests` window.
                    let client_ip = client_ip(req.headers()).unwrap_or("-");
                    tracing::info_span!(
                        "request",
                        method = %req.method(),
                        path = %req.uri().path(),
                        request_id = %request_id,
                        client_ip = %client_ip,
                    )
                })
                .on_response(
                    |res: &axum::http::Response<_>, latency: std::time::Duration, _span: &Span| {
                        tracing::info!(
                            status = res.status().as_u16(),
                            latency_ms = latency.as_millis() as u64,
                            "response"
                        );
                    },
                )
                .on_failure(
                    |err: ServerErrorsFailureClass, latency: std::time::Duration, _span: &Span| {
                        tracing::error!(
                            error = %err,
                            latency_ms = latency.as_millis() as u64,
                            "request failed"
                        );
                    },
                ),
        )
        .layer(SetRequestIdLayer::new(
            "x-request-id".parse().unwrap(),
            MakeRequestUuid,
        ))
}

#[cfg(test)]
mod tests {
    use super::client_ip;
    use axum::http::HeaderMap;

    fn headers_with_xff(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", value.parse().unwrap());
        h
    }

    #[test]
    fn client_ip_is_none_without_the_header() {
        assert_eq!(client_ip(&HeaderMap::new()), None);
    }

    #[test]
    fn client_ip_reads_a_single_entry() {
        assert_eq!(
            client_ip(&headers_with_xff("203.0.113.7")),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn client_ip_takes_the_last_entry_not_the_first() {
        // The ALB appends the peer it observed, so the LAST entry is the trustworthy one. Taking the
        // leftmost would let a caller forge its own source address and misattribute an attack.
        assert_eq!(
            client_ip(&headers_with_xff("1.2.3.4, 203.0.113.7")),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn client_ip_ignores_a_spoofed_chain() {
        assert_eq!(
            client_ip(&headers_with_xff(
                "10.0.0.1, 192.168.1.1, 172.16.0.1, 203.0.113.7"
            )),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn client_ip_trims_whitespace() {
        assert_eq!(
            client_ip(&headers_with_xff("1.2.3.4 ,   203.0.113.7   ")),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn client_ip_skips_trailing_empty_entries() {
        assert_eq!(
            client_ip(&headers_with_xff("203.0.113.7, ,")),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn client_ip_is_none_for_an_all_blank_header() {
        assert_eq!(client_ip(&headers_with_xff(" , , ")), None);
    }

    #[test]
    fn client_ip_handles_ipv6() {
        assert_eq!(
            client_ip(&headers_with_xff("1.2.3.4, 2001:db8::1")),
            Some("2001:db8::1")
        );
    }
}
