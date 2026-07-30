use crate::models::VideoStatus;
use aws_sdk_dynamodb::{types::AttributeValue, Client};
use shared::error::AppError;
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::Video;

pub async fn create_video(
    db: &Client,
    channel_id: &str,
    title: &str,
    description: &str,
    genre: &str,
    tags: &[String],
) -> Result<Video, AppError> {
    let video_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let mut item = HashMap::new();
    item.insert("video_id".to_string(), AttributeValue::S(video_id.clone()));
    item.insert(
        "channel_id".to_string(),
        AttributeValue::S(channel_id.to_string()),
    );
    item.insert("title".to_string(), AttributeValue::S(title.to_string()));
    item.insert(
        "description".to_string(),
        AttributeValue::S(description.to_string()),
    );
    item.insert("genre".to_string(), AttributeValue::S(genre.to_string()));
    item.insert(
        "tags".to_string(),
        AttributeValue::L(tags.iter().map(|t| AttributeValue::S(t.clone())).collect()),
    );
    item.insert(
        "status".to_string(),
        AttributeValue::S(VideoStatus::Draft.as_str().to_string()),
    );
    item.insert(
        "visibility".to_string(),
        AttributeValue::S("public".to_string()),
    );
    item.insert("created_at".to_string(), AttributeValue::S(now.clone()));
    item.insert("updated_at".to_string(), AttributeValue::S(now.clone()));

    shared::dynamo::put_item(db, &shared::tables::table("videos"), item).await?;

    Ok(Video {
        video_id,
        channel_id: channel_id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        genre: genre.to_string(),
        tags: tags.to_vec(),
        status: VideoStatus::Draft,
        visibility: crate::models::Visibility::Public,
        thumbnail_url: None,
        manifest_url: None,
        duration_seconds: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

pub async fn get_video(db: &Client, video_id: &str) -> Result<Video, AppError> {
    let mut key = HashMap::new();
    key.insert(
        "video_id".to_string(),
        AttributeValue::S(video_id.to_string()),
    );

    let item = shared::dynamo::get_item(db, &shared::tables::table("videos"), key)
        .await?
        .ok_or_else(|| AppError::NotFound("video not found".to_string()))?;

    let video = parse_video(&item);
    // A soft-deleted video is a tombstone awaiting cleanup; treat it as gone.
    if video.status == VideoStatus::Deleted {
        return Err(AppError::NotFound("video not found".to_string()));
    }
    Ok(video)
}

pub async fn list_by_channel(db: &Client, channel_id: &str) -> Result<Vec<Video>, AppError> {
    let items = shared::dynamo::query_by_index(
        db,
        &shared::tables::table("videos"),
        "channel-index",
        "channel_id",
        AttributeValue::S(channel_id.to_string()),
    )
    .await?;

    Ok(items
        .iter()
        .map(parse_video)
        .filter(|v| v.status != VideoStatus::Deleted)
        .collect())
}

pub async fn list_feed(db: &Client) -> Result<Vec<Video>, AppError> {
    let items = shared::dynamo::query_by_index(
        db,
        &shared::tables::table("videos"),
        "status-index",
        "status",
        AttributeValue::S(VideoStatus::Published.as_str().to_string()),
    )
    .await?;

    let mut videos: Vec<Video> = items
        .iter()
        .map(parse_video)
        .filter(|v| v.visibility == crate::models::Visibility::Public)
        .collect();
    videos.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(videos)
}

pub async fn update_video(
    db: &Client,
    video_id: &str,
    title: Option<&str>,
    description: Option<&str>,
    genre: Option<&str>,
    tags: Option<&[String]>,
    visibility: Option<&crate::models::Visibility>,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut expr_parts = vec!["updated_at = :now".to_string()];
    let mut values = HashMap::new();
    values.insert(":now".to_string(), AttributeValue::S(now));

    if let Some(t) = title {
        expr_parts.push("title = :title".to_string());
        values.insert(":title".to_string(), AttributeValue::S(t.to_string()));
    }
    if let Some(d) = description {
        expr_parts.push("description = :desc".to_string());
        values.insert(":desc".to_string(), AttributeValue::S(d.to_string()));
    }
    if let Some(g) = genre {
        expr_parts.push("genre = :genre".to_string());
        values.insert(":genre".to_string(), AttributeValue::S(g.to_string()));
    }
    if let Some(t) = tags {
        expr_parts.push("tags = :tags".to_string());
        values.insert(
            ":tags".to_string(),
            AttributeValue::L(t.iter().map(|s| AttributeValue::S(s.clone())).collect()),
        );
    }
    if let Some(v) = visibility {
        expr_parts.push("visibility = :vis".to_string());
        values.insert(
            ":vis".to_string(),
            AttributeValue::S(v.as_str().to_string()),
        );
    }

    let expr = format!("SET {}", expr_parts.join(", "));

    db.update_item()
        .table_name(shared::tables::table("videos"))
        .key("video_id", AttributeValue::S(video_id.to_string()))
        .update_expression(expr)
        .set_expression_attribute_values(Some(values))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(())
}

pub async fn update_status(
    db: &Client,
    video_id: &str,
    status: VideoStatus,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();

    db.update_item()
        .table_name(shared::tables::table("videos"))
        .key("video_id", AttributeValue::S(video_id.to_string()))
        .update_expression("SET #s = :status, updated_at = :now")
        .expression_attribute_names("#s", "status")
        .expression_attribute_values(":status", AttributeValue::S(status.as_str().to_string()))
        .expression_attribute_values(":now", AttributeValue::S(now))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(())
}

pub async fn get_published_video_ids(db: &Client) -> Result<Vec<String>, AppError> {
    let items = shared::dynamo::query_by_index(
        db,
        &shared::tables::table("videos"),
        "status-index",
        "status",
        AttributeValue::S(VideoStatus::Published.as_str().to_string()),
    )
    .await?;

    Ok(items
        .iter()
        .filter(|item| {
            item.get("visibility")
                .and_then(|v| v.as_s().ok())
                .unwrap_or(&"public".to_string())
                == "public"
        })
        .filter_map(|item| item.get("video_id").and_then(|v| v.as_s().ok()).cloned())
        .collect())
}

fn parse_video(item: &HashMap<String, AttributeValue>) -> Video {
    Video {
        video_id: item
            .get("video_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        channel_id: item
            .get("channel_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        title: item
            .get("title")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        description: item
            .get("description")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        genre: item
            .get("genre")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        tags: item
            .get("tags")
            .and_then(|v| v.as_l().ok())
            .map(|l| l.iter().filter_map(|v| v.as_s().ok().cloned()).collect())
            .unwrap_or_default(),
        status: item
            .get("status")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default(),
        visibility: item
            .get("visibility")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default(),
        thumbnail_url: item
            .get("thumbnail_url")
            .and_then(|v| v.as_s().ok())
            .cloned(),
        manifest_url: item
            .get("manifest_url")
            .and_then(|v| v.as_s().ok())
            .cloned(),
        created_at: item
            .get("created_at")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        updated_at: item
            .get("updated_at")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default(),
        duration_seconds: item
            .get("duration_seconds")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<f64>().ok()),
    }
}

/// Soft-delete: mark the video as a `deleted` tombstone (and stamp `deleted_at`) rather than removing
/// the row. The status change flows through the videos stream to the cleanup worker (which reclaims
/// the dependent data) and makes the video non-indexable so search drops it; a TTL/finalizer later
/// removes the tombstone. Soft-delete also keeps deletes resurrection-safe under multi-region Global
/// Tables.
pub async fn delete_video(db: &Client, video_id: &str) -> Result<(), AppError> {
    let now = chrono::Utc::now();
    let now_rfc = now.to_rfc3339();
    // `purge_at` (epoch seconds) is the DynamoDB TTL attribute that hard-deletes this tombstone after
    // a grace window (the TTL finalizer). It is set here at delete time — NOT by the cleanup
    // worker — so the worker never writes the videos row (a `deleted`->`deleted` MODIFY would
    // otherwise be re-forwarded by the cleanup Pipe's `status==deleted` filter, looping). Cleanup
    // finishes in seconds and DynamoDB's TTL deletion lags by hours, so the row is always purged well
    // after cleanup; the 24h grace also gives an audit/undo window.
    const TOMBSTONE_GRACE_SECS: i64 = 86_400; // 24h
    let purge_at = now.timestamp() + TOMBSTONE_GRACE_SECS;
    db.update_item()
        .table_name(shared::tables::table("videos"))
        .key("video_id", AttributeValue::S(video_id.to_string()))
        .update_expression(
            "SET #s = :deleted, deleted_at = :now, updated_at = :now, purge_at = :purge",
        )
        .expression_attribute_names("#s", "status")
        .expression_attribute_values(
            ":deleted",
            AttributeValue::S(VideoStatus::Deleted.as_str().to_string()),
        )
        .expression_attribute_values(":now", AttributeValue::S(now_rfc))
        .expression_attribute_values(":purge", AttributeValue::N(purge_at.to_string()))
        .send()
        .await
        .map_err(AppError::internal)?;
    Ok(())
}
