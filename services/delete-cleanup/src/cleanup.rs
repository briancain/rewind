//! Idempotent reclamation of all data that belongs to a deleted `video_id`, split into two
//! self-contained functions (`cleanup_social`, `cleanup_media`) that take only a `video_id` and
//! share no state. This is deliberate: when the cascade is later fanned out to per-service
//! consumers (the future per-service fan-out), each function lifts verbatim into its owning service.
//!
//! Everything here is idempotent — querying yields nothing and deleting an absent row/object is a
//! no-op — so at-least-once delivery and redrive are safe.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_sdk_s3::Client as S3Client;
use shared::error::AppError;

use crate::state::AppState;
use shared::tables::table;

/// Reclaim every dependent resource for a deleted video: the social rows it owns and the media it
/// produced. Called once per soft-delete event (see `decider`).
pub async fn cleanup_video(state: &AppState, video_id: &str) -> Result<(), AppError> {
    tracing::info!(video_id, "reclaiming dependent data for deleted video");
    cleanup_social(&state.db, video_id).await?;
    cleanup_media(
        &state.s3,
        &state.db,
        video_id,
        &state.video_bucket,
        &state.raw_bucket,
    )
    .await?;
    // After the origin objects are gone, purge the CDN edge so the deleted video can't keep playing
    // from cache. Only when a distribution is configured (skipped locally / pre-`cdn`-stack). A
    // failure propagates so the message redrives — cleanup is idempotent and a persistent failure
    // dead-letters into the existing DLQ alarm.
    if let (Some(cf), Some(dist)) = (&state.cloudfront, &state.cdn_distribution_id) {
        crate::cdn::invalidate_video(cf, dist, video_id).await?;
    }
    tracing::info!(video_id, "cleanup complete");
    Ok(())
}

/// Delete the social-service rows for a video: comments, video likes/dislikes, comment reactions,
/// watch-history entries, and the stats counter. Each table is keyed (or GSI-keyed) by `video_id`,
/// so cleanup is a `Query(video_id) -> BatchDelete` (per the cascade schema).
pub async fn cleanup_social(db: &DynamoClient, video_id: &str) -> Result<(), AppError> {
    delete_by_video(
        db,
        &table("comments"),
        None,
        video_id,
        &["video_id", "comment_id"],
    )
    .await?;
    delete_by_video(
        db,
        &table("reactions"),
        None,
        video_id,
        &["video_id", "user_id"],
    )
    .await?;
    delete_by_video(
        db,
        &table("comment_reactions"),
        None,
        video_id,
        &["video_id", "sk"],
    )
    .await?;
    // view_history is PK=user_id; the `video-id-index` GSI (KEYS_ONLY) lets us find a video's entries.
    delete_by_video(
        db,
        &table("view_history"),
        Some("video-id-index"),
        video_id,
        &["user_id", "watched_at"],
    )
    .await?;
    // video_stats is a single item keyed by video_id.
    let mut key = HashMap::new();
    key.insert(
        "video_id".to_string(),
        AttributeValue::S(video_id.to_string()),
    );
    shared::dynamo::delete_item(db, &table("video_stats"), key).await?;
    Ok(())
}

/// Delete the media a video produced: the S3 objects under `hls/{id}/`, `mp4/{id}/`,
/// `thumbnails/{id}/` (videos bucket) and `raw/{id}/` (raw bucket), plus the transcode-job record
/// (found via its `video-id-index` GSI).
pub async fn cleanup_media(
    s3: &S3Client,
    db: &DynamoClient,
    video_id: &str,
    video_bucket: &str,
    raw_bucket: &str,
) -> Result<(), AppError> {
    for prefix in [
        format!("hls/{video_id}/"),
        format!("mp4/{video_id}/"),
        format!("thumbnails/{video_id}/"),
    ] {
        delete_prefix(s3, video_bucket, &prefix).await?;
    }
    delete_prefix(s3, raw_bucket, &format!("raw/{video_id}/")).await?;

    delete_by_video(
        db,
        &table("transcode_jobs"),
        Some("video-id-index"),
        video_id,
        &["job_id"],
    )
    .await?;
    Ok(())
}

/// Query a table (optionally via a GSI) for all items whose `video_id` matches, then batch-delete
/// them by the given primary-key attributes. Paginates the query so large result sets are fully
/// reclaimed (an incomplete delete would leave orphans — the exact bug the cascade fixes).
async fn delete_by_video(
    db: &DynamoClient,
    table_name: &str,
    index: Option<&str>,
    video_id: &str,
    key_attrs: &[&str],
) -> Result<(), AppError> {
    let items = query_all_by_video(db, table_name, index, video_id).await?;
    let keys: Vec<HashMap<String, AttributeValue>> = items
        .iter()
        .filter_map(|item| key_from(item, key_attrs))
        .collect();

    if !keys.is_empty() {
        let count = keys.len();
        shared::dynamo::batch_delete(db, table_name, keys).await?;
        tracing::info!(table = table_name, count, video_id, "deleted rows");
    }
    Ok(())
}

