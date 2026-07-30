use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;

#[derive(Clone)]
pub struct AppState {
    pub db: DynamoClient,
    pub s3: S3Client,
    pub sqs: SqsClient,
    pub bucket: String,
    pub queue_url: String,
}
