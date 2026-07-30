use axum::http::HeaderValue;
use tower_http::cors::{Any, CorsLayer};

/// Returns a CORS layer. If `ALLOWED_ORIGIN` is set, restricts to that origin.
/// Otherwise (local dev), allows any origin.
pub fn permissive() -> CorsLayer {
    if let Ok(origin) = std::env::var("ALLOWED_ORIGIN") {
        CorsLayer::new()
            .allow_origin(origin.parse::<HeaderValue>().unwrap())
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}
