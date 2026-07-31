use axum::{extract::State, http::HeaderMap, http::StatusCode, Json};
use shared::error::AppError;

use crate::models::*;
use crate::repo;
use crate::state::AppState;

pub async fn initiate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InitiateRequest>,
) -> Result<(StatusCode, Json<InitiateResponse>), AppError> {
    let _user_id = shared::auth::authenticate(&state.db, &headers).await?;

    if req.video_id.trim().is_empty() {
        return Err(AppError::BadRequest("video_id is required".to_string()));
    }
    if req.filename.trim().is_empty() {
        return Err(AppError::BadRequest("filename is required".to_string()));
    }
    if req.part_count == 0 {
        return Err(AppError::BadRequest(
            "part_count must be at least 1".to_string(),
        ));
    }
    if !req.content_type.starts_with("video/") {
        return Err(AppError::BadRequest(
            "content_type must be a video MIME type".to_string(),
        ));
    }

    let key = format!("raw/{}/{}", req.video_id, req.filename);

    let upload_id =
        repo::initiate_multipart(&state.s3, &state.bucket, &key, &req.content_type).await?;

    let presigned_urls =
        repo::generate_presigned_urls(&state.s3, &state.bucket, &key, &upload_id, req.part_count)
            .await?;

    Ok((
        StatusCode::OK,
        Json(InitiateResponse {
            upload_id,
            s3_key: key,
            presigned_urls,
        }),
    ))
}

pub async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteRequest>,
) -> Result<Json<CompleteResponse>, AppError> {
    let _user_id = shared::auth::authenticate(&state.db, &headers).await?;

    if req.video_id.trim().is_empty() {
        return Err(AppError::BadRequest("video_id is required".to_string()));
    }
    if req.upload_id.trim().is_empty() {
        return Err(AppError::BadRequest("upload_id is required".to_string()));
    }
    if req.s3_key.trim().is_empty() {
        return Err(AppError::BadRequest("s3_key is required".to_string()));
    }

    // Assemble parts from S3, not the request body (keeps the body size-independent).
    let parts = repo::list_parts(&state.s3, &state.bucket, &req.s3_key, &req.upload_id).await?;
    if parts.is_empty() {
        return Err(AppError::BadRequest(
            "no uploaded parts found for upload_id".to_string(),
        ));
    }

    repo::complete_multipart(
        &state.s3,
        &state.bucket,
        &req.s3_key,
        &req.upload_id,
        &parts,
    )
    .await?;

    repo::enqueue_transcode_job(
        &state.sqs,
        &state.queue_url,
        &req.video_id,
        &req.s3_key,
        &state.bucket,
    )
    .await?;

    Ok(Json(CompleteResponse {
        message: "upload complete, transcode job enqueued".to_string(),
        s3_key: req.s3_key,
    }))
}
