use aws_sdk_dynamodb::{types::AttributeValue, Client};
use shared::error::AppError;
use std::collections::HashMap;

use crate::models::{Comment, StatsResponse};

/// Toggle a reaction. Returns true if added, false if removed.
pub async fn toggle_reaction(
    db: &Client,
    video_id: &str,
    user_id: &str,
    reaction: &str,
) -> Result<bool, AppError> {
    let key = reaction_key(video_id, user_id);
    let existing = db
        .get_item()
        .table_name(shared::tables::table("reactions"))
        .set_key(Some(key.clone()))
        .send()
        .await
        .map_err(AppError::internal)?;

    if let Some(item) = existing.item() {
        let current = item.get("reaction").and_then(|v| v.as_s().ok());
        if current == Some(&reaction.to_string()) {
            // Same reaction exists — remove it
            db.delete_item()
                .table_name(shared::tables::table("reactions"))
                .set_key(Some(key))
                .send()
                .await
                .map_err(AppError::internal)?;
            update_stat(db, video_id, reaction, -1).await?;
            return Ok(false);
        }
        // Different reaction — switch it
        let old_reaction = current.cloned().unwrap_or_default();
        update_stat(db, video_id, &old_reaction, -1).await?;
    }

    // Add new reaction
    let mut item = key;
    item.insert("reaction".into(), AttributeValue::S(reaction.into()));
    db.put_item()
        .table_name(shared::tables::table("reactions"))
        .set_item(Some(item))
        .send()
        .await
        .map_err(AppError::internal)?;
    update_stat(db, video_id, reaction, 1).await?;
    Ok(true)
}

pub async fn add_comment(
    db: &Client,
    video_id: &str,
    user_id: &str,
    text: &str,
) -> Result<Comment, AppError> {
    let comment_id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();

    let mut item = HashMap::new();
    item.insert("video_id".into(), AttributeValue::S(video_id.into()));
    item.insert("comment_id".into(), AttributeValue::S(comment_id.clone()));
    item.insert("user_id".into(), AttributeValue::S(user_id.into()));
    item.insert("text".into(), AttributeValue::S(text.into()));
    item.insert("created_at".into(), AttributeValue::S(created_at.clone()));

    db.put_item()
        .table_name(shared::tables::table("comments"))
        .set_item(Some(item))
        .send()
        .await
        .map_err(AppError::internal)?;

    update_stat(db, video_id, "comment_count", 1).await?;

    Ok(Comment {
        comment_id,
        video_id: video_id.into(),
        user_id: user_id.into(),
        text: text.into(),
        created_at,
        likes: 0,
    })
}

pub async fn list_comments(db: &Client, video_id: &str) -> Result<Vec<Comment>, AppError> {
    let resp = db
        .query()
        .table_name(shared::tables::table("comments"))
        .key_condition_expression("video_id = :v")
        .expression_attribute_values(":v", AttributeValue::S(video_id.into()))
        .scan_index_forward(false)
        .send()
        .await
        .map_err(AppError::internal)?;

    let comments = resp
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(parse_comment)
        .collect();
    Ok(comments)
}

pub async fn increment_views(db: &Client, video_id: &str) -> Result<(), AppError> {
    update_stat(db, video_id, "views", 1).await
}

pub async fn get_stats(db: &Client, video_id: &str) -> Result<StatsResponse, AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    let resp = db
        .get_item()
        .table_name(shared::tables::table("video_stats"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;

    let item = resp.item.unwrap_or_default();
    Ok(StatsResponse {
        video_id: video_id.into(),
        likes: get_n(&item, "like"),
        dislikes: get_n(&item, "dislike"),
        views: get_n(&item, "views"),
        comment_count: get_n(&item, "comment_count"),
    })
}

// --- helpers ---

async fn update_stat(db: &Client, video_id: &str, stat: &str, delta: i64) -> Result<(), AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    db.update_item()
        .table_name(shared::tables::table("video_stats"))
        .set_key(Some(key))
        .update_expression("ADD #s :d")
        .expression_attribute_names("#s", stat)
        .expression_attribute_values(":d", AttributeValue::N(delta.to_string()))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

fn reaction_key(video_id: &str, user_id: &str) -> HashMap<String, AttributeValue> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));
    key.insert("user_id".into(), AttributeValue::S(user_id.into()));
    key
}

