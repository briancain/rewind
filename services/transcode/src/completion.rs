//! MediaConvert job-completion handling.
//!
//! MediaConvert emits a "MediaConvert Job State Change" event to EventBridge when a job finishes.
//! An EventBridge rule (source `aws.mediaconvert`, status COMPLETE/ERROR) forwards the event to an
//! SQS queue that this consumer drains. On COMPLETE we publish the video with the real CloudFront
//! manifest URL, the progressive MP4 key, the thumbnail key, and the duration — all read straight
//! from the event (no extra AWS calls, no ffprobe). On ERROR we mark the video `failed`.
//!
//! The consumer only runs when `completion_queue_url` is configured (cloud), so local dev
//! (DISABLE_MEDIACONVERT, no completions queue) never spawns it.

use crate::repo;
use crate::state::AppState;
use shared::error::AppError;
use shared::video::VideoStatus;

/// Outcome of parsing a MediaConvert Job State Change event.
#[derive(Debug, PartialEq)]
pub enum CompletionOutcome {
    /// Job finished successfully. Paths are `s3://...` URIs read from the event.
    Complete {
        video_id: String,
        manifest_s3: String,
        mp4_s3: Option<String>,
        thumbnail_s3: Option<String>,
        duration_secs: Option<f64>,
    },
    /// Job failed.
    Failed { video_id: String },
    /// Non-terminal status, missing correlation id, or unparseable — nothing to do.
    Ignored,
}

/// Map an `s3://bucket/key` URI to a CloudFront URL: `{cdn_base}/{key}`. `cdn_base` must not have a
/// trailing slash (e.g. `https://cdn.example.com`).
pub fn s3_to_cdn_url(s3_uri: &str, cdn_base: &str) -> String {
    format!("{}/{}", cdn_base.trim_end_matches('/'), s3_key_of(s3_uri))
}

/// Choose the poster frame from the frame-capture JPGs reported by the job. MediaConvert always
/// emits frame 0 first (`...thumb.0000000.jpg`, usually a black fade-in); the job is configured to
/// also capture a frame at ~25% (`...thumb.0000001.jpg`). We pick the candidate with the highest
/// numeric frame-index suffix, which is robust regardless of the order the event lists paths in.
/// With a single candidate (e.g. a very short clip that only produced frame 0) we return it.
pub fn select_poster_frame(jpg_uris: &[String]) -> Option<String> {
    jpg_uris
        .iter()
        .max_by_key(|uri| frame_index_of(uri))
        .cloned()
}

/// Parse the trailing frame index from a frame-capture filename such as `...thumb.0000001.jpg`.
/// Returns 0 when there is no numeric segment, so an unparseable name sorts as frame 0.
fn frame_index_of(uri: &str) -> u64 {
    let name = uri.rsplit('/').next().unwrap_or(uri);
    let stem = name
        .strip_suffix(".jpg")
        .or_else(|| name.strip_suffix(".jpeg"))
        .or_else(|| name.strip_suffix(".JPG"))
        .or_else(|| name.strip_suffix(".JPEG"))
        .unwrap_or(name);
    stem.rsplit('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Extract the object key from an `s3://bucket/key` URI (everything after the bucket). Returns the
/// input unchanged if it isn't an `s3://` URI.
pub fn s3_key_of(s3_uri: &str) -> String {
    s3_uri
        .strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_bucket, key)| key.to_string())
        .unwrap_or_else(|| s3_uri.to_string())
}

