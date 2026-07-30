use aws_sdk_dynamodb::{types::AttributeValue, Client};
use axum::http::HeaderMap;
use std::collections::HashMap;

use crate::dynamo;
use crate::error::AppError;

pub fn extract_token(headers: &HeaderMap) -> Result<String, AppError> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::Unauthorized("missing token".to_string()))
}

pub async fn get_session_user_id(db: &Client, session_token: &str) -> Result<String, AppError> {
    let mut key = HashMap::new();
    key.insert(
        "session_token".to_string(),
        AttributeValue::S(session_token.to_string()),
    );

    let item = dynamo::get_item(db, &crate::tables::table("sessions"), key)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid session".to_string()))?;

    // Check TTL expiration
    if let Some(ttl_val) = item.get("ttl").and_then(|v| v.as_n().ok()) {
        if let Ok(ttl) = ttl_val.parse::<i64>() {
            let now = chrono::Utc::now().timestamp();
            if now > ttl {
                return Err(AppError::Unauthorized("session expired".to_string()));
            }
        }
    }

    item.get("user_id")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| AppError::Internal("bad session data".to_string()))
}

pub async fn authenticate(db: &Client, headers: &HeaderMap) -> Result<String, AppError> {
    let token = extract_token(headers)?;
    get_session_user_id(db, &token).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn extract_token_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer abc123".parse().unwrap());
        assert_eq!(extract_token(&headers).unwrap(), "abc123");
    }

    #[test]
    fn extract_token_missing_header() {
        let headers = HeaderMap::new();
        let err = extract_token(&headers).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn extract_token_no_bearer_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Token abc123".parse().unwrap());
        let err = extract_token(&headers).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }
}
