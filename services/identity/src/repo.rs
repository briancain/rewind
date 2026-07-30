use aws_sdk_dynamodb::{types::AttributeValue, Client};
use shared::error::AppError;
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::UserProfile;

// --- Invite Codes ---

pub async fn validate_invite_code(db: &Client, code: &str) -> Result<(), AppError> {
    let mut key = HashMap::new();
    key.insert("code".to_string(), AttributeValue::S(code.to_string()));

    let item = shared::dynamo::get_item(db, &shared::tables::table("invite_codes"), key)
        .await?
        .ok_or_else(|| AppError::BadRequest("invalid or already used invite code".to_string()))?;

    let used = item
        .get("used")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(true);

    if used {
        return Err(AppError::BadRequest(
            "invalid or already used invite code".to_string(),
        ));
    }
    Ok(())
}

pub async fn consume_invite_code(db: &Client, code: &str) -> Result<(), AppError> {
    let result = db
        .update_item()
        .table_name(shared::tables::table("invite_codes"))
        .key("code", AttributeValue::S(code.to_string()))
        .update_expression("SET used = :t, used_at = :now")
        .condition_expression("attribute_exists(code) AND used = :f")
        .expression_attribute_values(":t", AttributeValue::Bool(true))
        .expression_attribute_values(":f", AttributeValue::Bool(false))
        .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
        .send()
        .await;

    // A failed conditional update means the code was already used or never existed — a client
    // error, not a server error.
    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(AppError::BadRequest(
            "invalid or already used invite code".to_string(),
        )),
    }
}

// --- Users ---

pub async fn find_user_by_email(
    db: &Client,
    email: &str,
) -> Result<Option<HashMap<String, AttributeValue>>, AppError> {
    let results = shared::dynamo::query_by_index(
        db,
        &shared::tables::table("users"),
        "email-index",
        "email",
        AttributeValue::S(email.to_string()),
    )
    .await?;

    Ok(results.into_iter().next())
}

pub async fn create_user(
    db: &Client,
    email: &str,
    display_name: &str,
    password_hash: &str,
) -> Result<String, AppError> {
    let user_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let mut item = HashMap::new();
    item.insert("user_id".to_string(), AttributeValue::S(user_id.clone()));
    item.insert("email".to_string(), AttributeValue::S(email.to_string()));
    item.insert(
        "display_name".to_string(),
        AttributeValue::S(display_name.to_string()),
    );
    item.insert(
        "password_hash".to_string(),
        AttributeValue::S(password_hash.to_string()),
    );
    item.insert("email_verified".to_string(), AttributeValue::Bool(false));
    item.insert("created_at".to_string(), AttributeValue::S(now));

    shared::dynamo::put_item(db, &shared::tables::table("users"), item).await?;
    Ok(user_id)
}

pub async fn get_user_by_id(db: &Client, user_id: &str) -> Result<UserProfile, AppError> {
    let mut key = HashMap::new();
    key.insert(
        "user_id".to_string(),
        AttributeValue::S(user_id.to_string()),
    );

    let item = shared::dynamo::get_item(db, &shared::tables::table("users"), key)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".to_string()))?;

    Ok(parse_user_profile(&item))
}

