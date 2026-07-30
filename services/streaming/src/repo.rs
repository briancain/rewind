use aws_sdk_dynamodb::{types::AttributeValue, Client as DynamoClient};
use aws_sdk_s3::{presigning::PresigningConfig, Client as S3Client};
use shared::error::AppError;
use shared::video::{VideoStatus, Visibility};
use std::collections::HashMap;
use std::time::Duration;

const PRESIGN_EXPIRY_SECS: u64 = 3600;

/// Look up the S3 key for a video from the videos table.
pub async fn get_video_s3_key(
    db: &DynamoClient,
    video_id: &str,
) -> Result<Option<String>, AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    let resp = db
        .get_item()
        .table_name(shared::tables::table("videos"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(resp
        .item
        .and_then(|item| item.get("s3_key").and_then(|v| v.as_s().ok()).cloned()))
}

/// Look up visibility, channel_id, and status for access control. `status` lets callers reject a
/// soft-deleted ("deleted") video — a tombstone awaiting cleanup that must not be streamable.
pub async fn get_video_access_info(
    db: &DynamoClient,
    video_id: &str,
) -> Result<Option<(Visibility, String, VideoStatus)>, AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    let resp = db
        .get_item()
        .table_name(shared::tables::table("videos"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(resp.item.map(|item| {
        let visibility = item
            .get("visibility")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let channel_id = item
            .get("channel_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default();
        let status = item
            .get("status")
            .and_then(|v| v.as_s().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        (visibility, channel_id, status)
    }))
}

/// Look up the thumbnail S3 key for a video.
pub async fn get_video_thumbnail_key(
    db: &DynamoClient,
    video_id: &str,
) -> Result<Option<String>, AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    let resp = db
        .get_item()
        .table_name(shared::tables::table("videos"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(resp.item.and_then(|item| {
        item.get("thumbnail_url")
            .and_then(|v| v.as_s().ok())
            .cloned()
    }))
}

/// Look up the HLS manifest URL (a full CloudFront URL) for a video, if it has one. Present for
/// MediaConvert-transcoded videos; absent for legacy progressive uploads.
pub async fn get_video_manifest_url(
    db: &DynamoClient,
    video_id: &str,
) -> Result<Option<String>, AppError> {
    let mut key = HashMap::new();
    key.insert("video_id".into(), AttributeValue::S(video_id.into()));

    let resp = db
        .get_item()
        .table_name(shared::tables::table("videos"))
        .set_key(Some(key))
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(resp.item.and_then(|item| {
        item.get("manifest_url")
            .and_then(|v| v.as_s().ok())
            .filter(|s| !s.is_empty())
            .cloned()
    }))
}

/// Generate a presigned GET URL for the video file.
pub async fn presign_get_url(s3: &S3Client, bucket: &str, key: &str) -> Result<String, AppError> {
    let presign_config = PresigningConfig::expires_in(Duration::from_secs(PRESIGN_EXPIRY_SECS))
        .expect("valid duration");

    let presigned = s3
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(presign_config)
        .await
        .map_err(AppError::internal)?;

    Ok(presigned.uri().to_string())
}

pub fn expiry_secs() -> u64 {
    PRESIGN_EXPIRY_SECS
}
