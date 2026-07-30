use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_ses::Client as SesClient;

#[derive(Clone)]
pub struct AppState {
    pub db: DynamoClient,
    pub ses: Option<SesClient>,
    pub base_url: String,
    pub from_email: String,
}
