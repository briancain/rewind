use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct TranscodeJob {
    pub video_id: String,
    pub s3_key: String,
    pub bucket: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcode_job_roundtrip() {
        let job = TranscodeJob {
            video_id: "v1".into(),
            s3_key: "raw/v1/video.mp4".into(),
            bucket: "rewind-raw".into(),
        };
        let json = serde_json::to_string(&job).unwrap();
        let parsed: TranscodeJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.video_id, "v1");
        assert_eq!(parsed.bucket, "rewind-raw");
    }
}
