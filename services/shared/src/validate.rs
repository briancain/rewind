//! Guards for caller-supplied values that end up in a DynamoDB **key** position — a table's hash /
//! range key, or a GSI key-condition value.
//!
//! DynamoDB rejects an empty string in any key attribute with a `ValidationException`. Passing
//! caller input straight through to the SDK therefore turns a *client* mistake (`{"email": ""}`,
//! `?channel_id=`) into an unhandled SDK error, which the shared `From<aws_sdk_dynamodb::Error>`
//! surfaces as a 500. That breaks the platform's error contract (DESIGN §10.10: a 5xx means an
//! *unexpected* fault) and lets any unauthenticated caller inflate the per-service 5xx alarms at
//! will. `error::from_dynamo_code` is the backstop; validating here is the fix, because only the
//! handler knows the field name to put in the message.
//!
//! Pure and AWS-free, so every rule unit-tests directly.

use crate::error::AppError;

/// Require a caller-supplied field to be present and non-blank.
///
/// Rejects whitespace-only input as well as `""`: a blank key would not trip DynamoDB's empty-string
/// rule but is never a legitimate identifier, and matching the existing `trim().is_empty()` guards
/// (upload's `/initiate`, catalog's `create_video`) keeps one rule across the platform.
pub fn non_empty(field: &str, value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(AppError::BadRequest(format!("{field} is required")));
    }
    Ok(())
}

/// Bound a caller-supplied field's length. DynamoDB caps a hash key at 2048 bytes and a range key at
/// 1024, and an oversized value is another client-caused `ValidationException`.
pub fn max_len(field: &str, value: &str, max: usize) -> Result<(), AppError> {
    if value.len() > max {
        return Err(AppError::BadRequest(format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(())
}

/// The longest caller-supplied string we will place in a key attribute. Well under DynamoDB's
/// 1024-byte range-key limit, and generous for an email / id / invite code.
pub const MAX_KEY_LEN: usize = 512;

/// `non_empty` + `max_len(MAX_KEY_LEN)` — the full guard for a value headed for a key attribute.
pub fn key_field(field: &str, value: &str) -> Result<(), AppError> {
    non_empty(field, value)?;
    max_len(field, value, MAX_KEY_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn non_empty_accepts_a_normal_value() {
        assert!(non_empty("email", "someone@example.com").is_ok());
    }

    #[test]
    fn non_empty_rejects_the_empty_string() {
        // The exact shape that produced a 500: `{"email": ""}` / `?channel_id=`.
        let err = non_empty("email", "").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn non_empty_rejects_whitespace_only() {
        for blank in [" ", "\t", "\n", "   \t "] {
            assert!(
                non_empty("invite_code", blank).is_err(),
                "{blank:?} should be rejected"
            );
        }
    }

    #[test]
    fn non_empty_maps_to_400_not_500() {
        // The whole point of the guard: client input must never reach the 5xx alarms.
        let status = non_empty("email", "").unwrap_err().into_response().status();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn non_empty_message_names_the_field() {
        assert_eq!(
            non_empty("invite_code", "").unwrap_err().to_string(),
            "bad request: invite_code is required"
        );
    }

    #[test]
    fn non_empty_does_not_trim_the_accepted_value() {
        // Padding is tolerated; only fully-blank input is rejected (callers pass the original value
        // through, so this must not imply the value gets trimmed).
        assert!(non_empty("email", " a@b.c ").is_ok());
    }

    #[test]
    fn max_len_accepts_at_the_boundary_and_rejects_past_it() {
        assert!(max_len("email", &"a".repeat(10), 10).is_ok());
        assert!(max_len("email", &"a".repeat(11), 10).is_err());
    }

    #[test]
    fn max_len_message_states_the_limit() {
        assert_eq!(
            max_len("email", "abc", 2).unwrap_err().to_string(),
            "bad request: email must be at most 2 characters"
        );
    }

    #[test]
    fn key_field_rejects_both_blank_and_oversized() {
        assert!(key_field("code", "").is_err());
        assert!(key_field("code", &"a".repeat(MAX_KEY_LEN + 1)).is_err());
        assert!(key_field("code", &"a".repeat(MAX_KEY_LEN)).is_ok());
    }

    #[test]
    fn key_field_accepts_a_uuid_and_an_email() {
        assert!(key_field("video_id", "e4bb95ec-3468-49d2-9252-727641db0812").is_ok());
        assert!(key_field("email", "someone@example.com").is_ok());
    }
}
