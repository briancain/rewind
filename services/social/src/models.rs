use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct StatsResponse {
    pub video_id: String,
    pub likes: i64,
    pub dislikes: i64,
    pub views: i64,
    pub comment_count: i64,
}

#[derive(Deserialize)]
pub struct AddCommentRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct Comment {
    pub comment_id: String,
    pub video_id: String,
    pub user_id: String,
    pub text: String,
    pub created_at: String,
    pub likes: i64,
}

#[derive(Serialize)]
pub struct CommentsResponse {
    pub comments: Vec<Comment>,
}

#[derive(Serialize)]
pub struct ReactionResponse {
    pub action: String, // "added" or "removed"
}

#[derive(Serialize)]
pub struct HistoryEntry {
    pub video_id: String,
    pub watched_at: String,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub entries: Vec<HistoryEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_comment_request_deserializes() {
        let json = r#"{"text":"Great video!"}"#;
        let req: AddCommentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.text, "Great video!");
    }

    #[test]
    fn stats_response_serializes() {
        let stats = StatsResponse {
            video_id: "v1".into(),
            likes: 10,
            dislikes: 2,
            views: 100,
            comment_count: 5,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"likes\":10"));
        assert!(json.contains("\"views\":100"));
    }

    #[test]
    fn comment_serializes() {
        let c = Comment {
            comment_id: "c1".into(),
            video_id: "v1".into(),
            user_id: "u1".into(),
            text: "hello".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            likes: 3,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"comment_id\":\"c1\""));
        assert!(json.contains("\"likes\":3"));
    }
}
