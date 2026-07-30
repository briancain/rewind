use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use shared::error::AppError;

use crate::{models::StreamUrlResponse, repo, state::AppState};
use shared::video::{VideoStatus, Visibility};

/// How to deliver a video's bytes to the player.
#[derive(Debug, PartialEq)]
pub enum Delivery {
    /// Serve this URL directly (a CloudFront HLS manifest). The CDN gates nothing, so this is only
    /// used for public/unlisted videos.
    Manifest(String),
    /// Presign the progressive MP4 from S3. Used for private videos (owner-only, gated by this
    /// service before the short-lived URL is issued) and as the legacy/local fallback.
    PresignMp4,
}

/// Decide delivery from a video's visibility and whether it has an HLS manifest.
/// - private               -> always PresignMp4 (secure: the URL is issued only after the owner
///   check, and is short-lived — CloudFront would expose a stable, guessable path).
/// - public/unlisted + HLS -> Manifest (CloudFront).
/// - otherwise             -> PresignMp4 (legacy progressive uploads, local dev).
pub fn decide_delivery(visibility: Visibility, manifest_url: Option<String>) -> Delivery {
    if visibility != Visibility::Private {
        if let Some(m) = manifest_url {
            return Delivery::Manifest(m);
        }
    }
    Delivery::PresignMp4
}

pub async fn stream_url(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
) -> Result<Json<StreamUrlResponse>, AppError> {
    let access = repo::get_video_access_info(&state.db, &video_id).await?;

    // Enforce access control. A soft-deleted video is a tombstone — treat it as not found.
    let visibility = match &access {
        Some((visibility, channel_id, status)) => {
            if *status == VideoStatus::Deleted {
                return Err(AppError::NotFound("video not found".to_string()));
            }
            if *visibility == Visibility::Private {
                let caller = shared::auth::authenticate(&state.db, &headers).await.ok();
                if caller.as_ref() != Some(channel_id) {
                    return Err(AppError::Forbidden("this video is private".to_string()));
                }
            }
            *visibility
        }
        None => Visibility::Public,
    };

    let manifest_url = repo::get_video_manifest_url(&state.db, &video_id).await?;

    let url = match decide_delivery(visibility, manifest_url) {
        Delivery::Manifest(m) => m,
        Delivery::PresignMp4 => {
            let s3_key = repo::get_video_s3_key(&state.db, &video_id)
                .await?
                .ok_or_else(|| AppError::NotFound("video not found".to_string()))?;
            repo::presign_get_url(&state.s3, &state.bucket, &s3_key).await?
        }
    };

    Ok(Json(StreamUrlResponse {
        video_id,
        url,
        expires_in_secs: repo::expiry_secs(),
    }))
}

pub async fn thumbnail_url(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
) -> Result<Json<StreamUrlResponse>, AppError> {
    // Check visibility for private videos
    if let Some((visibility, channel_id, status)) =
        repo::get_video_access_info(&state.db, &video_id).await?
    {
        if status == VideoStatus::Deleted {
            return Err(AppError::NotFound("no thumbnail".to_string()));
        }
        if visibility == Visibility::Private {
            let caller = shared::auth::authenticate(&state.db, &headers).await.ok();
            if caller.as_ref() != Some(&channel_id) {
                return Err(AppError::Forbidden("this video is private".to_string()));
            }
        }
    }

    let thumb_key = repo::get_video_thumbnail_key(&state.db, &video_id)
        .await?
        .ok_or_else(|| AppError::NotFound("no thumbnail".to_string()))?;

    let url = repo::presign_get_url(&state.s3, &state.bucket, &thumb_key).await?;

    Ok(Json(StreamUrlResponse {
        video_id,
        url,
        expires_in_secs: repo::expiry_secs(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{decide_delivery, Delivery};
    use shared::video::Visibility;

    #[test]
    fn private_always_presigns_mp4_even_with_manifest() {
        // Private must never be served from the open CDN, even if an HLS manifest exists.
        assert_eq!(
            decide_delivery(
                Visibility::Private,
                Some("https://cdn/hls/v/clip.m3u8".into())
            ),
            Delivery::PresignMp4
        );
    }

    #[test]
    fn public_with_manifest_uses_cloudfront() {
        assert_eq!(
            decide_delivery(
                Visibility::Public,
                Some("https://cdn/hls/v/clip.m3u8".into())
            ),
            Delivery::Manifest("https://cdn/hls/v/clip.m3u8".into())
        );
    }

    #[test]
    fn unlisted_with_manifest_uses_cloudfront() {
        assert_eq!(
            decide_delivery(
                Visibility::Unlisted,
                Some("https://cdn/hls/v/clip.m3u8".into())
            ),
            Delivery::Manifest("https://cdn/hls/v/clip.m3u8".into())
        );
    }

    #[test]
    fn public_without_manifest_falls_back_to_presign() {
        // Legacy progressive uploads + local dev have no manifest.
        assert_eq!(
            decide_delivery(Visibility::Public, None),
            Delivery::PresignMp4
        );
    }
}