fn get_n(item: &HashMap<String, AttributeValue>, key: &str) -> i64 {
    item.get(key)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn parse_comment(item: HashMap<String, AttributeValue>) -> Option<Comment> {
    Some(Comment {
        comment_id: item.get("comment_id")?.as_s().ok()?.clone(),
        video_id: item.get("video_id")?.as_s().ok()?.clone(),
        user_id: item.get("user_id")?.as_s().ok()?.clone(),
        text: item.get("text")?.as_s().ok()?.clone(),
        created_at: item.get("created_at")?.as_s().ok()?.clone(),
        likes: get_n(&item, "likes"),
    })
}

pub async fn toggle_comment_reaction(
    db: &Client,
    video_id: &str,
    comment_id: &str,
    user_id: &str,
    reaction_type: &str,
) -> Result<String, AppError> {
    // comment_reactions is keyed PK=video_id, SK="{comment_id}#{user_id}" so all of a video's
    // comment reactions are a single Query (used by the cascade cleanup).
    let sk = format!("{}#{}", comment_id, user_id);
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));
    key.insert("sk".into(), AttributeValue::S(sk.clone()));

    // Check existing reaction
    let existing = db
        .get_item()
        .table_name(shared::tables::table("comment_reactions"))
        .set_key(Some(key.clone()))
        .send()
        .await
        .map_err(AppError::internal)?
        .item;

    let old_type = existing
        .as_ref()
        .and_then(|item| item.get("reaction_type"))
        .and_then(|v| v.as_s().ok())
        .cloned();

    // Comment key for updating likes count
    let mut comment_key = HashMap::new();
    comment_key.insert("video_id".into(), AttributeValue::S(video_id.into()));
    comment_key.insert("comment_id".into(), AttributeValue::S(comment_id.into()));

    match old_type.as_deref() {
        Some(existing_type) if existing_type == reaction_type => {
            // Same reaction again — remove it (toggle off)
            db.delete_item()
                .table_name(shared::tables::table("comment_reactions"))
                .set_key(Some(key))
                .send()
                .await
                .map_err(AppError::internal)?;
            let delta: i64 = if reaction_type == "like" { -1 } else { 1 };
            db.update_item()
                .table_name(shared::tables::table("comments"))
                .set_key(Some(comment_key))
                .update_expression("ADD likes :d")
                .expression_attribute_values(":d", AttributeValue::N(delta.to_string()))
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok("removed".into())
        }
        Some(_) => {
            // Switching reaction (like→dislike or dislike→like)
            let mut item = HashMap::new();
            item.insert("video_id".into(), AttributeValue::S(video_id.into()));
            item.insert("sk".into(), AttributeValue::S(sk.clone()));
            item.insert(
                "reaction_type".into(),
                AttributeValue::S(reaction_type.into()),
            );
            shared::dynamo::put_item(db, &shared::tables::table("comment_reactions"), item).await?;
            // Swing by 2 (undo old + apply new)
            let delta: i64 = if reaction_type == "like" { 2 } else { -2 };
            db.update_item()
                .table_name(shared::tables::table("comments"))
                .set_key(Some(comment_key))
                .update_expression("ADD likes :d")
                .expression_attribute_values(":d", AttributeValue::N(delta.to_string()))
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok("switched".into())
        }
        None => {
            // New reaction
            let mut item = HashMap::new();
            item.insert("video_id".into(), AttributeValue::S(video_id.into()));
            item.insert("sk".into(), AttributeValue::S(sk.clone()));
            item.insert(
                "reaction_type".into(),
                AttributeValue::S(reaction_type.into()),
            );
            shared::dynamo::put_item(db, &shared::tables::table("comment_reactions"), item).await?;
            let delta: i64 = if reaction_type == "like" { 1 } else { -1 };
            db.update_item()
                .table_name(shared::tables::table("comments"))
                .set_key(Some(comment_key))
                .update_expression("ADD likes :d")
                .expression_attribute_values(":d", AttributeValue::N(delta.to_string()))
                .send()
                .await
                .map_err(AppError::internal)?;
            Ok("added".into())
        }
    }
}

pub async fn delete_comment(
    db: &Client,
    video_id: &str,
    comment_id: &str,
    user_id: &str,
) -> Result<(), AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));
    key.insert("comment_id".into(), AttributeValue::S(comment_id.into()));

    // Verify ownership. A missing comment is a 404 and a non-owner is a 403 — previously these were
    // String errors that the handler flattened to 500 (inflating the 5xx signal); now they map to
    // the correct client-error statuses.
    let item = db
        .get_item()
        .table_name(shared::tables::table("comments"))
        .set_key(Some(key.clone()))
        .send()
        .await
        .map_err(AppError::internal)?
        .item
        .ok_or_else(|| AppError::NotFound("comment not found".to_string()))?;

    let owner = item
        .get("user_id")
        .and_then(|v| v.as_s().ok())
        .ok_or_else(|| AppError::Internal("bad comment data".to_string()))?;

    if owner != user_id {
        return Err(AppError::Forbidden("not your comment".to_string()));
    }

    db.delete_item()
        .table_name(shared::tables::table("comments"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(())
}

pub async fn record_view_history(
    db: &Client,
    user_id: &str,
    video_id: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut item = HashMap::new();
    item.insert("user_id".into(), AttributeValue::S(user_id.into()));
    item.insert("watched_at".into(), AttributeValue::S(now));
    item.insert("video_id".into(), AttributeValue::S(video_id.into()));

    shared::dynamo::put_item(db, &shared::tables::table("view_history"), item).await?;
    Ok(())
}

pub async fn get_view_history(
    db: &Client,
    user_id: &str,
) -> Result<Vec<crate::models::HistoryEntry>, AppError> {
    let results = db
        .query()
        .table_name(shared::tables::table("view_history"))
        .key_condition_expression("user_id = :uid")
        .expression_attribute_values(":uid", AttributeValue::S(user_id.into()))
        .scan_index_forward(false) // newest first
        .limit(50)
        .send()
        .await
        .map_err(AppError::internal)?;

    let entries = results
        .items()
        .iter()
        .filter_map(|item| {
            Some(crate::models::HistoryEntry {
                video_id: item.get("video_id")?.as_s().ok()?.clone(),
                watched_at: item.get("watched_at")?.as_s().ok()?.clone(),
            })
        })
        .collect();

    Ok(entries)
}

pub async fn delete_view_history_entry(
    db: &Client,
    user_id: &str,
    watched_at: &str,
) -> Result<(), AppError> {
    let mut key = HashMap::new();
    key.insert("user_id".into(), AttributeValue::S(user_id.into()));
    key.insert("watched_at".into(), AttributeValue::S(watched_at.into()));

    shared::dynamo::delete_item(db, &shared::tables::table("view_history"), key).await?;
    Ok(())
}
