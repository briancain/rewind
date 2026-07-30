//! SQS consumer that reclaims a deleted video's dependent data. Messages arrive from an
//! EventBridge Pipe whose source is the `videos` DynamoDB stream, filtered to soft-deletes
//! (`status == "deleted"`).
//!
//! Mirrors the search service's consumer: receive -> process -> delete-on-success, leaving a
//! message for SQS redrive/DLQ on error. Processing is idempotent (cleanup by `video_id`), so
//! at-least-once delivery and retries are safe.

use std::time::Duration;

use shared::error::AppError;

use crate::cleanup;
use crate::decider::{self, CleanupAction};
use crate::state::AppState;

/// Run the consumer loop forever. Spawned from `main` only when a queue is configured.
pub async fn run(state: AppState) {
    tracing::info!(queue_url = %state.queue_url, "starting delete-cleanup consumer loop");
    loop {
        match poll_once(&state).await {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(count = n, "processed cleanup messages");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "delete-cleanup consumer error");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// One receive -> process -> delete cycle. Returns the number of messages received. Exposed (not
/// just `run`) so integration tests can drive a single cycle.
pub async fn poll_once(state: &AppState) -> Result<usize, AppError> {
    let resp = state
        .sqs
        .receive_message()
        .queue_url(&state.queue_url)
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

        match process_message(state, body).await {
            Ok(()) => {
                if let Err(e) = state
                    .sqs
                    .delete_message()
                    .queue_url(&state.queue_url)
                    .receipt_handle(&receipt)
                    .send()
                    .await
                {
                    tracing::error!(error = %e, "failed to delete processed message");
                }
            }
            Err(e) => {
                // Leave on the queue for redrive / DLQ. Idempotency makes the retry safe.
                tracing::error!(error = %e, "failed to process cleanup message; leaving for redrive");
            }
        }
    }

    Ok(count)
}

/// Process one SQS message body: one stream record or a batched array. A body that yields no
/// actionable records (including malformed JSON) is a no-op success, so poison messages drain
/// rather than redriving forever.
pub async fn process_message(state: &AppState, body: &str) -> Result<(), AppError> {
    for record in decider::parse_pipe_message(body) {
        if let CleanupAction::Cleanup(video_id) = decider::decide(&record) {
            cleanup::cleanup_video(state, &video_id).await?;
        }
    }
    Ok(())
}