/// Paginated `Query` on `video_id` (the hash key of the base table or of the named GSI).
async fn query_all_by_video(
    db: &DynamoClient,
    table_name: &str,
    index: Option<&str>,
    video_id: &str,
) -> Result<Vec<HashMap<String, AttributeValue>>, AppError> {
    let mut items = Vec::new();
    let mut last_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let mut req = db
            .query()
            .table_name(table_name)
            .key_condition_expression("video_id = :v")
            .expression_attribute_values(":v", AttributeValue::S(video_id.to_string()))
            .set_exclusive_start_key(last_key.take());
        if let Some(idx) = index {
            req = req.index_name(idx);
        }

        let resp = req.send().await.map_err(AppError::internal)?;
        if let Some(batch) = resp.items {
            items.extend(batch);
        }
        match resp.last_evaluated_key {
            Some(k) if !k.is_empty() => last_key = Some(k),
            _ => break,
        }
    }
    Ok(items)
}

/// Build a primary-key map by copying the named attributes out of an item. Returns `None` if any
/// required attribute is missing (a malformed row is skipped rather than producing a bad key). Pure
/// + unit-tested.
fn key_from(
    item: &HashMap<String, AttributeValue>,
    attrs: &[&str],
) -> Option<HashMap<String, AttributeValue>> {
    let mut key = HashMap::new();
    for attr in attrs {
        key.insert((*attr).to_string(), item.get(*attr)?.clone());
    }
    Some(key)
}

/// Delete every object under `prefix` in `bucket`, paginating the listing. Each page holds at most
/// 1000 keys, which is exactly the `DeleteObjects` limit, so one delete call per page. Deleting an
/// empty prefix is a no-op.
async fn delete_prefix(s3: &S3Client, bucket: &str, prefix: &str) -> Result<(), AppError> {
    let mut token: Option<String> = None;
    loop {
        let mut req = s3.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(t) = token.take() {
            req = req.continuation_token(t);
        }
        let resp = req.send().await.map_err(AppError::internal)?;

        let objects: Vec<ObjectIdentifier> = resp
            .contents()
            .iter()
            .filter_map(|o| o.key())
            .map(|k| {
                ObjectIdentifier::builder()
                    .key(k)
                    .build()
                    .expect("object key is set")
            })
            .collect();

        if !objects.is_empty() {
            let count = objects.len();
            let delete = Delete::builder()
                .set_objects(Some(objects))
                .build()
                .expect("delete payload is valid");
            s3.delete_objects()
                .bucket(bucket)
                .delete(delete)
                .send()
                .await
                .map_err(AppError::internal)?;
            tracing::info!(bucket, prefix, count, "deleted s3 objects");
        }

        if resp.is_truncated() == Some(true) {
            token = resp.next_continuation_token().map(|s| s.to_string());
            if token.is_none() {
                break;
            }
        } else {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pairs: &[(&str, &str)]) -> HashMap<String, AttributeValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), AttributeValue::S(v.to_string())))
            .collect()
    }

    #[test]
    fn key_from_extracts_only_requested_attrs() {
        let it = item(&[
            ("video_id", "v1"),
            ("comment_id", "c1"),
            ("text", "ignored"),
            ("user_id", "u1"),
        ]);
        let key = key_from(&it, &["video_id", "comment_id"]).unwrap();
        assert_eq!(key.len(), 2);
        assert_eq!(key.get("video_id").unwrap().as_s().unwrap(), "v1");
        assert_eq!(key.get("comment_id").unwrap().as_s().unwrap(), "c1");
        assert!(!key.contains_key("text"));
    }

    #[test]
    fn key_from_composite_view_history_key() {
        // view_history GSI items project user_id + watched_at + video_id; we delete by base key.
        let it = item(&[
            ("user_id", "u1"),
            ("watched_at", "2026-06-15T00:00:00Z"),
            ("video_id", "v1"),
        ]);
        let key = key_from(&it, &["user_id", "watched_at"]).unwrap();
        assert_eq!(key.len(), 2);
        assert!(key.contains_key("user_id") && key.contains_key("watched_at"));
    }

    #[test]
    fn key_from_missing_attr_is_none() {
        let it = item(&[("video_id", "v1")]);
        assert!(key_from(&it, &["video_id", "comment_id"]).is_none());
    }
}
