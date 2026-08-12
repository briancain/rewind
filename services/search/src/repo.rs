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

/// Build the OpenSearch query body for a free-text search across title/description/tags. Pure (no
/// I/O) so the query shape is unit-testable without a cluster.
pub fn build_text_query(query: &str) -> Value {
    json!({
        "query": {
            "multi_match": {
                "query": query,
                "fields": ["title^3", "description", "tags"]
            }
        }
    })
}

/// Build the OpenSearch query body for an exact tag filter, sorted newest-first.
///
/// This is the query behind a clickable hashtag. It matches on the `tags.keyword` sub-field (the
/// exact, un-analyzed tag) via a `term` query — so `#cats` returns videos actually *tagged* "cats",
/// not videos that merely mention "cats" in their title or description (which is what the free-text
/// `multi_match` above would do). `case_insensitive` groups `#Cats` with `#cats` without requiring
/// tags to be normalized at write time, so no reindex of existing data is needed.
///
/// Results are ordered by `created_at` descending: a tag page is a browse feed where every hit
/// matches the tag equally, so BM25 relevance carries no signal and recency is the meaningful
/// default. (`created_at` is an RFC3339 UTC string; OpenSearch dynamic date-detection maps it to a
/// `date`, so the sort is chronological.)
pub fn build_tag_query(tag: &str) -> Value {
    json!({
        "query": {
            "term": {
                "tags.keyword": {
                    "value": tag,
                    "case_insensitive": true
                }
            }
        },
        "sort": [
            { "created_at": { "order": "desc" } }
        ]
    })
}

/// Parse an OpenSearch `_search` response body into our `SearchResponse`. Pure so the hit
/// extraction is unit-testable independently of the transport. Documents that fail to deserialize
/// are skipped rather than failing the whole response.
pub fn parse_search_response(data: &Value) -> SearchResponse {
    let hits = data["hits"]["hits"].as_array().cloned().unwrap_or_default();
    let total = data["hits"]["total"]["value"].as_u64().unwrap_or(0);

    let results = hits
        .into_iter()
        .filter_map(|hit| serde_json::from_value(hit["_source"].clone()).ok())
        .collect();

    SearchResponse { results, total }
}

/// Run a prepared query body against the index. A missing index (404) is treated as an empty
/// result (the index is created lazily on first upsert), and any other non-success maps to an
/// error. Shared by the free-text and tag search paths.
async fn run_search(client: &SearchClient, body: Value) -> Result<SearchResponse, AppError> {
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
    Ok(parse_search_response(&data))
}

/// Free-text search across title/description/tags, ranked by BM25 relevance.
pub async fn search_videos(client: &SearchClient, query: &str) -> Result<SearchResponse, AppError> {
    run_search(client, build_text_query(query)).await
}

/// Exact tag filter (the clickable-hashtag path), newest-first.
pub async fn search_by_tag(client: &SearchClient, tag: &str) -> Result<SearchResponse, AppError> {
    run_search(client, build_tag_query(tag)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_query_multi_matches_boosted_fields() {
        let q = build_text_query("cats");
        let mm = &q["query"]["multi_match"];
        assert_eq!(mm["query"], "cats");
        let fields: Vec<&str> = mm["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        assert_eq!(fields, vec!["title^3", "description", "tags"]);
        // Free-text search is relevance-ranked: it must NOT impose an explicit sort.
        assert!(q.get("sort").is_none());
    }

    #[test]
    fn tag_query_is_exact_case_insensitive_term_on_keyword() {
        let q = build_tag_query("cats");
        let term = &q["query"]["term"]["tags.keyword"];
        // Exact match on the un-analyzed sub-field (not the analyzed `tags` text field), so it
        // matches the tag itself and not free-text mentions in title/description.
        assert_eq!(term["value"], "cats");
        assert_eq!(term["case_insensitive"], true);
    }

    #[test]
    fn tag_query_sorts_newest_first() {
        let q = build_tag_query("cats");
        assert_eq!(q["sort"][0]["created_at"]["order"], "desc");
    }

    #[test]
    fn tag_query_preserves_tag_value_verbatim() {
        // Multi-word / mixed-case tags flow through unchanged; case-insensitivity is handled by the
        // query option, not by mangling the value.
        let q = build_tag_query("Big Cats");
        assert_eq!(q["query"]["term"]["tags.keyword"]["value"], "Big Cats");
    }

    #[test]
    fn parse_extracts_hits_and_total() {
        let data = json!({
            "hits": {
                "total": { "value": 2 },
                "hits": [
                    { "_source": { "video_id": "v1", "title": "One" } },
                    { "_source": { "video_id": "v2", "title": "Two" } }
                ]
            }
        });
        let resp = parse_search_response(&data);
        assert_eq!(resp.total, 2);
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].video_id, "v1");
        assert_eq!(resp.results[1].video_id, "v2");
    }

    #[test]
    fn parse_preserves_hit_order() {
        // The tag path relies on OpenSearch's `sort` order surviving parsing (newest-first).
        let data = json!({
            "hits": {
                "total": { "value": 2 },
                "hits": [
                    { "_source": { "video_id": "newer", "title": "N", "created_at": "2026-02-01T00:00:00Z" } },
                    { "_source": { "video_id": "older", "title": "O", "created_at": "2026-01-01T00:00:00Z" } }
                ]
            }
        });
        let resp = parse_search_response(&data);
        assert_eq!(resp.results[0].video_id, "newer");
        assert_eq!(resp.results[1].video_id, "older");
    }

    #[test]
    fn parse_empty_hits_is_zero() {
        let data = json!({ "hits": { "total": { "value": 0 }, "hits": [] } });
        let resp = parse_search_response(&data);
        assert_eq!(resp.total, 0);
        assert!(resp.results.is_empty());
    }

    #[test]
    fn parse_skips_malformed_hits() {
        // A hit missing the required `video_id` is skipped, not fatal.
        let data = json!({
            "hits": {
                "total": { "value": 2 },
                "hits": [
                    { "_source": { "title": "no id" } },
                    { "_source": { "video_id": "v2", "title": "Two" } }
                ]
            }
        });
        let resp = parse_search_response(&data);
        // total reflects the raw OpenSearch count; only the well-formed doc survives parsing.
        assert_eq!(resp.total, 2);
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].video_id, "v2");
    }
}
