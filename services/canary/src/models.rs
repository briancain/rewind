//! Minimal response DTOs the canary deserializes from the services. These intentionally mirror only
//! the fields the canary asserts on (the services own the authoritative shapes), so the canary is
//! resilient to additive response changes.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Profile {
    pub user_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct VideoView {
    pub video_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub visibility: String,
}

#[derive(Debug, Deserialize)]
pub struct Feed {
    pub videos: Vec<VideoView>,
}

#[derive(Debug, Deserialize)]
pub struct StreamUrl {
    #[serde(default)]
    pub video_id: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct CommentResponse {
    pub comment_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ReactionResponse {
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub likes: i64,
    #[serde(default)]
    pub dislikes: i64,
    #[serde(default)]
    pub views: i64,
    #[serde(default)]
    pub comment_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub video_id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    #[serde(default)]
    pub total: u64,
}
