//! Local-only validator: submits our `build_job_settings` job to the REAL MediaConvert and polls it
//! to completion, so we catch BOTH CreateJob validation errors and runtime encode errors without a
//! deploy cycle. Ignored by default (needs AWS creds + a real input).
//!
//! Run:
//!   AWS_PROFILE=rewind AWS_REGION=us-west-2 \
//!     MC_ROLE="$(cd infra/environments/dev/us-west-2 && terraform output -raw mediaconvert_role_arn)" \
//!     INPUT_URI="s3://rewind-dev-raw-us-west-2/raw/<id>/<file>.mp4" \
//!     cargo test -p transcode --test mc_validate -- --ignored --nocapture
//!
//! Outputs go under the `_settings-validate/` prefix (no userMetadata, so the completion consumer
//! ignores any event). Clean them up afterward with:
//!   aws s3 rm s3://rewind-dev-videos-us-west-2/hls/_settings-validate/ --recursive  (+ mp4/, thumbnails/)

use aws_sdk_mediaconvert::error::ProvideErrorMetadata;
use aws_sdk_mediaconvert::types::JobStatus;
use transcode::repo::{build_job_settings, JobDestinations};

#[tokio::test]
#[ignore]
async fn validate_job_settings_against_real_mediaconvert() {
    let role = std::env::var("MC_ROLE").expect("set MC_ROLE to the MediaConvert service role ARN");
    let input = std::env::var("INPUT_URI").expect("set INPUT_URI to a real s3:// input file");
    let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;
    let client = aws_sdk_mediaconvert::Client::new(&conf);

    let dest = JobDestinations::for_video("rewind-dev-videos-us-west-2", "_settings-validate");
    // Exercise the real Probe path too, then place the thumbnail at ~25% just like production.
    let duration = transcode::repo::probe_duration(&client, &input).await;
    let thumb_interval = transcode::repo::thumbnail_interval_secs(duration);
    println!("probed duration={duration:?}s → thumbnail interval={thumb_interval}s");
    let settings = build_job_settings(&input, &dest, thumb_interval);

    let created = client
        .create_job()
        .role(role)
        .settings(settings)
        .send()
        .await
        .unwrap_or_else(|e| {
            let d = e
                .as_service_error()
                .and_then(|se| se.message())
                .map(|m| m.to_string())
                .unwrap_or_else(|| format!("{e:?}"));
            panic!("CreateJob REJECTED (settings invalid): {d}");
        });
    let id = created
        .job()
        .and_then(|j| j.id())
        .expect("job id")
        .to_string();
    println!("CreateJob accepted; job {id} submitted — polling to completion...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let resp = client.get_job().id(&id).send().await.expect("get_job");
        let job = resp.job().expect("job present");
        match job.status() {
            Some(JobStatus::Complete) => {
                println!("JOB COMPLETE — settings fully valid (CreateJob + encode).");
                // Note: the GetJob API model does not carry output file paths (those appear only in
                // the EventBridge COMPLETE event). Verify the produced thumbnails by listing
                // s3://rewind-dev-videos-us-west-2/thumbnails/_settings-validate/ after this run —
                // expect thumb.0000000.jpg (frame 0) + thumb.0000001.jpg (the ~25% poster frame).
                break;
            }
            Some(JobStatus::Error) => {
                panic!(
                    "JOB ERROR (runtime): code={:?} message={:?}",
                    job.error_code(),
                    job.error_message()
                );
            }
            other => println!("  status: {other:?} ..."),
        }
    }
}
