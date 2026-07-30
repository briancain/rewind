//! Pure decision logic for turning a DynamoDB stream record (as delivered by EventBridge Pipes)
//! into an action against the OpenSearch index. Kept free of I/O so it is fully unit-testable
//! without any infrastructure.
//!
//! Indexing rule: a video is present in the search index **iff** it is `published` AND `public`.
//! Any other state (draft/processing, unlisted, private) means the document must be absent — so we
//! emit a `Delete`, which also removes a doc that was previously public+published (this is what
//! fixes the visibility-drift gap). Deletes are idempotent by `video_id`, so at-least-once stream
//! delivery and retries are safe.

use serde_json::Value;

use crate::models::VideoDocument;
use shared::video::{VideoStatus, Visibility};

#[derive(Debug, PartialEq)]
pub enum IndexAction {
    /// Upsert this document into the index (boxed to keep the enum small).
    Upsert(Box<VideoDocument>),
    /// Remove this video_id from the index.
    Delete(String),
    /// Nothing actionable (malformed record, missing keys, etc.).
    Skip,
}

/// Extract a DynamoDB String attribute, e.g. `{"S": "value"}`.
fn ddb_str(image: &Value, field: &str) -> Option<String> {
    image.get(field)?.get("S")?.as_str().map(|s| s.to_string())
}

