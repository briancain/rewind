use aws_sdk_cloudfront::Client as CloudFrontClient;
use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;

/// Everything the cleanup worker needs: the data stores it reclaims from (DynamoDB + S3), the SQS
/// queue it drains, and the two bucket names whose `{kind}/{video_id}/` prefixes it deletes.
#[derive(Clone)]
pub struct AppState {
    pub db: DynamoClient,
    pub s3: S3Client,
    pub sqs: SqsClient,
    /// The `delete-cleanup` FIFO queue (videos stream -> EventBridge Pipe -> this queue).
    pub queue_url: String,
    /// Bucket holding `hls/{id}/`, `mp4/{id}/`, `thumbnails/{id}/`.
    pub video_bucket: String,
    /// Bucket holding the raw upload at `raw/{id}/`.
    pub raw_bucket: String,
    /// CloudFront client + content-CDN distribution id for edge invalidation. `None` when
    /// `CDN_DISTRIBUTION_ID` is unset (local dev, or a fresh bootstrap before the `cdn` stack
    /// exists) — invalidation is then skipped. Both are `Some`/`None` together.
    pub cloudfront: Option<CloudFrontClient>,
    pub cdn_distribution_id: Option<String>,
}
