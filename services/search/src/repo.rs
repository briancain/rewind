use opensearch::{DeleteParts, IndexParts, SearchParts};
use serde_json::{json, Value};
use shared::error::AppError;

use crate::models::{SearchResponse, VideoDocument};
use crate::signing::SearchClient;

const INDEX: &str = "videos";

pub async fn index_video(client: &SearchClient, doc: &VideoDocument) -> Result<(), AppError> {
    let body = serde_json::to_value(doc).map_err(AppError::internal)?;

    let resp = client
        .send_with_retry(|| {
            client
                .os
                .index(IndexParts::IndexId(INDEX, &doc.video_id))
                .body(body.clone())
                .send()
        })
        .await?;

    if !resp.status_code().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "OpenSearch index failed: {}",
            body
        )));
    }
    Ok(())
}

/// Delete a video document from the index. A 404 (doc/index not present) is treated as success,
/// since the desired end state — "this video is not in the index" — is already satisfied. This
/// makes the operation idempotent and safe for at-least-once stream delivery.
pub async fn delete_video_doc(client: &SearchClient, video_id: &str) -> Result<(), AppError> {
    let resp = client
        .send_with_retry(|| {
            client
                .os
                .delete(DeleteParts::IndexId(INDEX, video_id))
                .send()
        })
        .await?;

    let status = resp.status_code().as_u16();
    if status == 404 || resp.status_code().is_success() {
        return Ok(());
    }

    let body = resp.text().await.unwrap_or_default();
    Err(AppError::Internal(format!(
        "OpenSearch delete failed ({}): {}",
        status, body
    )))
}

pub async fn search_videos(client: &SearchClient, query: &str) -> Result<SearchResponse, AppError> {
    let body = json!({
        "query": {
            "multi_match": {
                "query": query,
                "fields": ["title^3", "description", "tags"]
            }
        }
    });

    let resp = client
        .send_with_retry(|| {
            client
                .os
                .search(SearchParts::Index(&[INDEX]))
                .body(body.clone())
                .send()
        })
        .await?;

    let status = resp.status_code().as_u16();
    if status == 404 {
        return Ok(SearchResponse {
            results: vec![],
            total: 0,
        });
    }

    if !resp.status_code().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "OpenSearch search failed: {}",
            body
        )));
    }

    let data: Value = resp.json().await.map_err(AppError::internal)?;
    let hits = data["hits"]["hits"].as_array().cloned().unwrap_or_default();
    let total = data["hits"]["total"]["value"].as_u64().unwrap_or(0);

    let results = hits
        .into_iter()
        .filter_map(|hit| serde_json::from_value(hit["_source"].clone()).ok())
        .collect();

    Ok(SearchResponse { results, total })
}
