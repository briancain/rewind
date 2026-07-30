use crate::signing::SearchClient;

#[derive(Clone)]
pub struct AppState {
    pub client: SearchClient,
    /// SQS client + stream-events queue URL. Present only when `STREAM_QUEUE_URL` is configured
    /// (cloud); `None` in local/HTTP-only mode so the consumer does not run.
    pub sqs: Option<aws_sdk_sqs::Client>,
    pub stream_queue_url: Option<String>,
}
