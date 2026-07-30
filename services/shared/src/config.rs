use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ServiceConfig {
    pub port: u16,
    pub service_name: String,
    pub dynamodb_endpoint: Option<String>,
}

impl ServiceConfig {
    pub fn from_env(service_name: &str) -> Self {
        Self {
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .expect("PORT must be a number"),
            service_name: service_name.to_string(),
            dynamodb_endpoint: std::env::var("DYNAMODB_ENDPOINT").ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `from_env` reads process-global env vars (PORT, DYNAMODB_ENDPOINT), which are shared across
    // all test threads. Serialize the env-mutating tests with a lock so they are parallel-safe and
    // don't depend on `--test-threads=1`. (`unwrap_or_else(into_inner)` tolerates a poisoned lock
    // from an earlier panicking test so it doesn't cascade and hide the real failure.)
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("PORT");
        std::env::remove_var("DYNAMODB_ENDPOINT");
        let config = ServiceConfig::from_env("test-svc");
        assert_eq!(config.port, 3000);
        assert_eq!(config.service_name, "test-svc");
        assert!(config.dynamodb_endpoint.is_none());
    }

    #[test]
    fn from_env_custom_port() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("PORT", "8080");
        let config = ServiceConfig::from_env("identity");
        assert_eq!(config.port, 8080);
        std::env::remove_var("PORT");
    }

    #[test]
    fn from_env_with_dynamo_endpoint() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
        let config = ServiceConfig::from_env("svc");
        assert_eq!(config.dynamodb_endpoint.unwrap(), "http://localhost:8000");
        std::env::remove_var("DYNAMODB_ENDPOINT");
    }
}
