use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::CompletedMultipartUpload;
use aws_sdk_s3::types::CompletedPart;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;
use shared::error::AppError;
use std::time::Duration;

pub async fn initiate_multipart(
    s3: &S3Client,
    bucket: &str,
    key: &str,
    content_type: &str,
) -> Result<String, AppError> {
    let resp = s3
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .send()
        .await
        .map_err(|e| AppError::from_aws("create_multipart_upload", e))?;

    resp.upload_id()
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::Internal("no upload_id returned".to_string()))
}

pub async fn generate_presigned_urls(
    s3: &S3Client,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_count: u32,
) -> Result<Vec<String>, AppError> {
    let mut urls = Vec::with_capacity(part_count as usize);

    for part in 1..=part_count {
        let presign_config =
            PresigningConfig::expires_in(Duration::from_secs(3600)).map_err(AppError::internal)?;

        let url = s3
            .upload_part()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part as i32)
            .presigned(presign_config)
            .await
            .map_err(|e| AppError::from_aws("presign_upload_part", e))?;

        urls.push(url.uri().to_string());
    }

    Ok(urls)
}

// List uploaded parts as (part_number, etag) sorted by number, paginated (S3 caps pages at 1000;
// a 5 GB upload at 5 MB parts hits that). Built server-side so the /complete body stays tiny.
pub async fn list_parts(
    s3: &S3Client,
    bucket: &str,
    key: &str,
    upload_id: &str,
) -> Result<Vec<(i32, String)>, AppError> {
    let mut parts: Vec<(i32, String)> = Vec::new();
    let mut marker: Option<String> = None;

    loop {
        let mut req = s3.list_parts().bucket(bucket).key(key).upload_id(upload_id);
        if let Some(m) = marker.take() {
            req = req.part_number_marker(m);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::from_aws("list_parts", e))?;

        for part in resp.parts() {
            if let (Some(number), Some(etag)) = (part.part_number(), part.e_tag()) {
                parts.push((number, etag.to_string()));
            }
        }

        if resp.is_truncated() == Some(true) {
            match resp.next_part_number_marker() {
                Some(m) => marker = Some(m.to_string()),
                None => break,
            }
        } else {
            break;
        }
    }

    parts.sort_by_key(|(number, _)| *number);
    Ok(parts)
}

pub async fn complete_multipart(
    s3: &S3Client,
    bucket: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
) -> Result<(), AppError> {
    let completed_parts: Vec<CompletedPart> = parts
        .iter()
        .map(|(num, etag)| {
            CompletedPart::builder()
                .part_number(*num)
                .e_tag(etag)
                .build()
        })
        .collect();

    let upload = CompletedMultipartUpload::builder()
        .set_parts(Some(completed_parts))
        .build();

    s3.complete_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(upload_id)
        .multipart_upload(upload)
        .send()
        .await
        .map_err(|e| AppError::from_aws("complete_multipart_upload", e))?;

    Ok(())
}

pub async fn enqueue_transcode_job(
    sqs: &SqsClient,
    queue_url: &str,
    video_id: &str,
    s3_key: &str,
    bucket: &str,
) -> Result<(), AppError> {
    let body = serde_json::json!({
        "video_id": video_id,
        "s3_key": s3_key,
        "bucket": bucket,
    })
    .to_string();

    sqs.send_message()
        .queue_url(queue_url)
        .message_body(body)
        .send()
        .await
        .map_err(AppError::internal)?;

    Ok(())
}
