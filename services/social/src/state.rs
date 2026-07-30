use aws_sdk_dynamodb::Client;

#[derive(Clone)]
pub struct AppState {
    pub db: Client,
}
