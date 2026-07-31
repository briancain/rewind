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

// No parts array: the completed-parts list is assembled server-side via S3 ListParts, keeping the
// body tiny (a large file's per-part array used to exceed the ALB WAF 8 KB body limit).
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub video_id: String,
    pub upload_id: String,
    pub s3_key: String,
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
    fn complete_request_deserializes() {
        let json = r#"{"video_id":"v1","upload_id":"u1","s3_key":"raw/v1/test.mp4"}"#;
        let req: CompleteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.video_id, "v1");
        assert_eq!(req.upload_id, "u1");
        assert_eq!(req.s3_key, "raw/v1/test.mp4");
    }
}
