use aws_sdk_dynamodb::{
    types::{AttributeValue, DeleteRequest, WriteRequest},
    Client,
};
use std::collections::HashMap;
use std::time::Duration;

use crate::config::ServiceConfig;

pub async fn create_client(config: &ServiceConfig) -> Client {
    let shared = crate::aws::base_config().await;
    let mut builder = aws_sdk_dynamodb::config::Builder::from(&shared);
    if let Some(endpoint) = &config.dynamodb_endpoint {
        builder = builder.endpoint_url(endpoint);
    }
    Client::from_conf(builder.build())
}

pub async fn put_item(
    client: &Client,
    table: &str,
    item: HashMap<String, AttributeValue>,
) -> Result<(), aws_sdk_dynamodb::Error> {
    client
        .put_item()
        .table_name(table)
        .set_item(Some(item))
        .send()
        .await?;
    Ok(())
}

pub async fn get_item(
    client: &Client,
    table: &str,
    key: HashMap<String, AttributeValue>,
) -> Result<Option<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
    let resp = client
        .get_item()
        .table_name(table)
        .set_key(Some(key))
        .send()
        .await?;
    Ok(resp.item)
}

pub async fn delete_item(
    client: &Client,
    table: &str,
    key: HashMap<String, AttributeValue>,
) -> Result<(), aws_sdk_dynamodb::Error> {
    client
        .delete_item()
        .table_name(table)
        .set_key(Some(key))
        .send()
        .await?;
    Ok(())
}

pub async fn query_by_index(
    client: &Client,
    table: &str,
    index: &str,
    key_name: &str,
    key_value: AttributeValue,
) -> Result<Vec<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
    let resp = client
        .query()
        .table_name(table)
        .index_name(index)
        .key_condition_expression("#k = :v")
        .expression_attribute_names("#k", key_name)
        .expression_attribute_values(":v", key_value)
        .send()
        .await?;
    Ok(resp.items.unwrap_or_default())
}

/// Scan an entire table, following pagination until exhausted. Intended for low-frequency
/// administrative operations (e.g. search index backfill), not hot paths.
pub async fn scan_all(
    client: &Client,
    table: &str,
) -> Result<Vec<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
    let mut items = Vec::new();
    let mut last_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let resp = client
            .scan()
            .table_name(table)
            .set_exclusive_start_key(last_key.take())
            .send()
            .await?;

        if let Some(batch) = resp.items {
            items.extend(batch);
        }

        match resp.last_evaluated_key {
            Some(k) if !k.is_empty() => last_key = Some(k),
            _ => break,
        }
    }

    Ok(items)
}

/// DynamoDB's `BatchWriteItem` accepts at most 25 requests per call.
const BATCH_WRITE_MAX: usize = 25;
/// Bounded retries for `UnprocessedItems` (partial success under throttling). Generous because the
/// caller is idempotent — leftover items after this are re-handled on the next at-least-once redrive.
const BATCH_DELETE_MAX_ATTEMPTS: usize = 5;

/// Split a vector into chunks of at most `size`. Pure (no I/O) so the chunking boundaries are
/// unit-testable independently of any AWS call.
pub fn into_batches<T>(mut items: Vec<T>, size: usize) -> Vec<Vec<T>> {
    assert!(size > 0, "batch size must be positive");
    let mut batches = Vec::new();
    while !items.is_empty() {
        let take = size.min(items.len());
        batches.push(items.drain(..take).collect());
    }
    batches
}

/// Delete many items by primary key via `BatchWriteItem`, chunking into the 25-per-call limit and
/// retrying any `UnprocessedItems` with a small backoff. Each key map is a full primary key (a hash
/// key, or hash + range). Deleting an absent key is a no-op, so this is safe to retry / re-run —
/// intended for the idempotent cascade-cleanup of a video's dependent rows.
pub async fn batch_delete(
    client: &Client,
    table: &str,
    keys: Vec<HashMap<String, AttributeValue>>,
) -> Result<(), aws_sdk_dynamodb::Error> {
    for chunk in into_batches(keys, BATCH_WRITE_MAX) {
        let mut requests: Vec<WriteRequest> = chunk
            .into_iter()
            .map(|key| {
                WriteRequest::builder()
                    .delete_request(
                        DeleteRequest::builder()
                            .set_key(Some(key))
                            .build()
                            .expect("delete request key is set"),
                    )
                    .build()
            })
            .collect();

        let mut attempt = 0;
        while !requests.is_empty() {
            let resp = client
                .batch_write_item()
                .request_items(table, requests.clone())
                .send()
                .await?;

            requests = resp
                .unprocessed_items()
                .and_then(|m| m.get(table))
                .cloned()
                .unwrap_or_default();

            if requests.is_empty() {
                break;
            }

            attempt += 1;
            if attempt >= BATCH_DELETE_MAX_ATTEMPTS {
                tracing::warn!(
                    table,
                    remaining = requests.len(),
                    "batch_delete left unprocessed items after retries; a later redrive will reclaim them"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(50 * attempt as u64)).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::into_batches;

    #[test]
    fn into_batches_splits_on_boundary() {
        let batches = into_batches((0..50).collect::<Vec<_>>(), 25);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 25);
        assert_eq!(batches[1].len(), 25);
    }

    #[test]
    fn into_batches_handles_remainder() {
        let batches = into_batches((0..26).collect::<Vec<_>>(), 25);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 25);
        assert_eq!(batches[1], vec![25]);
    }

    #[test]
    fn into_batches_smaller_than_size_is_single_chunk() {
        let batches = into_batches(vec![1, 2, 3], 25);
        assert_eq!(batches, vec![vec![1, 2, 3]]);
    }

    #[test]
    fn into_batches_empty_is_empty() {
        let batches: Vec<Vec<i32>> = into_batches(Vec::new(), 25);
        assert!(batches.is_empty());
    }

    #[test]
    fn into_batches_preserves_all_items_in_order() {
        let batches = into_batches((0..7).collect::<Vec<_>>(), 3);
        let flat: Vec<_> = batches.into_iter().flatten().collect();
        assert_eq!(flat, (0..7).collect::<Vec<_>>());
    }
}
