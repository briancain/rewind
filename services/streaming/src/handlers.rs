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

/// Why a video has no servable asset yet — used only once we've established there's no manifest and
/// no playable S3 object. A not-yet-transcoded video must NOT return a bare 404: the watch page
/// polls right after upload, and a 404 there is indistinguishable from a genuinely missing video —
/// which is exactly what tripped the `video-playback-failing` alarm on normal post-upload views.
#[derive(Debug, PartialEq, Eq)]
pub enum Unservable {
    /// Still transcoding (draft/processing) — transient; the client should poll. -> 409.
    Processing,
    /// Transcode failed — terminal, but still not a "missing video" 404. -> 409.
    Failed,
    /// No such video, or a published video whose asset is genuinely gone. -> 404.
    Missing,
}

/// Classify why there's no servable asset, from the video's status (`None` = no row at all). Pure so
/// it unit-tests without DynamoDB.
pub fn unservable_reason(status: Option<VideoStatus>) -> Unservable {
    match status {
        Some(VideoStatus::Draft) | Some(VideoStatus::Processing) => Unservable::Processing,
        Some(VideoStatus::Failed) => Unservable::Failed,
        // Published-but-no-asset, or no row. (Deleted is handled earlier as NotFound.)
        _ => Unservable::Missing,
    }
}

impl Unservable {
    /// Map to an HTTP error. `missing_msg` is the 404 body for the genuinely-missing case (differs
    /// between stream-url and thumbnail-url). Processing/Failed become 409 so they stay out of the
    /// playback-404 metric.
    fn into_error(self, missing_msg: &str) -> AppError {
        match self {
            Unservable::Processing => AppError::Conflict("video is still processing".to_string()),
            Unservable::Failed => AppError::Conflict("video processing failed".to_string()),
            Unservable::Missing => AppError::NotFound(missing_msg.to_string()),
        }
    }
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
    let status = access.as_ref().map(|(_, _, status)| *status);

    let manifest_url = repo::get_video_manifest_url(&state.db, &video_id).await?;

    let url = match decide_delivery(visibility, manifest_url) {
        Delivery::Manifest(m) => m,
        Delivery::PresignMp4 => match repo::get_video_s3_key(&state.db, &video_id).await? {
            Some(s3_key) => repo::presign_get_url(&state.s3, &state.bucket, &s3_key).await?,
            // No manifest and no playable object: distinguish "still processing" (409, client
            // polls) from "genuinely missing" (404) so a normal post-upload poll isn't logged as a
            // playback failure.
            None => return Err(unservable_reason(status).into_error("video not found")),
        },
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
    let access = repo::get_video_access_info(&state.db, &video_id).await?;
    if let Some((visibility, channel_id, status)) = &access {
        if *status == VideoStatus::Deleted {
            return Err(AppError::NotFound("no thumbnail".to_string()));
        }
        if *visibility == Visibility::Private {
            let caller = shared::auth::authenticate(&state.db, &headers).await.ok();
            if caller.as_ref() != Some(channel_id) {
                return Err(AppError::Forbidden("this video is private".to_string()));
            }
        }
    }
    let status = access.as_ref().map(|(_, _, status)| *status);

    let thumb_key = match repo::get_video_thumbnail_key(&state.db, &video_id).await? {
        Some(k) => k,
        // Same not-ready-vs-missing distinction as stream_url: a thumbnail isn't generated until
        // transcode publishes, so a processing video's thumbnail poll must be a 409, not a 404.
        None => return Err(unservable_reason(status).into_error("no thumbnail")),
    };

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

    #[test]
    fn unservable_reason_treats_draft_and_processing_as_processing() {
        assert_eq!(
            super::unservable_reason(Some(shared::video::VideoStatus::Draft)),
            super::Unservable::Processing
        );
        assert_eq!(
            super::unservable_reason(Some(shared::video::VideoStatus::Processing)),
            super::Unservable::Processing
        );
    }

    #[test]
    fn unservable_reason_treats_failed_as_failed() {
        assert_eq!(
            super::unservable_reason(Some(shared::video::VideoStatus::Failed)),
            super::Unservable::Failed
        );
    }

    #[test]
    fn unservable_reason_treats_published_or_no_row_as_missing() {
        assert_eq!(
            super::unservable_reason(Some(shared::video::VideoStatus::Published)),
            super::Unservable::Missing
        );
        assert_eq!(super::unservable_reason(None), super::Unservable::Missing);
    }

    #[test]
    fn unservable_maps_processing_and_failed_to_409_and_missing_to_404() {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        assert_eq!(
            super::Unservable::Processing
                .into_error("video not found")
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            super::Unservable::Failed
                .into_error("video not found")
                .into_response()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            super::Unservable::Missing
                .into_error("video not found")
                .into_response()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
