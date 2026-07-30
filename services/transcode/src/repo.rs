use aws_sdk_dynamodb::{types::AttributeValue, Client as DynamoClient};
use aws_sdk_mediaconvert::error::ProvideErrorMetadata;
use aws_sdk_mediaconvert::types::{
    AacCodingMode, AacSettings, AudioCodec, AudioCodecSettings, AudioDefaultSelection,
    AudioDescription, AudioSelector, AutomatedAbrSettings, AutomatedEncodingSettings,
    ContainerSettings, ContainerType, FileGroupSettings, FrameCaptureSettings,
    H264QualityTuningLevel, H264RateControlMode, H264Settings, HlsGroupSettings, Input,
    JobSettings, M3u8Settings, Mp4Settings, Output, OutputGroup, OutputGroupSettings,
    OutputGroupType, VideoCodec, VideoCodecSettings, VideoDescription,
};
use aws_sdk_mediaconvert::Client as MediaConvertClient;
use shared::error::AppError;
use shared::video::VideoStatus;
use std::collections::HashMap;

use crate::models::TranscodeJob;

/// S3 destinations for a transcode job's three output groups, all derived from the output bucket.
/// HLS (adaptive, public/unlisted delivery via CloudFront), MP4 (progressive, private delivery via
/// presigned S3 + universal fallback), and a single frame-capture thumbnail.
pub struct JobDestinations {
    pub hls: String,
    pub mp4: String,
    pub thumbnail: String,
}

impl JobDestinations {
    /// Conventional layout under the output (videos) bucket: `hls/{id}/`, `mp4/{id}/`,
    /// `thumbnails/{id}/`. Thumbnails live in the videos bucket (presigned by streaming), matching
    /// the existing layout.
    pub fn for_video(output_bucket: &str, video_id: &str) -> Self {
        Self {
            hls: format!("s3://{output_bucket}/hls/{video_id}/"),
            mp4: format!("s3://{output_bucket}/mp4/{video_id}/"),
            thumbnail: format!("s3://{output_bucket}/thumbnails/{video_id}/"),
        }
    }
}

