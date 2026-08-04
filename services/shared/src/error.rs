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

    /// Classify an AWS service **error code** into the right HTTP-mapped variant.
    ///
    /// This is the pure core of [`from_aws`](Self::from_aws) (no SDK types), so the mapping is
    /// unit-testable without constructing a real `SdkError`. Client-caused S3 conditions map to a
    /// 4xx; anything else (throttling, network, permissions, timeouts — `code` is `None` for
    /// non-service errors) is a server fault and stays a 500. Keeping infra faults as the *only*
    /// 500s is what keeps the per-service 5xx alarms meaningful (see DESIGN §10.10).
    pub fn from_aws_code(context: &str, code: Option<&str>) -> AppError {
        match code.unwrap_or("") {
            // The upload session is stale/aborted/never-existed — the caller must re-initiate.
            "NoSuchUpload" => AppError::BadRequest(
                "upload session is invalid or has expired; please restart the upload".to_string(),
            ),
            // The assembled part set didn't match S3 (missing/mismatched etag, ordering, or a
            // non-final part below the 5 MiB minimum) — a client/upload problem, not a server fault.
            "InvalidPart" | "InvalidPartOrder" | "EntityTooSmall" => AppError::BadRequest(
                "uploaded parts could not be assembled; please retry the upload".to_string(),
            ),
            "NoSuchKey" | "NoSuchBucket" => AppError::NotFound("upload not found".to_string()),
            other => {
                let detail = if other.is_empty() { "unknown" } else { other };
                AppError::Internal(format!("{context}: {detail}"))
            }
        }
    }

    /// Classify a **DynamoDB** service error code, the sibling of [`from_aws_code`](Self::from_aws_code).
    ///
    /// `ValidationException` means DynamoDB rejected the *request itself* as malformed. Every call
    /// that reaches this classifier goes through the [`crate::dynamo`] helpers (`get_item`,
    /// `put_item`, `delete_item`, `query_by_index`, `scan_all`, `batch_delete`), which build their
    /// requests purely from a caller-supplied key/item map and a fixed key-condition expression —
    /// so a validation failure there is caller *data* (an empty string in a key attribute, an
    /// oversized item, a wrong key type), not a server-side coding error. Mapping it to a 400 is
    /// what stops `{"email": ""}` from being counted as an unexpected fault by the per-service 5xx
    /// alarms (DESIGN §10.10).
    ///
    /// Server-assembled expressions deliberately do *not* route through here: the dynamic
    /// `update_item` calls in the repos use `.map_err(AppError::internal)`, so a genuine
    /// expression bug still surfaces as a 500 and still pages the 5xx alarm.
    ///
    /// Everything else — throttling, permissions, network, timeouts (`code` is `None` for a
    /// non-service error) — is an infra fault and stays a 500.
    pub fn from_dynamo_code(code: Option<&str>) -> AppError {
        match code.unwrap_or("") {
            "ValidationException" => AppError::BadRequest("invalid request parameter".to_string()),
            other => {
                let detail = if other.is_empty() { "unknown" } else { other };
                AppError::Internal(format!("dynamodb: {detail}"))
            }
        }
    }

    /// Map an AWS SDK error to an [`AppError`], logging the underlying service code and the full
    /// error context. Use this (rather than [`internal`](Self::internal)) at S3/AWS call sites where
    /// a client-caused failure should surface as a 4xx instead of a spurious 500 — most notably the
    /// upload `/complete` path, where S3 returns `NoSuchUpload`/`InvalidPart` for a stale or
    /// malformed multipart upload. The code is logged so the failure is diagnosable (an SDK
    /// `SdkError`'s bare `Display` is only the useless string `"service error"`).
    pub fn from_aws<E, R>(context: &str, err: aws_sdk_s3::error::SdkError<E, R>) -> AppError
    where
        E: aws_smithy_types::error::metadata::ProvideErrorMetadata + std::error::Error + 'static,
        R: std::fmt::Debug,
    {
        use aws_smithy_types::error::metadata::ProvideErrorMetadata;
        let code = err.code().map(str::to_string);
        tracing::error!(
            context = context,
            aws_error_code = code.as_deref().unwrap_or("none"),
            error = %aws_smithy_types::error::display::DisplayErrorContext(&err),
            "aws sdk call failed"
        );
        AppError::from_aws_code(context, code.as_deref())
    }
}