/// Extract a DynamoDB String List attribute, e.g. `{"L": [{"S":"a"},{"S":"b"}]}`.
fn ddb_str_list(image: &Value, field: &str) -> Vec<String> {
    image
        .get(field)
        .and_then(|v| v.get("L"))
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("S").and_then(|s| s.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Build a `VideoDocument` from a DynamoDB image (typically `NewImage`).
/// Returns `None` if the image has no `video_id`.
pub fn document_from_image(image: &Value) -> Option<VideoDocument> {
    let video_id = ddb_str(image, "video_id")?;
    Some(VideoDocument {
        video_id,
        title: ddb_str(image, "title").unwrap_or_default(),
        description: ddb_str(image, "description").unwrap_or_default(),
        tags: ddb_str_list(image, "tags"),
        channel_id: ddb_str(image, "channel_id").unwrap_or_default(),
        genre: ddb_str(image, "genre").unwrap_or_default(),
        created_at: ddb_str(image, "created_at").unwrap_or_default(),
    })
}

/// Returns true if a video in the given (status, visibility) state belongs in the search index.
fn is_indexable(status: &str, visibility: &str) -> bool {
    status == VideoStatus::Published.as_str() && visibility == Visibility::Public.as_str()
}

/// Core eligibility decision from a single DynamoDB image (a `NewImage` or a scanned item).
/// This is the one place the index-membership rule lives: upsert iff published+public, otherwise
/// delete by `video_id`, skip if the image has no `video_id`.
fn decide_from_image(image: &Value) -> IndexAction {
    let video_id = match ddb_str(image, "video_id") {
        Some(id) => id,
        None => return IndexAction::Skip,
    };

    let status = ddb_str(image, "status").unwrap_or_default();
    // Visibility defaults to "public" when absent, matching the catalog's create-time default.
    let visibility =
        ddb_str(image, "visibility").unwrap_or_else(|| Visibility::Public.as_str().to_string());

    if is_indexable(&status, &visibility) {
        // video_id is already present, so document_from_image is Some; map defensively anyway.
        match document_from_image(image) {
            Some(doc) => IndexAction::Upsert(Box::new(doc)),
            None => IndexAction::Skip,
        }
    } else {
        // Not eligible — ensure it is absent (handles public->private/unlisted, unpublish, drafts).
        IndexAction::Delete(video_id)
    }
}

/// Decide what to do with a single DynamoDB stream record (`{"eventName", "dynamodb": {...}}`).
/// Handles the stream envelope (REMOVE vs INSERT/MODIFY) and delegates the eligibility rule to
/// [`decide_from_image`].
pub fn decide_action(record: &Value) -> IndexAction {
    let event_name = record
        .get("eventName")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let dynamodb = match record.get("dynamodb") {
        Some(d) => d,
        None => return IndexAction::Skip,
    };

    // REMOVE: there is no NewImage; delete by key (fall back to OldImage).
    if event_name == "REMOVE" {
        if let Some(id) = dynamodb.get("Keys").and_then(|k| ddb_str(k, "video_id")) {
            return IndexAction::Delete(id);
        }
        if let Some(id) = dynamodb
            .get("OldImage")
            .and_then(|i| ddb_str(i, "video_id"))
        {
            return IndexAction::Delete(id);
        }
        return IndexAction::Skip;
    }

    // INSERT / MODIFY: the desired index state is derived purely from the new image.
    match dynamodb.get("NewImage") {
        Some(new_image) => decide_from_image(new_image),
        None => IndexAction::Skip,
    }
}

/// Decide the action for a full table item (as returned by a `Scan`, in DynamoDB attribute-value
/// format). Used by the `/reindex` backfill. Same eligibility rule as the stream consumer.
pub fn decide_action_for_item(item: &Value) -> IndexAction {
    decide_from_image(item)
}

/// Parse an SQS message body delivered by EventBridge Pipes into a list of stream records.
/// Handles both a single record object and a batched JSON array of records.
pub fn parse_pipe_message(body: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(body) {
        Ok(Value::Array(items)) => items,
        Ok(obj @ Value::Object(_)) => vec![obj],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a DynamoDB stream record with the given event name and optional images.
    fn record(event: &str, new_image: Option<Value>, old_image: Option<Value>) -> Value {
        let mut dynamodb = serde_json::Map::new();
        // Keys are always present on real records.
        if let Some(img) = new_image.as_ref().or(old_image.as_ref()) {
            if let Some(vid) = img.get("video_id") {
                dynamodb.insert("Keys".to_string(), json!({ "video_id": vid }));
            }
        }
        if let Some(img) = new_image {
            dynamodb.insert("NewImage".to_string(), img);
        }
        if let Some(img) = old_image {
            dynamodb.insert("OldImage".to_string(), img);
        }
        json!({ "eventName": event, "eventSource": "aws:dynamodb", "dynamodb": dynamodb })
    }

    fn image(video_id: &str, status: &str, visibility: &str) -> Value {
        json!({
            "video_id": {"S": video_id},
            "title": {"S": "A Title"},
            "description": {"S": "A description"},
            "tags": {"L": [{"S": "rust"}, {"S": "video"}]},
            "channel_id": {"S": "chan-1"},
            "genre": {"S": "tech"},
            "status": {"S": status},
            "visibility": {"S": visibility},
            "created_at": {"S": "2026-06-11T00:00:00Z"}
        })
    }

    #[test]
    fn insert_published_public_upserts_with_full_doc() {
        let rec = record("INSERT", Some(image("v1", "published", "public")), None);
        match decide_action(&rec) {
            IndexAction::Upsert(doc) => {
                assert_eq!(doc.video_id, "v1");
                assert_eq!(doc.title, "A Title");
                assert_eq!(doc.tags, vec!["rust", "video"]);
                assert_eq!(doc.channel_id, "chan-1");
                assert_eq!(doc.genre, "tech");
                assert_eq!(doc.created_at, "2026-06-11T00:00:00Z");
            }
            other => panic!("expected Upsert, got {:?}", other),
        }
    }

    #[test]
    fn insert_draft_deletes() {
        // A brand-new video is created as a draft; it must not be indexed.
        let rec = record("INSERT", Some(image("v1", "draft", "public")), None);
        assert_eq!(decide_action(&rec), IndexAction::Delete("v1".into()));
    }

    #[test]
    fn modify_public_to_private_deletes() {
        // The visibility-drift regression: flipping a published video to private removes it.
        let rec = record(
            "MODIFY",
            Some(image("v1", "published", "private")),
            Some(image("v1", "published", "public")),
        );
        assert_eq!(decide_action(&rec), IndexAction::Delete("v1".into()));
    }

    #[test]
    fn modify_public_to_unlisted_deletes() {
        let rec = record(
            "MODIFY",
            Some(image("v1", "published", "unlisted")),
            Some(image("v1", "published", "public")),
        );
        assert_eq!(decide_action(&rec), IndexAction::Delete("v1".into()));
    }

    #[test]
    fn modify_title_while_public_upserts() {
        let mut new_img = image("v1", "published", "public");
        new_img["title"] = json!({"S": "Edited Title"});
        let rec = record(
            "MODIFY",
            Some(new_img),
            Some(image("v1", "published", "public")),
        );
        match decide_action(&rec) {
            IndexAction::Upsert(doc) => assert_eq!(doc.title, "Edited Title"),
            other => panic!("expected Upsert, got {:?}", other),
        }
    }

    #[test]
    fn remove_deletes_by_key() {
        let rec = record("REMOVE", None, Some(image("v9", "published", "public")));
        assert_eq!(decide_action(&rec), IndexAction::Delete("v9".into()));
    }

    #[test]
    fn remove_falls_back_to_old_image_when_no_keys() {
        // Construct a REMOVE record that only carries OldImage (no Keys).
        let rec = json!({
            "eventName": "REMOVE",
            "dynamodb": { "OldImage": image("v9", "published", "public") }
        });
        assert_eq!(decide_action(&rec), IndexAction::Delete("v9".into()));
    }

    #[test]
    fn missing_visibility_defaults_public() {
        let mut img = image("v1", "published", "public");
        img.as_object_mut().unwrap().remove("visibility");
        let rec = record("INSERT", Some(img), None);
        match decide_action(&rec) {
            IndexAction::Upsert(doc) => assert_eq!(doc.video_id, "v1"),
            other => panic!("expected Upsert (default public), got {:?}", other),
        }
    }

    #[test]
    fn missing_dynamodb_skips() {
        let rec = json!({ "eventName": "INSERT" });
        assert_eq!(decide_action(&rec), IndexAction::Skip);
    }

    #[test]
    fn missing_new_image_on_insert_skips() {
        let rec =
            json!({ "eventName": "INSERT", "dynamodb": { "Keys": {"video_id": {"S": "v1"}} } });
        assert_eq!(decide_action(&rec), IndexAction::Skip);
    }

    #[test]
    fn missing_video_id_skips() {
        let rec = json!({
            "eventName": "INSERT",
            "dynamodb": { "NewImage": { "title": {"S": "no id"}, "status": {"S": "published"} } }
        });
        assert_eq!(decide_action(&rec), IndexAction::Skip);
    }

    #[test]
    fn decide_action_for_item_published_public_upserts() {
        let item = image("v1", "published", "public");
        match decide_action_for_item(&item) {
            IndexAction::Upsert(doc) => assert_eq!(doc.video_id, "v1"),
            other => panic!("expected Upsert, got {:?}", other),
        }
    }

    #[test]
    fn decide_action_for_item_private_deletes() {
        let item = image("v1", "published", "private");
        assert_eq!(
            decide_action_for_item(&item),
            IndexAction::Delete("v1".into())
        );
    }

    #[test]
    fn parse_pipe_message_single_object() {
        let body = json!({"eventName": "INSERT", "dynamodb": {}}).to_string();
        let recs = parse_pipe_message(&body);
        assert_eq!(recs.len(), 1);
    }

    #[test]
    fn parse_pipe_message_batched_array() {
        let body = json!([
            {"eventName": "INSERT", "dynamodb": {}},
            {"eventName": "REMOVE", "dynamodb": {}}
        ])
        .to_string();
        let recs = parse_pipe_message(&body);
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn parse_pipe_message_invalid_is_empty() {
        assert!(parse_pipe_message("not json {{{").is_empty());
    }
}
