use aws_sdk_dynamodb::Client as DynamoClient;

#[derive(Clone)]
pub struct AppState {
    pub db: DynamoClient,
}
