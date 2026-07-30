//! CloudFront cache invalidation for a deleted video — the edge-side completion of the cascade.
//!
//! Deleting a video's S3 objects at origin is the durable correctness guarantee, but the content CDN
//! (`cdn.<domain>`, see DESIGN §10.3/§10.7) may still serve the deleted video from its edge caches —
//! and HLS segments are immutable with a long TTL, so without an explicit invalidation a deleted
//! video can remain playable from the edge for hours. After the worker deletes the origin objects it
//! invalidates the video's three CDN prefixes so "deleted" means gone at the edge too.
//!
//! Wiring follows the existing optional-capability pattern (cf. the consumer gated on
//! `CLEANUP_QUEUE_URL`): invalidation runs only when a distribution id is configured
//! (`CDN_DISTRIBUTION_ID`). It is absent locally (no CloudFront) and during a fresh bootstrap (the
//! `cdn` stack is applied after the first `deploy.sh`), in which case invalidation is skipped.
//!
//! The pure path builder is split from the AWS call so the path shapes are unit-testable without a
//! client.

use aws_sdk_cloudfront::types::{InvalidationBatch, Paths};
use aws_sdk_cloudfront::Client as CfClient;
use shared::error::AppError;

/// The CloudFront invalidation path patterns for a video's three edge-cached prefixes (`hls/`,
/// `mp4/`, `thumbnails/`, all under the videos bucket / `cdn.<domain>`). Leading-slash, trailing
/// `*` wildcard — the form `CreateInvalidation` expects to purge everything beneath each prefix.
/// Pure (no I/O) so the shapes are unit-tested independently of any client.
pub fn invalidation_paths(video_id: &str) -> Vec<String> {
    vec![
        format!("/hls/{video_id}/*"),
        format!("/mp4/{video_id}/*"),
        format!("/thumbnails/{video_id}/*"),
    ]
}

/// Invalidate a deleted video's CDN paths on `distribution_id`. Called after the origin S3 objects
/// are deleted (invalidating before deletion would let a request in the gap re-cache the live
/// object). A failure is propagated as an error so the SQS message redrives and retries — the worker
/// is idempotent (re-running the deletes is a no-op) and a persistent failure dead-letters into the
/// existing delete-cleanup DLQ alarm, so "gone at the edge" is actually guaranteed rather than
/// best-effort.
///
/// `CreateInvalidation` returns once CloudFront *accepts* the request (propagation to edges completes
/// shortly after) — that acceptance is the guarantee we need. The `CallerReference` carries a
/// timestamp so a redrive issues a fresh invalidation rather than colliding with the prior attempt.
pub async fn invalidate_video(
    cf: &CfClient,
    distribution_id: &str,
    video_id: &str,
) -> Result<(), AppError> {
    let paths = invalidation_paths(video_id);
    let quantity = paths.len() as i32;

    let paths_obj = Paths::builder()
        .quantity(quantity)
        .set_items(Some(paths))
        .build()
        .map_err(AppError::internal)?;

    let caller_reference = format!(
        "cleanup-{video_id}-{}",
        chrono::Utc::now().timestamp_millis()
    );

    let batch = InvalidationBatch::builder()
        .caller_reference(caller_reference)
        .paths(paths_obj)
        .build()
        .map_err(AppError::internal)?;

    cf.create_invalidation()
        .distribution_id(distribution_id)
        .invalidation_batch(batch)
        .send()
        .await
        .map_err(AppError::internal)?;

    tracing::info!(
        video_id,
        distribution_id,
        "issued CloudFront invalidation for deleted video"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_paths_covers_all_three_prefixes() {
        let paths = invalidation_paths("vid-123");
        assert_eq!(
            paths,
            vec![
                "/hls/vid-123/*".to_string(),
                "/mp4/vid-123/*".to_string(),
                "/thumbnails/vid-123/*".to_string(),
            ]
        );
    }

    #[test]
    fn invalidation_paths_are_rooted_and_wildcarded() {
        // Every path must be absolute (leading slash) and a prefix wildcard (trailing /*), which is
        // what CreateInvalidation needs to purge everything under the prefix.
        for p in invalidation_paths("abc") {
            assert!(p.starts_with('/'), "path {p} must be absolute");
            assert!(p.ends_with("/*"), "path {p} must be a prefix wildcard");
            assert!(p.contains("abc"), "path {p} must scope to the video id");
        }
    }
}
