use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use shared::error::AppError;
use std::collections::HashMap;

use crate::models::*;
use crate::password;
use crate::repo;
use crate::state::AppState;

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<AuthResponse>), AppError> {
    repo::validate_invite_code(&state.db, &req.invite_code).await?;

    if req.password.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".to_string(),
        ));
    }

    if repo::find_user_by_email(&state.db, &req.email)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("email already registered".to_string()));
    }

    let password_hash = password::hash_password(&req.password).map_err(AppError::internal)?;

    let user_id =
        repo::create_user(&state.db, &req.email, &req.display_name, &password_hash).await?;

    repo::consume_invite_code(&state.db, &req.invite_code).await?;

    let verify_token = repo::create_verification_token(&state.db, &user_id).await?;

    if let Some(ses) = &state.ses {
        let link = format!("{}/verify?token={}", state.base_url, verify_token);
        let _ = send_verification_email(ses, &state.from_email, &req.email, &link).await;
    }

    let token = repo::create_session(&state.db, &user_id).await?;
    Ok((StatusCode::CREATED, Json(AuthResponse { token, user_id })))
}

pub async fn verify_email(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let token = params
        .get("token")
        .ok_or_else(|| AppError::BadRequest("missing token".to_string()))?;

    let user_id = repo::consume_verification_token(&state.db, token).await?;
    repo::mark_email_verified(&state.db, &user_id).await?;

    Ok((StatusCode::OK, Json(serde_json::json!({"verified": true}))))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let user = repo::find_user_by_email(&state.db, &req.email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid credentials".to_string()))?;

    let hash = user
        .get("password_hash")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| AppError::Internal("bad user data".to_string()))?;

    if !password::verify_password(&req.password, hash) {
        return Err(AppError::Unauthorized("invalid credentials".to_string()));
    }

    let user_id = user
        .get("user_id")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| AppError::Internal("bad user data".to_string()))?;

    let token = repo::create_session(&state.db, &user_id).await?;
    Ok(Json(AuthResponse { token, user_id }))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, AppError> {
    let token = shared::auth::extract_token(&headers)?;
    repo::delete_session(&state.db, &token).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Change the authenticated user's password. Verifies the current password, sets a new argon2 hash,
/// and invalidates all of the user's OTHER sessions (the current session stays valid).
pub async fn change_password(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    // Authenticate, capturing the current session token so we can keep it alive.
    let token = shared::auth::extract_token(&headers)?;
    let user_id = shared::auth::get_session_user_id(&state.db, &token).await?;

    if req.new_password.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".to_string(),
        ));
    }

    let current_hash = repo::get_password_hash(&state.db, &user_id).await?;
    if !password::verify_password(&req.current_password, &current_hash) {
        return Err(AppError::Unauthorized(
            "current password is incorrect".to_string(),
        ));
    }

    let new_hash = password::hash_password(&req.new_password).map_err(AppError::internal)?;
    repo::update_password(&state.db, &user_id, &new_hash).await?;

    // Log out other devices; keep the caller's current session.
    let invalidated = repo::delete_other_sessions(&state.db, &user_id, &token).await?;
    tracing::info!(user_id = %user_id, invalidated_sessions = invalidated, "password changed");

    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UserProfile>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let profile = repo::get_user_by_id(&state.db, &user_id).await?;
    Ok(Json(profile))
}

// --- Auth helpers ---

fn extract_token(headers: &axum::http::HeaderMap) -> Result<String, AppError> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::Unauthorized("missing token".to_string()))
}

pub async fn get_authenticated_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<String, AppError> {
    let token = extract_token(headers)?;
    repo::get_session_user_id(&state.db, &token).await
}

// --- Email ---

async fn send_verification_email(
    ses: &aws_sdk_ses::Client,
    from: &str,
    to: &str,
    link: &str,
) -> Result<(), aws_sdk_ses::Error> {
    ses.send_email()
        .source(from)
        .destination(
            aws_sdk_ses::types::Destination::builder()
                .to_addresses(to)
                .build(),
        )
        .message(
            aws_sdk_ses::types::Message::builder()
                .subject(
                    aws_sdk_ses::types::Content::builder()
                        .data("Verify your Rewind account")
                        .build()
                        .unwrap(),
                )
                .body(
                    aws_sdk_ses::types::Body::builder()
                        .html(
                            aws_sdk_ses::types::Content::builder()
                                .data(format!(
                                    "<h1>Welcome to Rewind!</h1><p>Click <a href=\"{}\">here</a> to verify your email.</p>",
                                    link
                                ))
                                .build()
                                .unwrap(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await?;
    Ok(())
}

pub async fn get_user(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let profile = repo::get_user_by_id(&state.db, &user_id).await?;
    Ok(Json(serde_json::json!({
        "user_id": profile.user_id,
        "display_name": profile.display_name,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_token_valid() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer mytoken".parse().unwrap());
        assert_eq!(extract_token(&headers).unwrap(), "mytoken");
    }

    #[test]
    fn extract_token_missing() {
        let headers = axum::http::HeaderMap::new();
        let err = extract_token(&headers).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn extract_token_wrong_scheme() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Basic abc".parse().unwrap());
        let err = extract_token(&headers).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }
}
