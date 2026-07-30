use serde::{Deserialize, Serialize};

// Video-domain value types live in the shared crate so every service shares one definition.
// Re-exported here so existing `crate::models::{VideoStatus, Visibility}` references keep resolving.
pub use shared::video::{VideoStatus, Visibility};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Video {
    pub video_id: String,
    pub channel_id: String,
    pub title: String,
    pub description: String,
    pub genre: String,
    pub tags: Vec<String>,
    pub status: VideoStatus, // draft/processing/published/failed (processing/failed are system-set by transcode); deleted = tombstone
    pub visibility: Visibility,
    pub thumbnail_url: Option<String>,
    pub manifest_url: Option<String>,
    pub duration_seconds: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateVideoRequest {
    pub title: String,
    pub description: String,
    pub genre: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateVideoRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub genre: Option<String>,
    pub tags: Option<Vec<String>>,
    pub visibility: Option<Visibility>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct VideoList {
    pub videos: Vec<Video>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_video_request_deserializes() {
        let json = r#"{"title":"Test","description":"desc","genre":"tech","tags":["rust"]}"#;
        let req: CreateVideoRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.title, "Test");
        assert_eq!(req.tags, vec!["rust"]);
    }

    #[test]
    fn create_video_request_default_tags() {
        let json = r#"{"title":"Test","description":"","genre":"tech"}"#;
        let req: CreateVideoRequest = serde_json::from_str(json).unwrap();
        assert!(req.tags.is_empty());
    }

    #[test]
    fn update_video_request_all_optional() {
        let json = r#"{}"#;
        let req: UpdateVideoRequest = serde_json::from_str(json).unwrap();
        assert!(req.title.is_none());
        assert!(req.description.is_none());
        assert!(req.genre.is_none());
        assert!(req.tags.is_none());
    }

    #[test]
    fn video_serialization_roundtrip() {
        let video = Video {
            video_id: "v1".into(),
            channel_id: "u1".into(),
            title: "Title".into(),
            description: "Desc".into(),
            genre: "tech".into(),
            tags: vec!["a".into()],
            status: VideoStatus::Published,
            visibility: Visibility::Public,
            thumbnail_url: None,
            manifest_url: None,
            duration_seconds: Some(120.5),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&video).unwrap();
        let parsed: Video = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.video_id, "v1");
        assert_eq!(parsed.duration_seconds, Some(120.5));
    }
}