pub async fn mark_email_verified(db: &Client, user_id: &str) -> Result<(), AppError> {
    db.update_item()
        .table_name(shared::tables::table("users"))
        .key("user_id", AttributeValue::S(user_id.to_string()))
        .update_expression("SET email_verified = :v")
        .expression_attribute_values(":v", AttributeValue::Bool(true))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

fn parse_user_profile(item: &HashMap<String, AttributeValue>) -> UserProfile {
    UserProfile {
        user_id: item
            .get("user_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        email: item
            .get("email")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        display_name: item
            .get("display_name")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        email_verified: item
            .get("email_verified")
            .and_then(|v| v.as_bool().ok())
            .copied()
            .unwrap_or(false),
        created_at: item
            .get("created_at")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
    }
}

// --- Verification Tokens ---

pub async fn create_verification_token(db: &Client, user_id: &str) -> Result<String, AppError> {
    let token = Uuid::new_v4().to_string();
    let ttl = (chrono::Utc::now().timestamp() + 86400).to_string();

    let mut item = HashMap::new();
    item.insert("token".to_string(), AttributeValue::S(token.clone()));
    item.insert(
        "user_id".to_string(),
        AttributeValue::S(user_id.to_string()),
    );
    item.insert("ttl".to_string(), AttributeValue::N(ttl));

    shared::dynamo::put_item(db, &shared::tables::table("verification_tokens"), item).await?;
    Ok(token)
}

pub async fn consume_verification_token(db: &Client, token: &str) -> Result<String, AppError> {
    let mut key = HashMap::new();
    key.insert("token".to_string(), AttributeValue::S(token.to_string()));

    let item = shared::dynamo::get_item(
        db,
        &shared::tables::table("verification_tokens"),
        key.clone(),
    )
    .await?
    .ok_or_else(|| AppError::NotFound("invalid token".to_string()))?;

    let user_id = item
        .get("user_id")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| AppError::Internal("bad token data".to_string()))?;

    let _ =
        shared::dynamo::delete_item(db, &shared::tables::table("verification_tokens"), key).await;
    Ok(user_id)
}

// --- Sessions ---

pub async fn create_session(db: &Client, user_id: &str) -> Result<String, AppError> {
    let token = Uuid::new_v4().to_string();
    let ttl = (chrono::Utc::now().timestamp() + 7 * 86400).to_string();

    let mut item = HashMap::new();
    item.insert(
        "session_token".to_string(),
        AttributeValue::S(token.clone()),
    );
    item.insert(
        "user_id".to_string(),
        AttributeValue::S(user_id.to_string()),
    );
    item.insert("ttl".to_string(), AttributeValue::N(ttl));

    shared::dynamo::put_item(db, &shared::tables::table("sessions"), item).await?;
    Ok(token)
}

pub async fn delete_session(db: &Client, session_token: &str) -> Result<(), AppError> {
    let mut key = HashMap::new();
    key.insert(
        "session_token".to_string(),
        AttributeValue::S(session_token.to_string()),
    );
    shared::dynamo::delete_item(db, &shared::tables::table("sessions"), key).await?;
    Ok(())
}

pub async fn get_session_user_id(db: &Client, session_token: &str) -> Result<String, AppError> {
    let mut key = HashMap::new();
    key.insert(
        "session_token".to_string(),
        AttributeValue::S(session_token.to_string()),
    );

    let item = shared::dynamo::get_item(db, &shared::tables::table("sessions"), key)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid session".to_string()))?;

    item.get("user_id")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| AppError::Internal("bad session data".to_string()))
}

// --- Password management ---

/// Fetch a user's stored argon2 password hash by user_id (for verifying the current password).
pub async fn get_password_hash(db: &Client, user_id: &str) -> Result<String, AppError> {
    let mut key = HashMap::new();
    key.insert(
        "user_id".to_string(),
        AttributeValue::S(user_id.to_string()),
    );

    let item = shared::dynamo::get_item(db, &shared::tables::table("users"), key)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".to_string()))?;

    item.get("password_hash")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .ok_or_else(|| AppError::Internal("bad user data".to_string()))
}

pub async fn update_password(db: &Client, user_id: &str, new_hash: &str) -> Result<(), AppError> {
    db.update_item()
        .table_name(shared::tables::table("users"))
        .key("user_id", AttributeValue::S(user_id.to_string()))
        .update_expression("SET password_hash = :h")
        .expression_attribute_values(":h", AttributeValue::S(new_hash.to_string()))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

/// Delete all of a user's sessions except `keep_token` (used to log out other devices on password
/// change). Uses the sessions `user-id-index` GSI. Returns how many sessions were deleted.
pub async fn delete_other_sessions(
    db: &Client,
    user_id: &str,
    keep_token: &str,
) -> Result<u32, AppError> {
    let items = shared::dynamo::query_by_index(
        db,
        &shared::tables::table("sessions"),
        "user-id-index",
        "user_id",
        AttributeValue::S(user_id.to_string()),
    )
    .await?;

    let mut deleted = 0;
    for item in items {
        let Some(token) = item.get("session_token").and_then(|v| v.as_s().ok()) else {
            continue;
        };
        if token == keep_token {
            continue;
        }
        let mut key = HashMap::new();
        key.insert(
            "session_token".to_string(),
            AttributeValue::S(token.clone()),
        );
        shared::dynamo::delete_item(db, &shared::tables::table("sessions"), key).await?;
        deleted += 1;
    }
    Ok(deleted)
}
