use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// The one error type shared by every Rewind service. Request-serving services return it from
/// handlers (it is `IntoResponse`, mapping each variant to an HTTP status); worker services
/// (transcode, delete-cleanup, search consumer/backfill) use the same type so the platform has a
/// single error vocabulary — the HTTP status is simply unused on the worker paths, which only log
/// the error and decide whether to redrive.
///
/// The response body is the bare variant message (e.g. `"video not found"`), not the `Display`
/// string (`"not found: video not found"`); `Display` keeps the prefix for logs.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Build an `Internal` (500) from any displayable error — typically an AWS SDK or I/O error.
    /// The detail is logged when the response is produced (or by the caller on worker paths);
    /// prefer the semantic variants (`NotFound`, `Forbidden`, ...) when the condition is known.
    pub fn internal(e: impl std::fmt::Display) -> Self {
        AppError::Internal(e.to_string())
    }
}

// `?` ergonomics for the two SDKs every repo touches. Both map to `Internal` (a failed AWS call is
// a server-side error). Other sources (reqwest, serde_json, sigv4, std::io) map at the call site
// with `.map_err(AppError::internal)` so `shared` needn't depend on them.
impl From<aws_sdk_dynamodb::Error> for AppError {
    fn from(e: aws_sdk_dynamodb::Error) -> Self {
        AppError::internal(e)
    }
}

impl From<aws_sdk_s3::Error> for AppError {
    fn from(e: aws_sdk_s3::Error) -> Self {
        AppError::internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Internal(m) => {
                tracing::error!(error = %m, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, m)
            }
        };
        (status, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    #[test]
    fn status_codes_map_per_variant() {
        assert_eq!(
            AppError::NotFound("x".into()).into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AppError::BadRequest("x".into()).into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::Unauthorized("x".into()).into_response().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AppError::Forbidden("x".into()).into_response().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AppError::Conflict("x".into()).into_response().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AppError::Internal("x".into()).into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn response_body_is_the_bare_message_not_the_display_prefix() {
        // Clients see "video not found", not "not found: video not found" — this preserves the
        // exact bodies the previous (StatusCode, String) tuples produced.
        let resp = AppError::NotFound("video not found".into()).into_response();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"video not found");
    }

    #[test]
    fn display_keeps_prefix_for_logs() {
        assert_eq!(AppError::NotFound("x".into()).to_string(), "not found: x");
        assert_eq!(
            AppError::Forbidden("nope".into()).to_string(),
            "forbidden: nope"
        );
        assert_eq!(
            AppError::Internal("boom".into()).to_string(),
            "internal error: boom"
        );
    }

    #[test]
    fn internal_constructor_stringifies() {
        let e = AppError::internal("disk on fire");
        assert!(matches!(e, AppError::Internal(m) if m == "disk on fire"));
    }
}
