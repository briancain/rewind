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

/// Search query parameters. Both are optional: `tag` drives the exact-tag filter (the clickable
/// hashtag path) and takes precedence; otherwise `q` is a free-text search. Both default to empty,
/// so a missing/blank param yields an empty result rather than a 4xx — an empty search is not a
/// client error.
#[derive(Deserialize, Default)]
pub struct SearchParams {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    // Tag mode takes precedence: an exact, newest-first filter on the tag.
    if let Some(tag) = params
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let results = repo::search_by_tag(&state.client, tag).await?;
        return Ok(Json(results));
    }

    let q = params.q.as_deref().unwrap_or_default().trim();
    let results = repo::search_videos(&state.client, q).await?;
    Ok(Json(results))
}