// `?` ergonomics for the two SDKs every repo touches. Other sources (reqwest, serde_json, sigv4,
// std::io) map at the call site with `.map_err(AppError::internal)` so `shared` needn't depend on
// them.
//
// DynamoDB errors are *classified* (see `from_dynamo_code`) rather than blanket-mapped to `Internal`:
// the `dynamo` helpers build their requests from caller-supplied keys, so a `ValidationException`
// there is bad client input and must be a 4xx. The code is logged because an
// `aws_sdk_dynamodb::Error`'s bare `Display` is only `"unhandled error (ValidationException)"` — no
// operation, no table, no key.
impl From<aws_sdk_dynamodb::Error> for AppError {
    fn from(e: aws_sdk_dynamodb::Error) -> Self {
        use aws_smithy_types::error::metadata::ProvideErrorMetadata;
        let code = e.code().map(str::to_string);
        tracing::error!(
            aws_error_code = code.as_deref().unwrap_or("none"),
            error = %e,
            "dynamodb call failed"
        );
        AppError::from_dynamo_code(code.as_deref())
    }
}

// A failed S3 call reached through `?` is a server-side error; call sites that need client-error
// classification use `from_aws` explicitly.
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

    #[test]
    fn from_aws_code_maps_client_caused_multipart_errors_to_400() {
        for code in [
            "NoSuchUpload",
            "InvalidPart",
            "InvalidPartOrder",
            "EntityTooSmall",
        ] {
            let e = AppError::from_aws_code("complete_multipart_upload", Some(code));
            assert_eq!(
                e.into_response().status(),
                StatusCode::BAD_REQUEST,
                "S3 code {code} should map to 400"
            );
        }
    }

    #[test]
    fn from_aws_code_maps_missing_key_or_bucket_to_404() {
        for code in ["NoSuchKey", "NoSuchBucket"] {
            let e = AppError::from_aws_code("list_parts", Some(code));
            assert_eq!(e.into_response().status(), StatusCode::NOT_FOUND);
        }
    }

    #[test]
    fn from_aws_code_maps_infra_and_unknown_codes_to_500() {
        // Throttling / permissions / network are server-side faults and must stay 500 so the
        // per-service 5xx alarms keep meaning "unexpected fault".
        for code in [
            Some("ThrottlingException"),
            Some("AccessDenied"),
            Some("SlowDown"),
            Some("InternalError"),
            None, // non-service error (timeout / dispatch failure) has no code
        ] {
            let e = AppError::from_aws_code("complete_multipart_upload", code);
            assert_eq!(
                e.into_response().status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "code {code:?} should map to 500"
            );
        }
    }

    #[test]
    fn from_aws_code_internal_message_includes_context_and_code() {
        assert_eq!(
            AppError::from_aws_code("list_parts", Some("Throttling")).to_string(),
            "internal error: list_parts: Throttling"
        );
        assert_eq!(
            AppError::from_aws_code("list_parts", None).to_string(),
            "internal error: list_parts: unknown"
        );
    }

    #[test]
    fn from_dynamo_code_maps_validation_exception_to_400() {
        // Caller data DynamoDB refuses — an empty string in a key attribute, an oversized item.
        // This must not be counted as an unexpected fault by the per-service 5xx alarms.
        let e = AppError::from_dynamo_code(Some("ValidationException"));
        assert_eq!(e.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn from_dynamo_code_validation_message_leaks_no_internals() {
        assert_eq!(
            AppError::from_dynamo_code(Some("ValidationException")).to_string(),
            "bad request: invalid request parameter"
        );
    }

    #[test]
    fn from_dynamo_code_maps_infra_and_unknown_codes_to_500() {
        for code in [
            Some("ProvisionedThroughputExceededException"),
            Some("ThrottlingException"),
            Some("AccessDeniedException"),
            Some("InternalServerError"),
            Some("ResourceNotFoundException"),
            None, // non-service error (timeout / dispatch failure) has no code
        ] {
            let e = AppError::from_dynamo_code(code);
            assert_eq!(
                e.into_response().status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "code {code:?} should stay a 500"
            );
        }
    }

    #[test]
    fn from_dynamo_code_internal_message_includes_the_code() {
        assert_eq!(
            AppError::from_dynamo_code(Some("ThrottlingException")).to_string(),
            "internal error: dynamodb: ThrottlingException"
        );
        assert_eq!(
            AppError::from_dynamo_code(None).to_string(),
            "internal error: dynamodb: unknown"
        );
    }
}
