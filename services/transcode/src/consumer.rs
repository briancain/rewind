use crate::models::TranscodeJob;
use crate::repo;
use crate::state::AppState;
use shared::error::AppError;
use shared::video::VideoStatus;

pub async fn run(state: AppState) {
    tracing::info!("starting SQS consumer loop");

    loop {
        match poll_and_process(&state).await {
            Ok(count) => {
                if count > 0 {
                    tracing::info!(count, "processed messages");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "consumer error");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn poll_and_process(state: &AppState) -> Result<usize, AppError> {
    let resp = state
        .sqs
        .receive_message()
        .queue_url(&state.queue_url)
        .max_number_of_messages(10)
        .wait_time_seconds(20)
        .send()
        .await
        .map_err(AppError::internal)?;

    let messages = resp.messages();
    let count = messages.len();

    for msg in messages {
        let body = msg.body().unwrap_or("{}");
        let receipt = msg.receipt_handle().unwrap_or_default().to_string();

        match serde_json::from_str::<TranscodeJob>(body) {
            Ok(job) => {
                if let Err(e) = process_job(state, &job).await {
                    tracing::error!(video_id = %job.video_id, error = %e, "failed to process job");
                    continue;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, body, "failed to parse message");
            }
        }

        let _ = state
            .sqs
            .delete_message()
            .queue_url(&state.queue_url)
            .receipt_handle(&receipt)
            .send()
            .await;
    }

    Ok(count)
}

async fn process_job(state: &AppState, job: &TranscodeJob) -> Result<(), AppError> {
    tracing::info!(video_id = %job.video_id, "processing transcode job");

    repo::update_video_status(
        &state.db,
        &job.video_id,
        VideoStatus::Processing,
        None,
        None,
        None,
        None,
    )
    .await?;

    if let Some(mc) = &state.mediaconvert {
        // Cloud path: submit a MediaConvert job and stop here. The video stays `processing`; the
        // completion consumer publishes it when MediaConvert emits COMPLETE, or marks it `failed`
        // on ERROR.
        match repo::submit_mediaconvert_job(mc, job, &state.output_bucket, &state.mediaconvert_role)
            .await
        {
            Ok(mc_job_id) => {
                tracing::info!(video_id = %job.video_id, mc_job_id, "MediaConvert job submitted; awaiting completion event");
                return Ok(());
            }
            Err(e) => {
                // Don't leave the video stuck in `processing`. Mark it `failed` so the UI reflects
                // the failure. We still return Err so SQS retries (transient errors) and eventually
                // dead-letters (poison jobs); a later successful submit republishes via COMPLETE.
                if let Err(mark) = repo::update_video_status(
                    &state.db,
                    &job.video_id,
                    VideoStatus::Failed,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                {
                    tracing::error!(video_id = %job.video_id, error = %mark, "failed to mark video failed after submit error");
                }
                return Err(e);
            }
        }
    }

    // Local/dev fallback (DISABLE_MEDIACONVERT): copy the raw file into the videos bucket so the
    // streaming service can serve it as progressive MP4, extract a thumbnail + duration with ffmpeg,
    // and publish immediately. No real transcode happens in this path.
    let dest_key = format!("videos/{}/video.mp4", job.video_id);

    state
        .s3
        .copy_object()
        .copy_source(format!("{}/{}", job.bucket, job.s3_key))
        .bucket(&state.output_bucket)
        .key(&dest_key)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("s3 copy failed: {e}")))?;

    tracing::info!(video_id = %job.video_id, dest_key, "copied raw file to videos bucket");

    // Extract thumbnail with ffmpeg
    let thumb_key = format!("thumbnails/{}/thumb.jpg", job.video_id);
    if let Err(e) = extract_and_upload_thumbnail(state, job, &thumb_key).await {
        tracing::warn!(video_id = %job.video_id, error = %e, "thumbnail extraction failed, continuing without");
    }

    // Get video duration with ffprobe
    let duration = get_video_duration(state, job).await;

    repo::update_video_status(
        &state.db,
        &job.video_id,
        VideoStatus::Published,
        None,
        Some(&thumb_key),
        Some(&dest_key),
        duration,
    )
    .await?;

    // Index in search if SEARCH_ENDPOINT is set (local dev only; prod uses DDB Streams)
    if let Ok(search_url) = std::env::var("SEARCH_ENDPOINT") {
        let video = shared::dynamo::get_item(&state.db, &shared::tables::table("videos"), {
            let mut k = std::collections::HashMap::new();
            k.insert(
                "video_id".into(),
                aws_sdk_dynamodb::types::AttributeValue::S(job.video_id.clone()),
            );
            k
        })
        .await
        .ok()
        .flatten();

        if let Some(item) = video {
            let visibility = item
                .get("visibility")
                .and_then(|v| v.as_s().ok())
                .unwrap_or(&"public".to_string())
                .clone();
            if visibility == "public" {
                let doc = serde_json::json!({
                    "video_id": job.video_id,
                    "title": item.get("title").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                    "description": item.get("description").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                    "channel_id": item.get("channel_id").and_then(|v| v.as_s().ok()).unwrap_or(&String::new()),
                    "tags": item.get("tags").and_then(|v| v.as_l().ok()).map(|l| l.iter().filter_map(|v| v.as_s().ok()).collect::<Vec<_>>()).unwrap_or_default(),
                });
                let _ = reqwest::Client::new()
                    .post(format!("{}/index", search_url))
                    .json(&doc)
                    .send()
                    .await;
            }
        }
    }

    tracing::info!(video_id = %job.video_id, "transcode complete");
    Ok(())
}

async fn extract_and_upload_thumbnail(
    state: &AppState,
    job: &TranscodeJob,
    thumb_key: &str,
) -> Result<(), AppError> {
    let tmp_dir = std::env::temp_dir().join(format!("rewind-{}", job.video_id));
    std::fs::create_dir_all(&tmp_dir).map_err(AppError::internal)?;
    let video_path = tmp_dir.join("input.mp4");
    let thumb_path = tmp_dir.join("thumb.jpg");

    // Download raw file from S3
    let resp = state
        .s3
        .get_object()
        .bucket(&job.bucket)
        .key(&job.s3_key)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("s3 get: {e}")))?;

    let bytes = resp
        .body
        .collect()
        .await
        .map_err(|e| AppError::Internal(format!("read body: {e}")))?
        .into_bytes();
    std::fs::write(&video_path, &bytes).map_err(AppError::internal)?;

    // Extract frame at 25% with ffmpeg
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-i",
            video_path.to_str().unwrap(),
            "-ss",
            "25%",
            "-frames:v",
            "1",
            "-q:v",
            "2",
            "-y",
            thumb_path.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| AppError::Internal(format!("ffmpeg exec: {e}")))?;

    // ffmpeg doesn't support percentage for -ss, use a fixed 2s fallback
    if !thumb_path.exists() {
        std::process::Command::new("ffmpeg")
            .args([
                "-i",
                video_path.to_str().unwrap(),
                "-ss",
                "2",
                "-frames:v",
                "1",
                "-q:v",
                "2",
                "-y",
                thumb_path.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| AppError::Internal(format!("ffmpeg fallback: {e}")))?;
    }

    if !thumb_path.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(AppError::Internal(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    // Upload thumbnail to S3
    let thumb_bytes = std::fs::read(&thumb_path).map_err(AppError::internal)?;
    state
        .s3
        .put_object()
        .bucket(&state.output_bucket)
        .key(thumb_key)
        .body(thumb_bytes.into())
        .content_type("image/jpeg")
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("s3 put thumb: {e}")))?;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    tracing::info!(video_id = %job.video_id, "thumbnail extracted");
    Ok(())
}

async fn get_video_duration(state: &AppState, job: &TranscodeJob) -> Option<f64> {
    let tmp_dir = std::env::temp_dir().join(format!("dur-{}", job.video_id));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let video_path = tmp_dir.join("video.mp4");

    let obj = state
        .s3
        .get_object()
        .bucket(&job.bucket)
        .key(&job.s3_key)
        .send()
        .await
        .ok()?;
    let bytes = obj.body.collect().await.ok()?.into_bytes();
    std::fs::write(&video_path, &bytes).ok()?;

    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            video_path.to_str().unwrap(),
        ])
        .output()
        .ok()?;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().parse::<f64>().ok()
}
