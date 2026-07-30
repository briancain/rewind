//! Out-of-band data the canary needs to seed (a one-time invite code, the per-run unlisted video,
//! and ephemeral-user teardown) plus the read-only verification probes that confirm the cascade
//! reclaimed everything. The canary seeds via DynamoDB directly (it has no "create invite" or
//! "publish a transcoded video" public API) and verifies via read-only DynamoDB/S3 queries — it
//! never hand-deletes dependent data, that is the product's cascade's job.

use std::collections::HashMap;

use aws_sdk_dynamodb::{types::AttributeValue, Client as DynamoClient};
use aws_sdk_s3::Client as S3Client;

use crate::assertions::CleanupReport;
use shared::tables::table;
use shared::video::{VideoStatus, Visibility};

fn s(v: &str) -> AttributeValue {
    AttributeValue::S(v.to_string())
}

/// Seed a fresh, unused invite code so an ephemeral user can register. Mirrors the item shape in
/// `scripts/invite.sh` (`code`, `created_at`, `used=false`).
pub async fn seed_invite_code(db: &DynamoClient, code: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut item = HashMap::new();
    item.insert("code".to_string(), s(code));
    item.insert("created_at".to_string(), s(&now));
    item.insert("used".to_string(), AttributeValue::Bool(false));
    shared::dynamo::put_item(db, &table("invite_codes"), item)
        .await
        .map_err(|e| format!("seed invite code: {e}"))
}

/// Seed the per-run video as **published + unlisted** with a `manifest_url`, owned by `owner_id`.
/// Unlisted ⇒ it never appears in the public feed or the search index, so it doesn't pollute the
/// live demo; the `manifest_url` makes the streaming service return a URL (it echoes the manifest
/// for unlisted videos, so no real transcode is needed). `channel_id == owner_id` is required for
/// the catalog `DELETE /videos` owner check to pass.
pub async fn seed_unlisted_video(
    db: &DynamoClient,
    video_id: &str,
    owner_id: &str,
    title: &str,
    manifest_url: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut item = HashMap::new();
    item.insert("video_id".to_string(), s(video_id));
    item.insert("channel_id".to_string(), s(owner_id));
    item.insert("title".to_string(), s(title));
    item.insert(
        "description".to_string(),
        s("Ephemeral canary video — created and cascade-deleted each run."),
    );
    item.insert("genre".to_string(), s("canary"));
    item.insert(
        "tags".to_string(),
        AttributeValue::L(vec![s("canary"), s("automated")]),
    );
    item.insert("status".to_string(), s(VideoStatus::Published.as_str()));
    item.insert("visibility".to_string(), s(Visibility::Unlisted.as_str()));
    item.insert("manifest_url".to_string(), s(manifest_url));
    item.insert("created_at".to_string(), s(&now));
    item.insert("updated_at".to_string(), s(&now));
    shared::dynamo::put_item(db, &table("videos"), item)
        .await
        .map_err(|e| format!("seed unlisted video: {e}"))
}

/// Best-effort deletion of the ephemeral auth user's `users` row at the end of a run. (Their
/// session is removed via the product's `POST /logout`.)
pub async fn delete_user(db: &DynamoClient, user_id: &str) -> Result<(), String> {
    let mut key = HashMap::new();
    key.insert("user_id".to_string(), s(user_id));
    shared::dynamo::delete_item(db, &table("users"), key)
        .await
        .map_err(|e| format!("delete ephemeral user: {e}"))
}

/// Count items on a base table whose hash key `video_id` matches (comments, reactions,
/// comment_reactions). Paginates so a large set is fully counted.
async fn count_base(db: &DynamoClient, table_name: &str, video_id: &str) -> Result<usize, String> {
    count_query(db, table_name, None, video_id).await
}

/// Count items found via a GSI keyed on `video_id` (view_history, transcode_jobs).
async fn count_index(
    db: &DynamoClient,
    table_name: &str,
    index: &str,
    video_id: &str,
) -> Result<usize, String> {
    count_query(db, table_name, Some(index), video_id).await
}

async fn count_query(
    db: &DynamoClient,
    table_name: &str,
    index: Option<&str>,
    video_id: &str,
) -> Result<usize, String> {
    let mut total = 0usize;
    let mut last_key: Option<HashMap<String, AttributeValue>> = None;
    loop {
        let mut req = db
            .query()
            .table_name(table_name)
            .key_condition_expression("video_id = :v")
            .expression_attribute_values(":v", s(video_id))
            .select(aws_sdk_dynamodb::types::Select::Count)
            .set_exclusive_start_key(last_key.take());
        if let Some(idx) = index {
            req = req.index_name(idx);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("count {table_name}: {e}"))?;
        total += resp.count() as usize;
        match resp.last_evaluated_key {
            Some(k) if !k.is_empty() => last_key = Some(k),
            _ => break,
        }
    }
    Ok(total)
}

/// Whether a `video_stats` row exists for the video.
async fn stats_present(db: &DynamoClient, video_id: &str) -> Result<bool, String> {
    let mut key = HashMap::new();
    key.insert("video_id".to_string(), s(video_id));
    let item = shared::dynamo::get_item(db, &table("video_stats"), key)
        .await
        .map_err(|e| format!("get video_stats: {e}"))?;
    Ok(item.is_some())
}

/// Count objects under an S3 prefix (paginated). The canary seeds no S3 objects (its IRSA role is
/// read-only `ListBucket`), so for the seeded video this is trivially 0 — it still guards against a
/// future upload-lifecycle canary leaving media behind.
async fn count_s3_prefix(s3: &S3Client, bucket: &str, prefix: &str) -> Result<usize, String> {
    let mut total = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut req = s3.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(t) = token.take() {
            req = req.continuation_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("list {bucket}/{prefix}: {e}"))?;
        total += resp.key_count().unwrap_or(0) as usize;
        if resp.is_truncated() == Some(true) {
            token = resp.next_continuation_token().map(|s| s.to_string());
            if token.is_none() {
                break;
            }
        } else {
            break;
        }
    }
    Ok(total)
}

/// Probe every store that the cascade is responsible for clearing, producing a [`CleanupReport`]
/// the caller polls until [`CleanupReport::is_clean`].
pub async fn probe_cleanup(
    db: &DynamoClient,
    s3: &S3Client,
    video_id: &str,
    video_bucket: &str,
    raw_bucket: &str,
) -> Result<CleanupReport, String> {
    let videos_objects = count_s3_prefix(s3, video_bucket, &format!("hls/{video_id}/")).await?
        + count_s3_prefix(s3, video_bucket, &format!("mp4/{video_id}/")).await?
        + count_s3_prefix(s3, video_bucket, &format!("thumbnails/{video_id}/")).await?;

    Ok(CleanupReport {
        comments: count_base(db, &table("comments"), video_id).await?,
        reactions: count_base(db, &table("reactions"), video_id).await?,
        comment_reactions: count_base(db, &table("comment_reactions"), video_id).await?,
        view_history: count_index(db, &table("view_history"), "video-id-index", video_id).await?,
        transcode_jobs: count_index(db, &table("transcode_jobs"), "video-id-index", video_id)
            .await?,
        stats_present: stats_present(db, video_id).await?,
        videos_objects,
        raw_objects: count_s3_prefix(s3, raw_bucket, &format!("raw/{video_id}/")).await?,
    })
}
