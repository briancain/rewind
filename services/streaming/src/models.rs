use serde::Serialize;

#[derive(Serialize)]
pub struct StreamUrlResponse {
    pub video_id: String,
    pub url: String,
    pub expires_in_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_url_response_serializes() {
        let resp = StreamUrlResponse {
            video_id: "v1".into(),
            url: "https://cdn.example.com/v1/manifest.m3u8".into(),
            expires_in_secs: 3600,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"expires_in_secs\":3600"));
        assert!(json.contains("\"video_id\":\"v1\""));
    }
}
