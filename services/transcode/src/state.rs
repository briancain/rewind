use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_mediaconvert::Client as MediaConvertClient;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;

#[derive(Clone)]
pub struct AppState {
    pub db: DynamoClient,
    pub sqs: SqsClient,
    pub s3: S3Client,
    pub mediaconvert: Option<MediaConvertClient>,
    pub queue_url: String,
    pub output_bucket: String,
    pub mediaconvert_role: String,
    pub cdn_base_url: String,
    /// SQS queue carrying MediaConvert completion events (EventBridge → SQS). When `None` (local
    /// dev), the completion consumer is not started.
    pub completion_queue_url: Option<String>,
}
