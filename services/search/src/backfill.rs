//! Backfill / reindex support: scans the `videos` table (source of truth) and reconciles the
//! OpenSearch index to match. Used to seed a freshly deployed region, recover from index loss, or
//! reindex after a mapping change. Reuses the same eligibility rule as
//! the stream consumer via `indexer::decide_action_for_item`.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use serde::Serialize;
use serde_json::{json, Value};
use shared::error::AppError;

use crate::indexer::{self, IndexAction};
use crate::repo;
use crate::signing::SearchClient;

/// Convert a single DynamoDB SDK `AttributeValue` into attribute-value JSON (e.g. `{"S": "x"}`),
/// so scanned items can be fed to the same indexer decision logic the stream consumer uses.
pub fn attr_to_json(av: &AttributeValue) -> Value {
    match av {
        AttributeValue::S(s) => json!({ "S": s }),
        AttributeValue::N(n) => json!({ "N": n }),
        AttributeValue::Bool(b) => json!({ "BOOL": b }),
        AttributeValue::Null(_) => json!({ "NULL": true }),
        AttributeValue::L(l) => json!({ "L": l.iter().map(attr_to_json).collect::<Vec<_>>() }),
        AttributeValue::M(m) => json!({ "M": map_to_json(m) }),
        // Other types (B, SS, NS, BS) are unused by the videos table.
        _ => Value::Null,
    }
}

/// Convert a DynamoDB item (attribute map) into attribute-value JSON.
pub fn map_to_json(item: &HashMap<String, AttributeValue>) -> Value {
    Value::Object(
        item.iter()
            .map(|(k, v)| (k.clone(), attr_to_json(v)))
            .collect(),
    )
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct BackfillReport {
    pub scanned: usize,
    pub indexed: usize,
    pub deleted: usize,
    pub skipped: usize,
}

/// Scan the videos table and reconcile OpenSearch: upsert public+published videos, delete the
/// rest (deletes are idempotent and harmless when the doc is already absent, e.g. seeding a fresh
/// index). Returns a summary report.
pub async fn reindex(
    db: &aws_sdk_dynamodb::Client,
    client: &SearchClient,
) -> Result<BackfillReport, AppError> {
    let items = shared::dynamo::scan_all(db, &shared::tables::table("videos")).await?;

    let mut report = BackfillReport {
        scanned: items.len(),
        ..Default::default()
    };

    for item in &items {
        let json_item = map_to_json(item);
        match indexer::decide_action_for_item(&json_item) {
            IndexAction::Upsert(doc) => {
                repo::index_video(client, &doc).await?;
                report.indexed += 1;
            }
            IndexAction::Delete(id) => {
                repo::delete_video_doc(client, &id).await?;
                report.deleted += 1;
            }
            IndexAction::Skip => report.skipped += 1,
        }
    }

    tracing::info!(
        scanned = report.scanned,
        indexed = report.indexed,
        deleted = report.deleted,
        skipped = report.skipped,
        "reindex backfill complete"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_to_json_string() {
        assert_eq!(
            attr_to_json(&AttributeValue::S("hi".into())),
            json!({"S": "hi"})
        );
    }

    #[test]
    fn attr_to_json_list_of_strings() {
        let av = AttributeValue::L(vec![
            AttributeValue::S("a".into()),
            AttributeValue::S("b".into()),
        ]);
        assert_eq!(attr_to_json(&av), json!({"L": [{"S": "a"}, {"S": "b"}]}));
    }

    #[test]
    fn map_to_json_roundtrips_into_decide_action() {
        // A published+public item should be converted such that the indexer upserts it.
        let mut item = HashMap::new();
        item.insert("video_id".to_string(), AttributeValue::S("v1".into()));
        item.insert("status".to_string(), AttributeValue::S("published".into()));
        item.insert("visibility".to_string(), AttributeValue::S("public".into()));
        item.insert("title".to_string(), AttributeValue::S("T".into()));
        item.insert(
            "tags".to_string(),
            AttributeValue::L(vec![AttributeValue::S("rust".into())]),
        );

        let json_item = map_to_json(&item);
        match indexer::decide_action_for_item(&json_item) {
            IndexAction::Upsert(doc) => {
                assert_eq!(doc.video_id, "v1");
                assert_eq!(doc.tags, vec!["rust"]);
            }
            other => panic!("expected Upsert, got {:?}", other),
        }
    }

    #[test]
    fn map_to_json_private_item_deletes() {
        let mut item = HashMap::new();
        item.insert("video_id".to_string(), AttributeValue::S("v2".into()));
        item.insert("status".to_string(), AttributeValue::S("published".into()));
        item.insert(
            "visibility".to_string(),
            AttributeValue::S("private".into()),
        );
        let json_item = map_to_json(&item);
        assert_eq!(
            indexer::decide_action_for_item(&json_item),
            IndexAction::Delete("v2".into())
        );
    }
}
