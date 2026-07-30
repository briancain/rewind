use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct InitiateRequest {
    pub video_id: String,
    pub filename: String,
    pub content_type: String,
    pub part_count: u32,
}

#[derive(Debug, Serialize)]
pub struct InitiateResponse {
    pub upload_id: String,
    pub s3_key: String,
    pub presigned_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub video_id: String,
    pub upload_id: String,
    pub s3_key: String,
    pub parts: Vec<CompletedPart>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompletedPart {
    pub part_number: i32,
    pub etag: String,
}

#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    pub message: String,
    pub s3_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initiate_request_deserializes() {
        let json =
            r#"{"video_id":"v1","filename":"test.mp4","content_type":"video/mp4","part_count":3}"#;
        let req: InitiateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.video_id, "v1");
        assert_eq!(req.part_count, 3);
    }

    #[test]
    fn complete_request_with_parts() {
        let json = r#"{"video_id":"v1","upload_id":"u1","s3_key":"raw/v1/test.mp4","parts":[{"part_number":1,"etag":"abc"}]}"#;
        let req: CompleteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.parts.len(), 1);
        assert_eq!(req.parts[0].part_number, 1);
        assert_eq!(req.parts[0].etag, "abc");
    }
}