/// Parse a MediaConvert Job State Change event (the raw EventBridge JSON delivered to SQS).
pub fn parse_completion(event_json: &str) -> CompletionOutcome {
    let v: serde_json::Value = match serde_json::from_str(event_json) {
        Ok(v) => v,
        Err(_) => return CompletionOutcome::Ignored,
    };
    let detail = &v["detail"];
    let status = detail["status"].as_str().unwrap_or("");
    let video_id = detail["userMetadata"]["video_id"].as_str();

    match status {
        "ERROR" => match video_id {
            Some(id) => CompletionOutcome::Failed {
                video_id: id.to_string(),
            },
            None => CompletionOutcome::Ignored,
        },
        "COMPLETE" => {
            let Some(video_id) = video_id else {
                return CompletionOutcome::Ignored;
            };
            let mut manifest_s3: Option<String> = None;
            let mut mp4_s3: Option<String> = None;
            let mut jpg_paths: Vec<String> = Vec::new();
            let mut duration_secs: Option<f64> = None;

            if let Some(groups) = detail["outputGroupDetails"].as_array() {
                for group in groups {
                    // HLS group → the multivariant (master) playlist is the manifest we serve.
                    if group["type"].as_str() == Some("HLS_GROUP") {
                        if let Some(p) = group["playlistFilePaths"][0].as_str() {
                            manifest_s3 = Some(p.to_string());
                        }
                    }
                    // Scan every output for duration + the MP4/JPG file paths (by extension).
                    if let Some(outputs) = group["outputDetails"].as_array() {
                        for od in outputs {
                            if duration_secs.is_none() {
                                if let Some(ms) = od["durationInMs"].as_f64() {
                                    duration_secs = Some(ms / 1000.0);
                                }
                            }
                            if let Some(paths) = od["outputFilePaths"].as_array() {
                                for path in paths.iter().filter_map(|p| p.as_str()) {
                                    let lower = path.to_ascii_lowercase();
                                    if lower.ends_with(".mp4") {
                                        mp4_s3 = Some(path.to_string());
                                    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                                        jpg_paths.push(path.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            match manifest_s3 {
                Some(manifest_s3) => CompletionOutcome::Complete {
                    video_id: video_id.to_string(),
                    manifest_s3,
                    mp4_s3,
                    thumbnail_s3: select_poster_frame(&jpg_paths),
                    duration_secs,
                },
                // COMPLETE without an HLS manifest shouldn't happen for our job shape; ignore
                // defensively rather than publish an unplayable video.
                None => CompletionOutcome::Ignored,
            }
        }
        _ => CompletionOutcome::Ignored,
    }
}

/// Apply a parsed outcome to the videos table. `manifest_url` is stored as a CloudFront URL;
/// `s3_key` (the MP4) and `thumbnail_url` are stored as bare object keys (the streaming service
/// presigns/serves them).
pub async fn apply_outcome(state: &AppState, outcome: CompletionOutcome) -> Result<(), AppError> {
    match outcome {
        CompletionOutcome::Complete {
            video_id,
            manifest_s3,
            mp4_s3,
            thumbnail_s3,
            duration_secs,
        } => {
            let manifest_url = s3_to_cdn_url(&manifest_s3, &state.cdn_base_url);
            let mp4_key = mp4_s3.as_deref().map(s3_key_of);
            let thumb_key = thumbnail_s3.as_deref().map(s3_key_of);

            repo::update_video_status(
                &state.db,
                &video_id,
                VideoStatus::Published,
                Some(&manifest_url),
                thumb_key.as_deref(),
                mp4_key.as_deref(),
                duration_secs,
            )
            .await?;
            tracing::info!(
                video_id,
                manifest_url,
                "video published from MediaConvert completion"
            );
            Ok(())
        }
        CompletionOutcome::Failed { video_id } => {
            repo::update_video_status(
                &state.db,
                &video_id,
                VideoStatus::Failed,
                None,
                None,
                None,
                None,
            )
            .await?;
            tracing::error!(video_id, "MediaConvert job failed; video marked failed");
            Ok(())
        }
        CompletionOutcome::Ignored => Ok(()),
    }
}

/// Drain the completions queue, applying each event to the videos table. Mirrors the transcode-jobs
/// consumer loop. Only call this when `completion_queue_url` is set.
pub async fn run(state: AppState) {
    let queue_url = match &state.completion_queue_url {
        Some(url) => url.clone(),
        None => {
            tracing::info!("no completion queue configured; completion consumer not started");
            return;
        }
    };
    tracing::info!("starting MediaConvert completion consumer loop");

    loop {
        match poll_once(&state, &queue_url).await {
            Ok(n) if n > 0 => tracing::info!(count = n, "processed completion events"),
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "completion consumer error");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn poll_once(state: &AppState, queue_url: &str) -> Result<usize, AppError> {
    let resp = state
        .sqs
        .receive_message()
        .queue_url(queue_url)
        .max_number_of_messages(10)
        .wait_time_seconds(20)
        .send()
        .await
        .map_err(AppError::internal)?;

    let messages = resp.messages();
    let count = messages.len();

    for msg in messages {
        let body = msg.body().unwrap_or("{}");
        match parse_completion(body) {
            CompletionOutcome::Ignored => {
                // The EventBridge rule only forwards COMPLETE/ERROR, so an Ignored result here means
                // a terminal event we couldn't turn into an action (missing video_id/manifest or
                // unparseable). Log it — otherwise the video would sit in `processing` with no clue.
                tracing::warn!(body, "MediaConvert terminal event produced no action (missing video_id/manifest or unparseable)");
            }
            outcome => {
                if let Err(e) = apply_outcome(state, outcome).await {
                    tracing::error!(error = %e, "failed to apply completion; leaving message for retry");
                    continue; // don't delete — let SQS redeliver / DLQ
                }
            }
        }
        if let Some(receipt) = msg.receipt_handle() {
            let _ = state
                .sqs
                .delete_message()
                .queue_url(queue_url)
                .receipt_handle(receipt)
                .send()
                .await;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Based on the AWS "Apple HLS group" COMPLETE sample event, with userMetadata.video_id added
    // and MP4 + frame-capture file groups appended (matching our 3-output-group job).
    const COMPLETE_EVENT: &str = r#"{
      "detail-type": "MediaConvert Job State Change",
      "source": "aws.mediaconvert",
      "detail": {
        "status": "COMPLETE",
        "jobId": "1536964333549-opn151",
        "userMetadata": { "video_id": "vid-abc" },
        "outputGroupDetails": [
          {
            "type": "HLS_GROUP",
            "outputDetails": [
              { "outputFilePaths": ["s3://rewind-dev-videos-us-west-2/hls/vid-abc/clipv2.m3u8"], "durationInMs": 180041 },
              { "outputFilePaths": ["s3://rewind-dev-videos-us-west-2/hls/vid-abc/clipv1.m3u8"], "durationInMs": 180041 }
            ],
            "playlistFilePaths": ["s3://rewind-dev-videos-us-west-2/hls/vid-abc/clip.m3u8"]
          },
          {
            "type": "FILE_GROUP",
            "outputDetails": [
              { "outputFilePaths": ["s3://rewind-dev-videos-us-west-2/mp4/vid-abc/video.mp4"], "durationInMs": 180041 }
            ]
          },
          {
            "type": "FILE_GROUP",
            "outputDetails": [
              { "outputFilePaths": [
                  "s3://rewind-dev-videos-us-west-2/thumbnails/vid-abc/thumb.0000000.jpg",
                  "s3://rewind-dev-videos-us-west-2/thumbnails/vid-abc/thumb.0000001.jpg"
              ] }
            ]
          }
        ]
      }
    }"#;

    const ERROR_EVENT: &str = r#"{
      "detail-type": "MediaConvert Job State Change",
      "source": "aws.mediaconvert",
      "detail": { "status": "ERROR", "errorMessage": "boom", "userMetadata": { "video_id": "vid-err" } }
    }"#;

    const PROGRESSING_EVENT: &str = r#"{
      "detail": { "status": "PROGRESSING", "userMetadata": { "video_id": "vid-x" } }
    }"#;

    #[test]
    fn parses_complete_event() {
        match parse_completion(COMPLETE_EVENT) {
            CompletionOutcome::Complete {
                video_id,
                manifest_s3,
                mp4_s3,
                thumbnail_s3,
                duration_secs,
            } => {
                assert_eq!(video_id, "vid-abc");
                assert_eq!(
                    manifest_s3,
                    "s3://rewind-dev-videos-us-west-2/hls/vid-abc/clip.m3u8"
                );
                assert_eq!(
                    mp4_s3.as_deref(),
                    Some("s3://rewind-dev-videos-us-west-2/mp4/vid-abc/video.mp4")
                );
                assert_eq!(
                    thumbnail_s3.as_deref(),
                    // The ~25% capture (frame 1), not the black frame 0.
                    Some("s3://rewind-dev-videos-us-west-2/thumbnails/vid-abc/thumb.0000001.jpg")
                );
                assert_eq!(duration_secs, Some(180.041));
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn parses_error_event() {
        assert_eq!(
            parse_completion(ERROR_EVENT),
            CompletionOutcome::Failed {
                video_id: "vid-err".to_string()
            }
        );
    }

    #[test]
    fn ignores_non_terminal_status() {
        assert_eq!(
            parse_completion(PROGRESSING_EVENT),
            CompletionOutcome::Ignored
        );
    }

    #[test]
    fn ignores_malformed_json() {
        assert_eq!(parse_completion("not json"), CompletionOutcome::Ignored);
    }

    #[test]
    fn ignores_complete_without_video_id() {
        let ev = r#"{"detail":{"status":"COMPLETE","outputGroupDetails":[]}}"#;
        assert_eq!(parse_completion(ev), CompletionOutcome::Ignored);
    }

    #[test]
    fn maps_s3_uri_to_cdn_url() {
        assert_eq!(
            s3_to_cdn_url(
                "s3://rewind-dev-videos-us-west-2/hls/vid-abc/clip.m3u8",
                "https://cdn.watch.example.com"
            ),
            "https://cdn.watch.example.com/hls/vid-abc/clip.m3u8"
        );
    }

    #[test]
    fn extracts_s3_key() {
        assert_eq!(
            s3_key_of("s3://bkt/mp4/vid-abc/video.mp4"),
            "mp4/vid-abc/video.mp4"
        );
    }

    #[test]
    fn poster_frame_prefers_highest_index_regardless_of_order() {
        // Frame 0 is the black opener; frame 1 is the ~25% capture. Order must not matter.
        let uris = vec![
            "s3://b/thumbnails/v/clipthumb.0000001.jpg".to_string(),
            "s3://b/thumbnails/v/clipthumb.0000000.jpg".to_string(),
        ];
        assert_eq!(
            select_poster_frame(&uris).as_deref(),
            Some("s3://b/thumbnails/v/clipthumb.0000001.jpg")
        );
    }

    #[test]
    fn poster_frame_single_candidate_is_returned() {
        // A clip too short for a second capture still yields its only frame.
        let uris = vec!["s3://b/thumbnails/v/clipthumb.0000000.jpg".to_string()];
        assert_eq!(
            select_poster_frame(&uris).as_deref(),
            Some("s3://b/thumbnails/v/clipthumb.0000000.jpg")
        );
    }

    #[test]
    fn poster_frame_none_when_empty() {
        assert_eq!(select_poster_frame(&[]), None);
    }

    #[test]
    fn frame_index_parses_suffix_and_defaults_zero() {
        assert_eq!(frame_index_of("s3://b/v/thumb.0000012.jpg"), 12);
        assert_eq!(frame_index_of("s3://b/v/poster.jpeg"), 0); // no numeric segment
        assert_eq!(frame_index_of("weird-name"), 0);
    }
}
