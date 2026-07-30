//! SQS consumer that keeps OpenSearch in sync with the `videos` table. Messages arrive from an
//! EventBridge Pipe whose source is the videos DynamoDB stream.
//!
//! The loop mirrors the transcode service's consumer. Processing is idempotent (upsert/delete by
//! `video_id`), so at-least-once delivery and redrive are safe. A message is only deleted from the
//! queue after it is fully processed; on a processing error it is left for SQS redrive/DLQ.

use std::time::Duration;

use aws_sdk_sqs::Client as SqsClient;
use shared::error::AppError;

use crate::indexer::{self, IndexAction};
use crate::repo;
use crate::signing::SearchClient;

/// Run the consumer loop forever. Spawned from `main` only when a queue is configured.
pub async fn run(sqs: SqsClient, queue_url: String, client: SearchClient) {
    tracing::info!(queue_url = %queue_url, "starting search stream consumer loop");

    loop {
        match poll_once(&sqs, &queue_url, &client).await {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(count = n, "processed stream messages");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "search consumer error");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// One receive → process → delete cycle. Returns the number of messages received.
/// Exposed (not just the infinite loop) so integration tests can drive a single cycle.
pub async fn poll_once(
    sqs: &SqsClient,
    queue_url: &str,
    client: &SearchClient,
) -> Result<usize, AppError> {
    let resp = sqs
        .receive_message()
        .queue_url(queue_url)
        .max_number_of_messages(10)
        .wait_time_seconds(20)
        .send()
        .await
        .map_err(AppError::internal)?;

    let messages = resp.messages();
    let count = messages.len();

    for msg in messages {
        let body = msg.body().unwrap_or("");
        let receipt = msg.receipt_handle().unwrap_or_default().to_string();

        match process_message(client, body).await {
            Ok(()) => {
                // Only delete after successful processing.
                if let Err(e) = sqs
                    .delete_message()
                    .queue_url(queue_url)
                    .receipt_handle(&receipt)
                    .send()
                    .await
                {
                    tracing::error!(error = %e, "failed to delete processed message");
                }
            }
            Err(e) => {
                // Leave the message on the queue for redrive / DLQ. Idempotency makes the retry safe.
                tracing::error!(error = %e, "failed to process stream message; leaving for redrive");
            }
        }
    }

    Ok(count)
}

/// Process a single SQS message body, which may contain one stream record or a batched array.
/// A body that parses to zero records (including malformed JSON) is a no-op success, so poison
/// messages are drained rather than redriven forever.
pub async fn process_message(client: &SearchClient, body: &str) -> Result<(), AppError> {
    let records = indexer::parse_pipe_message(body);
    for record in &records {
        apply_action(client, indexer::decide_action(record)).await?;
    }
    Ok(())
}

/// Apply a single decided action against OpenSearch.
pub async fn apply_action(client: &SearchClient, action: IndexAction) -> Result<(), AppError> {
    match action {
        IndexAction::Upsert(doc) => {
            tracing::info!(video_id = %doc.video_id, "indexing video");
            repo::index_video(client, &doc).await
        }
        IndexAction::Delete(video_id) => {
            tracing::info!(video_id = %video_id, "removing video from index");
            repo::delete_video_doc(client, &video_id).await
        }
        IndexAction::Skip => Ok(()),
    }
}
