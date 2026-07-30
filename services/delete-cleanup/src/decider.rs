//! Pure decision logic for turning a `videos` DynamoDB stream record (as delivered by an
//! EventBridge Pipe) into a cleanup action. Kept free of I/O so it is fully unit-testable.
//!
//! The worker only acts on a **soft-delete**: a record whose new image shows
//! `status == "deleted"`. The EventBridge Pipe is additionally
//! filtered to only forward those events, so in cloud the worker rarely sees anything else — but
//! the decision is made here too (defense in depth) so the logic is correct even without the
//! filter and is exhaustively unit-tested.
//!
//! We key purely off the *resulting image* (`status == deleted`), not the event name, which makes
//! the decision idempotent: re-delivering the soft-delete event simply re-issues `Cleanup`, and
//! cleaning an already-clean video is a no-op. A `REMOVE` (the later finalizer hard-delete) carries
//! no `NewImage`, so it yields `Skip` — cleanup already ran when the row was soft-deleted.

use serde_json::Value;
use shared::video::VideoStatus;

#[derive(Debug, PartialEq, Eq)]
pub enum CleanupAction {
    /// Reclaim all dependent data for this `video_id`.
    Cleanup(String),
    /// Nothing to do (not a soft-delete, malformed, or missing keys).
    Skip,
}

/// Extract a DynamoDB String attribute, e.g. `{"S": "value"}`.
fn ddb_str(image: &Value, field: &str) -> Option<String> {
    image.get(field)?.get("S")?.as_str().map(|s| s.to_string())
}

/// Decide what to do with a single DynamoDB stream record
/// (`{"eventName", "dynamodb": {"NewImage"|"OldImage"|"Keys": ...}}`).
///
/// Returns `Cleanup(video_id)` iff the record's `NewImage` shows `status == "deleted"` and carries
/// a `video_id`; otherwise `Skip`.
pub fn decide(record: &Value) -> CleanupAction {
    let dynamodb = match record.get("dynamodb") {
        Some(d) => d,
        None => return CleanupAction::Skip,
    };

    // Act only when the resulting image is a soft-delete tombstone. REMOVE / non-delete MODIFY /
    // INSERT all fall through to Skip.
    let new_image = match dynamodb.get("NewImage") {
        Some(img) => img,
        None => return CleanupAction::Skip,
    };

    if ddb_str(new_image, "status").as_deref() != Some(VideoStatus::Deleted.as_str()) {
        return CleanupAction::Skip;
    }

    match ddb_str(new_image, "video_id") {
        Some(video_id) => CleanupAction::Cleanup(video_id),
        None => CleanupAction::Skip,
    }
}

/// Parse an SQS message body delivered by an EventBridge Pipe into a list of stream records.
/// Handles both a single record object and a batched JSON array; malformed JSON yields an empty
/// list (so a poison message drains rather than redriving forever).
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

    fn image(video_id: &str, status: &str) -> Value {
        json!({
            "video_id": {"S": video_id},
            "status": {"S": status},
            "channel_id": {"S": "chan-1"},
            "created_at": {"S": "2026-06-15T00:00:00Z"}
        })
    }

    fn record(event: &str, new_image: Option<Value>, old_image: Option<Value>) -> Value {
        let mut dynamodb = serde_json::Map::new();
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

    #[test]
    fn soft_delete_modify_triggers_cleanup() {
        let rec = record(
            "MODIFY",
            Some(image("v1", "deleted")),
            Some(image("v1", "published")),
        );
        assert_eq!(decide(&rec), CleanupAction::Cleanup("v1".into()));
    }

    #[test]
    fn published_modify_skips() {
        // An ordinary publish/edit must never trigger cleanup.
        let rec = record(
            "MODIFY",
            Some(image("v1", "published")),
            Some(image("v1", "processing")),
        );
        assert_eq!(decide(&rec), CleanupAction::Skip);
    }

    #[test]
    fn insert_draft_skips() {
        let rec = record("INSERT", Some(image("v1", "draft")), None);
        assert_eq!(decide(&rec), CleanupAction::Skip);
    }

    #[test]
    fn processing_skips() {
        let rec = record("MODIFY", Some(image("v1", "processing")), None);
        assert_eq!(decide(&rec), CleanupAction::Skip);
    }

    #[test]
    fn remove_skips_no_new_image() {
        // The finalizer hard-delete: no NewImage, cleanup already ran at soft-delete time.
        let rec = record("REMOVE", None, Some(image("v9", "deleted")));
        assert_eq!(decide(&rec), CleanupAction::Skip);
    }

    #[test]
    fn deleted_without_video_id_skips() {
        let rec = json!({
            "eventName": "MODIFY",
            "dynamodb": { "NewImage": { "status": {"S": "deleted"} } }
        });
        assert_eq!(decide(&rec), CleanupAction::Skip);
    }

    #[test]
    fn missing_dynamodb_skips() {
        assert_eq!(
            decide(&json!({ "eventName": "MODIFY" })),
            CleanupAction::Skip
        );
    }

    #[test]
    fn parse_pipe_message_single_object() {
        let body = json!({"eventName": "MODIFY", "dynamodb": {}}).to_string();
        assert_eq!(parse_pipe_message(&body).len(), 1);
    }

    #[test]
    fn parse_pipe_message_batched_array() {
        let body = json!([
            {"eventName": "MODIFY", "dynamodb": {}},
            {"eventName": "INSERT", "dynamodb": {}}
        ])
        .to_string();
        assert_eq!(parse_pipe_message(&body).len(), 2);
    }

    #[test]
    fn parse_pipe_message_invalid_is_empty() {
        assert!(parse_pipe_message("not json {{{").is_empty());
    }
}
