use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use shared::error::AppError;
use std::collections::HashMap;

use crate::models::*;
use crate::repo;
use crate::state::AppState;

pub async fn create_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateVideoRequest>,
) -> Result<(StatusCode, Json<Video>), AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;

    if req.title.trim().is_empty() {
        return Err(AppError::BadRequest("title is required".to_string()));
    }

    let video = repo::create_video(
        &state.db,
        &user_id,
        &req.title,
        &req.description,
        &req.genre,
        &req.tags,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(video)))
}

pub async fn get_video(
    State(state): State<AppState>,
    Path(video_id): Path<String>,
) -> Result<Json<Video>, AppError> {
    let video = repo::get_video(&state.db, &video_id).await?;
    Ok(Json(video))
}

pub async fn list_videos(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<VideoList>, AppError> {
    let mut videos = if let Some(channel_id) = params.get("channel_id") {
        // `channel_id` is the channel-index key condition — an empty value is a client error, not a
        // server fault (`?channel_id=` otherwise reached DynamoDB and became a 500).
        shared::validate::key_field("channel_id", channel_id)?;
        repo::list_by_channel(&state.db, channel_id).await?
    } else {
        repo::list_feed(&state.db).await?
    };

    // Filter out private videos unless the requester is the owner
    let caller = shared::auth::authenticate(&state.db, &headers).await.ok();
    videos.retain(|v| {
        v.visibility != crate::models::Visibility::Private || caller.as_ref() == Some(&v.channel_id)
    });

    Ok(Json(VideoList { videos }))
}

pub async fn feed(State(state): State<AppState>) -> Result<Json<VideoList>, AppError> {
    let videos = repo::list_feed(&state.db).await?;
    Ok(Json(VideoList { videos }))
}

pub async fn update_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
    Json(req): Json<UpdateVideoRequest>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let video = repo::get_video(&state.db, &video_id).await?;

    if video.channel_id != user_id {
        return Err(AppError::Forbidden("not the video owner".to_string()));
    }

    repo::update_video(
        &state.db,
        &video_id,
        req.title.as_deref(),
        req.description.as_deref(),
        req.genre.as_deref(),
        req.tags.as_deref(),
        req.visibility.as_ref(),
    )
    .await?;

    // Search index stays in sync automatically: the videos DynamoDB stream feeds the search
    // service's consumer (EventBridge Pipe -> SQS FIFO), which re-indexes or removes the document
    // based on the new status/visibility.

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
    Json(req): Json<UpdateStatusRequest>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let video = repo::get_video(&state.db, &video_id).await?;

    if video.channel_id != user_id {
        return Err(AppError::Forbidden("not the video owner".to_string()));
    }

    // Only the owner-settable statuses are allowed here; processing/failed/deleted are system-set.
    let status = match req.status.parse::<VideoStatus>() {
        Ok(s @ (VideoStatus::Draft | VideoStatus::Published)) => s,
        _ => return Err(AppError::BadRequest("invalid status".to_string())),
    };

    repo::update_status(&state.db, &video_id, status).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn surf(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Video>, AppError> {
    let seed: u64 = params.get("seed").and_then(|s| s.parse().ok()).unwrap_or(0);
    let offset: usize = params
        .get("offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut ids = repo::get_published_video_ids(&state.db).await?;

    if ids.is_empty() {
        return Err(AppError::NotFound("no videos available".to_string()));
    }

    // Deterministic shuffle using seed
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    ids.shuffle(&mut rng);

    let idx = offset % ids.len();
    let video = repo::get_video(&state.db, &ids[idx]).await?;
    Ok(Json(video))
}

pub async fn delete_video(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(video_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let video = repo::get_video(&state.db, &video_id).await?;
    if video.channel_id != user_id {
        return Err(AppError::Forbidden("not your video".to_string()));
    }
    repo::delete_video(&state.db, &video_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
