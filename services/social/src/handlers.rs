use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use shared::error::AppError;

use crate::{
    models::{AddCommentRequest, CommentsResponse, ReactionResponse, StatsResponse},
    repo,
    state::AppState,
};

pub async fn like(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
) -> Result<Json<ReactionResponse>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let added = repo::toggle_reaction(&state.db, &video_id, &user_id, "like").await?;
    Ok(Json(ReactionResponse {
        action: if added { "added" } else { "removed" }.into(),
    }))
}

pub async fn dislike(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
) -> Result<Json<ReactionResponse>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let added = repo::toggle_reaction(&state.db, &video_id, &user_id, "dislike").await?;
    Ok(Json(ReactionResponse {
        action: if added { "added" } else { "removed" }.into(),
    }))
}

pub async fn add_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
    Json(req): Json<AddCommentRequest>,
) -> Result<(StatusCode, Json<crate::models::Comment>), AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let comment = repo::add_comment(&state.db, &video_id, &user_id, &req.text).await?;
    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn list_comments(
    State(state): State<AppState>,
    Path(video_id): Path<String>,
) -> Result<Json<CommentsResponse>, AppError> {
    let comments = repo::list_comments(&state.db, &video_id).await?;
    Ok(Json(CommentsResponse { comments }))
}

pub async fn record_view(
    State(state): State<AppState>,
    Path(video_id): Path<String>,
) -> StatusCode {
    let _ = repo::increment_views(&state.db, &video_id).await;
    StatusCode::NO_CONTENT
}

pub async fn get_stats(
    State(state): State<AppState>,
    Path(video_id): Path<String>,
) -> Result<Json<StatsResponse>, AppError> {
    let stats = repo::get_stats(&state.db, &video_id).await?;
    Ok(Json(stats))
}

pub async fn like_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((video_id, comment_id)): Path<(String, String)>,
) -> Result<Json<ReactionResponse>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let action =
        repo::toggle_comment_reaction(&state.db, &video_id, &comment_id, &user_id, "like").await?;
    Ok(Json(ReactionResponse { action }))
}

pub async fn dislike_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((video_id, comment_id)): Path<(String, String)>,
) -> Result<Json<ReactionResponse>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let action =
        repo::toggle_comment_reaction(&state.db, &video_id, &comment_id, &user_id, "dislike")
            .await?;
    Ok(Json(ReactionResponse { action }))
}

pub async fn delete_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((video_id, comment_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    repo::delete_comment(&state.db, &video_id, &comment_id, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn record_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(video_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    repo::record_view_history(&state.db, &user_id, &video_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::models::HistoryResponse>, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let entries = repo::get_view_history(&state.db, &user_id).await?;
    Ok(Json(crate::models::HistoryResponse { entries }))
}

pub async fn delete_history_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, AppError> {
    let user_id = shared::auth::authenticate(&state.db, &headers).await?;
    let watched_at = params
        .get("watched_at")
        .ok_or_else(|| AppError::BadRequest("missing watched_at".to_string()))?;
    repo::delete_view_history_entry(&state.db, &user_id, watched_at).await?;
    Ok(StatusCode::NO_CONTENT)
}
