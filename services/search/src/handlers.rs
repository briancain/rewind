use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use shared::error::AppError;

use crate::{
    models::{SearchResponse, VideoDocument},
    repo,
    state::AppState,
};

pub async fn index_video(
    State(state): State<AppState>,
    Json(doc): Json<VideoDocument>,
) -> Result<StatusCode, AppError> {
    repo::index_video(&state.client, &doc).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    let results = repo::search_videos(&state.client, &params.q).await?;
    Ok(Json(results))
}
