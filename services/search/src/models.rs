use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct VideoDocument {
    pub video_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub results: Vec<VideoDocument>,
    pub total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_document_deserializes_minimal() {
        let json = r#"{"video_id":"v1","title":"Test"}"#;
        let doc: VideoDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.video_id, "v1");
        assert_eq!(doc.description, "");
        assert!(doc.tags.is_empty());
        // New fields default cleanly for backward compatibility with old indexed docs.
        assert_eq!(doc.genre, "");
        assert_eq!(doc.created_at, "");
    }

    #[test]
    fn video_document_full() {
        let json = r#"{"video_id":"v1","title":"Test","description":"desc","tags":["a","b"],"channel_id":"u1","genre":"tech","created_at":"2026-01-01T00:00:00Z"}"#;
        let doc: VideoDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.tags.len(), 2);
        assert_eq!(doc.channel_id, "u1");
        assert_eq!(doc.genre, "tech");
        assert_eq!(doc.created_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn video_document_roundtrip_preserves_new_fields() {
        let doc = VideoDocument {
            video_id: "v1".into(),
            title: "Title".into(),
            description: "Desc".into(),
            tags: vec!["a".into()],
            channel_id: "u1".into(),
            genre: "music".into(),
            created_at: "2026-06-11T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&doc).unwrap();
        let parsed: VideoDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, doc);
    }

    #[test]
    fn search_response_serializes() {
        let resp = SearchResponse {
            results: vec![],
            total: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"total\":0"));
    }
}