/// Build a MediaConvert job that produces, in a single job: an HLS package with Automated ABR
/// (MediaConvert derives the rendition ladder), a progressive MP4, and a frame-capture thumbnail.
/// Pure (no AWS calls) so it is unit-testable. `input_uri` is `s3://{raw-bucket}/{key}`.
///
/// `thumb_interval_secs` is the frame-capture interval in seconds. MediaConvert frame capture
/// always emits frame 0 first (often a black fade-in), then one frame every interval; with
/// `max_captures = 2` the SECOND capture lands at `thumb_interval_secs` in. Callers pass ~25% of
/// the probed duration (see `thumbnail_interval_secs`) so the poster frame is representative rather
/// than the black opening frame.
pub fn build_job_settings(
    input_uri: &str,
    dest: &JobDestinations,
    thumb_interval_secs: i32,
) -> JobSettings {
    // Shared H.264 + AAC settings. QVBR rate control is required for Automated ABR and works well
    // for the MP4 too (no fixed bitrate to tune).
    // HLS template for Automated ABR: QVBR + MULTI_PASS_HQ, no fixed bitrate (ABR derives the ladder).
    let h264_abr = || {
        VideoCodecSettings::builder()
            .codec(VideoCodec::H264)
            .h264_settings(
                H264Settings::builder()
                    .rate_control_mode(H264RateControlMode::Qvbr)
                    .quality_tuning_level(H264QualityTuningLevel::MultiPassHq)
                    .build(),
            )
            .build()
    };
    // Progressive MP4: QVBR with no Automated ABR to manage it, so a max-bitrate cap is required.
    let h264_mp4 = || {
        VideoCodecSettings::builder()
            .codec(VideoCodec::H264)
            .h264_settings(
                H264Settings::builder()
                    .rate_control_mode(H264RateControlMode::Qvbr)
                    .max_bitrate(5_000_000)
                    .build(),
            )
            .build()
    };
    let aac = || {
        AudioDescription::builder()
            .audio_source_name("Audio Selector 1")
            .codec_settings(
                AudioCodecSettings::builder()
                    .codec(AudioCodec::Aac)
                    .aac_settings(
                        AacSettings::builder()
                            .bitrate(96_000)
                            .coding_mode(AacCodingMode::CodingMode20)
                            .sample_rate(48_000)
                            .build(),
                    )
                    .build(),
            )
            .build()
    };

    // 1) HLS output group with Automated ABR. The single Output is a template MediaConvert scales
    //    into multiple renditions.
    let hls_group = OutputGroup::builder()
        .name("HLS")
        .automated_encoding_settings(
            AutomatedEncodingSettings::builder()
                .abr_settings(AutomatedAbrSettings::builder().build())
                .build(),
        )
        .output_group_settings(
            OutputGroupSettings::builder()
                .r#type(OutputGroupType::HlsGroupSettings)
                .hls_group_settings(
                    HlsGroupSettings::builder()
                        .destination(&dest.hls)
                        .segment_length(6)
                        .min_segment_length(0)
                        .build(),
                )
                .build(),
        )
        .outputs(
            Output::builder()
                .name_modifier("_$dt$")
                .container_settings(
                    ContainerSettings::builder()
                        .container(ContainerType::M3U8)
                        .m3u8_settings(M3u8Settings::builder().build())
                        .build(),
                )
                .video_description(
                    VideoDescription::builder()
                        .codec_settings(h264_abr())
                        .build(),
                )
                .audio_descriptions(aac())
                .build(),
        )
        .build();

    // 2) Progressive MP4 (single rendition) for private playback + universal fallback.
    let mp4_group = OutputGroup::builder()
        .name("MP4")
        .output_group_settings(
            OutputGroupSettings::builder()
                .r#type(OutputGroupType::FileGroupSettings)
                .file_group_settings(FileGroupSettings::builder().destination(&dest.mp4).build())
                .build(),
        )
        .outputs(
            Output::builder()
                .name_modifier("video")
                .container_settings(
                    ContainerSettings::builder()
                        .container(ContainerType::Mp4)
                        .mp4_settings(Mp4Settings::builder().build())
                        .build(),
                )
                .video_description(
                    VideoDescription::builder()
                        .codec_settings(h264_mp4())
                        .build(),
                )
                .audio_descriptions(aac())
                .build(),
        )
        .build();

    // 3) Frame-capture thumbnail (no ffmpeg needed in the cloud path). MediaConvert always captures
    //    frame 0 first (often black), so we capture 2 frames `thumb_interval_secs` apart and let the
    //    completion consumer keep the SECOND one (the ~25% frame). framerate_numerator/denominator
    //    expresses "one capture every denominator/numerator seconds", so numerator=1 ⇒ the interval
    //    in seconds is exactly the denominator.
    let thumb_group = OutputGroup::builder()
        .name("Thumbnail")
        .output_group_settings(
            OutputGroupSettings::builder()
                .r#type(OutputGroupType::FileGroupSettings)
                .file_group_settings(
                    FileGroupSettings::builder()
                        .destination(&dest.thumbnail)
                        .build(),
                )
                .build(),
        )
        .outputs(
            Output::builder()
                .name_modifier("thumb")
                .container_settings(
                    ContainerSettings::builder()
                        .container(ContainerType::Raw)
                        .build(),
                )
                .video_description(
                    VideoDescription::builder()
                        .codec_settings(
                            VideoCodecSettings::builder()
                                .codec(VideoCodec::FrameCapture)
                                .frame_capture_settings(
                                    FrameCaptureSettings::builder()
                                        .framerate_numerator(1)
                                        .framerate_denominator(thumb_interval_secs.max(1))
                                        .max_captures(2)
                                        .quality(80)
                                        .build(),
                                )
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .build();

    JobSettings::builder()
        .inputs(
            Input::builder()
                .file_input(input_uri)
                // Audio descriptions reference this selector; without it MediaConvert errors at
                // encode time ("Invalid selector_sequence_id").
                .audio_selectors(
                    "Audio Selector 1",
                    AudioSelector::builder()
                        .default_selection(AudioDefaultSelection::Default)
                        .build(),
                )
                .build(),
        )
        .output_groups(hls_group)
        .output_groups(mp4_group)
        .output_groups(thumb_group)
        .build()
}

fn err(e: impl std::fmt::Debug + std::fmt::Display) -> AppError {
    // Log the Debug form: for AWS SdkError, Display is just "service error" while Debug carries the
    // error code and message. The AppError keeps the terse Display.
    tracing::error!(error = ?e, "internal error");
    AppError::internal(e)
}

/// Compute the frame-capture interval (seconds) that places the SECOND capture at ~25% of the
/// video. MediaConvert always emits frame 0 first, then one frame every interval, so with
/// `max_captures = 2` the second image sits at `interval` seconds in. Returns a small fixed
/// fallback when the duration is unknown or non-positive, so we never regress to a pure frame-0
/// (black) thumbnail. Pure + unit-tested.
pub fn thumbnail_interval_secs(duration_secs: Option<f64>) -> i32 {
    /// Used when Probe gives us no usable duration. Small enough to clear most black intros, yet
    /// the second capture still lands inside any clip longer than this.
    const FALLBACK_SECS: i32 = 3;
    match duration_secs {
        Some(d) if d.is_finite() && d > 0.0 => ((d * 0.25).round() as i32).max(1),
        _ => FALLBACK_SECS,
    }
}

/// Best-effort: probe the input via MediaConvert's `Probe` API to learn its duration (seconds),
/// used only to place the thumbnail at ~25%. Reads the file server-side from S3 (no download into
/// the pod, no ffmpeg). Returns `None` on any error so the caller falls back to a fixed interval —
/// a missing thumbnail offset must never block a transcode.
pub async fn probe_duration(client: &MediaConvertClient, input_uri: &str) -> Option<f64> {
    let resp = client
        .probe()
        .input_files(
            aws_sdk_mediaconvert::types::ProbeInputFile::builder()
                .file_url(input_uri)
                .build(),
        )
        .send()
        .await
        .inspect_err(|e| {
            tracing::warn!(error = ?e, input_uri, "MediaConvert Probe failed; using fixed thumbnail offset");
        })
        .ok()?;

    resp.probe_results()
        .first()
        .and_then(|r| r.container())
        .and_then(|c| c.duration())
        .filter(|d| d.is_finite() && *d > 0.0)
}

/// Submit a MediaConvert job for `job`, tagging it with `video_id` in userMetadata so the COMPLETE
/// EventBridge event can be correlated back to the video. Returns the MediaConvert job id.
pub async fn submit_mediaconvert_job(
    client: &MediaConvertClient,
    job: &TranscodeJob,
    output_bucket: &str,
    role_arn: &str,
) -> Result<String, AppError> {
    let input_uri = format!("s3://{}/{}", job.bucket, job.s3_key);
    let dest = JobDestinations::for_video(output_bucket, &job.video_id);

    // Probe first so the thumbnail lands at ~25% of the real duration (best-effort; falls back to a
    // fixed offset when Probe is unavailable). This is a quick, server-side metadata read.
    let duration = probe_duration(client, &input_uri).await;
    let thumb_interval = thumbnail_interval_secs(duration);
    tracing::info!(
        video_id = %job.video_id,
        duration_secs = ?duration,
        thumb_interval_secs = thumb_interval,
        "probed input; placing thumbnail near 25% of duration"
    );

    let settings = build_job_settings(&input_uri, &dest, thumb_interval);

    let resp = client
        .create_job()
        .role(role_arn)
        .user_metadata("video_id", &job.video_id)
        .settings(settings)
        .send()
        .await
        .map_err(|e| {
            // Surface MediaConvert's real rejection (code + message) instead of the SDK's terse
            // "service error". Debug carries the full structured error for the logs.
            let detail = e
                .as_service_error()
                .map(|se| {
                    format!(
                        "{}: {}",
                        se.code().unwrap_or("error"),
                        se.message().unwrap_or("(no message)")
                    )
                })
                .unwrap_or_else(|| "request failed before reaching MediaConvert".to_string());
            tracing::error!(error = ?e, video_id = %job.video_id, detail, "MediaConvert CreateJob failed");
            AppError::Internal(format!("MediaConvert CreateJob failed: {detail}"))
        })?;

    let job_id = resp
        .job()
        .and_then(|j| j.id())
        .unwrap_or("unknown")
        .to_string();

    Ok(job_id)
}

pub async fn update_video_status(
    db: &DynamoClient,
    video_id: &str,
    status: VideoStatus,
    manifest_url: Option<&str>,
    thumbnail_url: Option<&str>,
    s3_key: Option<&str>,
    duration_seconds: Option<f64>,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut expr_parts = vec!["#s = :status".to_string(), "updated_at = :now".to_string()];
    let mut values = HashMap::new();
    values.insert(
        ":status".to_string(),
        AttributeValue::S(status.as_str().to_string()),
    );
    values.insert(":now".to_string(), AttributeValue::S(now));

    if let Some(url) = manifest_url {
        expr_parts.push("manifest_url = :manifest".to_string());
        values.insert(":manifest".to_string(), AttributeValue::S(url.to_string()));
    }
    if let Some(url) = thumbnail_url {
        expr_parts.push("thumbnail_url = :thumb".to_string());
        values.insert(":thumb".to_string(), AttributeValue::S(url.to_string()));
    }
    if let Some(key) = s3_key {
        expr_parts.push("s3_key = :s3key".to_string());
        values.insert(":s3key".to_string(), AttributeValue::S(key.to_string()));
    }
    if let Some(dur) = duration_seconds {
        expr_parts.push("duration_seconds = :dur".to_string());
        values.insert(":dur".to_string(), AttributeValue::N(format!("{:.1}", dur)));
    }

    let expr = format!("SET {}", expr_parts.join(", "));

    // Resurrection guard: never overwrite a soft-deleted tombstone. If the video was deleted while
    // it was transcoding, a late MediaConvert completion must NOT re-publish (or re-fail) it. The
    // write is allowed when status is absent (brand-new row) or anything other than "deleted".
    values.insert(
        ":deleted".to_string(),
        AttributeValue::S(VideoStatus::Deleted.as_str().to_string()),
    );

    let result = db
        .update_item()
        .table_name(shared::tables::table("videos"))
        .key("video_id", AttributeValue::S(video_id.to_string()))
        .update_expression(expr)
        .condition_expression("attribute_not_exists(#s) OR #s <> :deleted")
        .expression_attribute_names("#s", "status")
        .set_expression_attribute_values(Some(values))
        .send()
        .await;

    if let Err(e) = result {
        // A failed condition means the video was deleted concurrently — expected; treat as a no-op.
        if e.as_service_error()
            .map(|se| se.is_conditional_check_failed_exception())
            .unwrap_or(false)
        {
            tracing::info!(
                video_id,
                "skipped status update; video was deleted (resurrection guard)"
            );
            return Ok(());
        }
        return Err(err(e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_mediaconvert::types::OutputGroupType;

    fn build() -> JobSettings {
        let dest = JobDestinations::for_video("rewind-dev-videos-us-west-2", "vid-123");
        build_job_settings(
            "s3://rewind-dev-raw-us-west-2/raw/vid-123/clip.mp4",
            &dest,
            45,
        )
    }

    #[test]
    fn thumbnail_interval_is_quarter_of_duration() {
        // 212.6s clip → 25% ≈ 53s.
        assert_eq!(thumbnail_interval_secs(Some(212.6)), 53);
        // Rounds to nearest second.
        assert_eq!(thumbnail_interval_secs(Some(40.0)), 10);
    }

    #[test]
    fn thumbnail_interval_never_zero() {
        // Very short clips still get a >=1s interval so the second capture exists.
        assert_eq!(thumbnail_interval_secs(Some(2.0)), 1);
        assert_eq!(thumbnail_interval_secs(Some(0.5)), 1);
    }

    #[test]
    fn thumbnail_interval_falls_back_when_duration_unknown() {
        assert_eq!(thumbnail_interval_secs(None), 3);
        assert_eq!(thumbnail_interval_secs(Some(0.0)), 3);
        assert_eq!(thumbnail_interval_secs(Some(f64::NAN)), 3);
        assert_eq!(thumbnail_interval_secs(Some(-5.0)), 3);
    }

    #[test]
    fn frame_capture_uses_two_captures_at_given_interval() {
        let dest = JobDestinations::for_video("bkt", "v");
        let s = build_job_settings("s3://raw/v/clip.mp4", &dest, 53);
        let fc = s
            .output_groups()
            .iter()
            .find_map(|g| {
                g.outputs().iter().find_map(|o| {
                    o.video_description()
                        .and_then(|v| v.codec_settings())
                        .and_then(|c| c.frame_capture_settings())
                })
            })
            .expect("frame capture settings present");
        assert_eq!(fc.max_captures(), Some(2));
        assert_eq!(fc.framerate_numerator(), Some(1));
        assert_eq!(fc.framerate_denominator(), Some(53));
    }

    #[test]
    fn destinations_follow_convention() {
        let d = JobDestinations::for_video("bkt", "abc");
        assert_eq!(d.hls, "s3://bkt/hls/abc/");
        assert_eq!(d.mp4, "s3://bkt/mp4/abc/");
        assert_eq!(d.thumbnail, "s3://bkt/thumbnails/abc/");
    }

    #[test]
    fn input_uri_is_set() {
        let s = build();
        assert_eq!(
            s.inputs()[0].file_input(),
            Some("s3://rewind-dev-raw-us-west-2/raw/vid-123/clip.mp4")
        );
    }

    #[test]
    fn has_three_output_groups() {
        assert_eq!(build().output_groups().len(), 3);
    }

    #[test]
    fn hls_group_has_automated_abr_and_destination() {
        let s = build();
        let hls = s
            .output_groups()
            .iter()
            .find(|g| {
                g.output_group_settings().and_then(|o| o.r#type())
                    == Some(&OutputGroupType::HlsGroupSettings)
            })
            .expect("HLS group present");
        // Automated ABR configured (so MediaConvert derives the rendition ladder).
        assert!(hls
            .automated_encoding_settings()
            .and_then(|a| a.abr_settings())
            .is_some());
        // 6-second segments at the conventional destination.
        let hls_settings = hls
            .output_group_settings()
            .unwrap()
            .hls_group_settings()
            .unwrap();
        assert_eq!(
            hls_settings.destination(),
            Some("s3://rewind-dev-videos-us-west-2/hls/vid-123/")
        );
        assert_eq!(hls_settings.segment_length(), Some(6));
    }

    #[test]
    fn mp4_and_thumbnail_file_groups_present() {
        let s = build();
        let file_dests: Vec<&str> = s
            .output_groups()
            .iter()
            .filter_map(|g| {
                g.output_group_settings()
                    .and_then(|o| o.file_group_settings())
                    .and_then(|f| f.destination())
            })
            .collect();
        assert!(file_dests.contains(&"s3://rewind-dev-videos-us-west-2/mp4/vid-123/"));
        assert!(file_dests.contains(&"s3://rewind-dev-videos-us-west-2/thumbnails/vid-123/"));
    }

    #[test]
    fn thumbnail_group_uses_frame_capture() {
        let s = build();
        let has_frame_capture = s.output_groups().iter().any(|g| {
            g.outputs().iter().any(|o| {
                o.video_description()
                    .and_then(|v| v.codec_settings())
                    .and_then(|c| c.frame_capture_settings())
                    .is_some()
            })
        });
        assert!(has_frame_capture);
    }
}
