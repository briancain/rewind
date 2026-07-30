use tracing_subscriber::{fmt, EnvFilter};

pub fn init(service_name: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .json()
        .with_current_span(true)
        .init();

    tracing::info!(service = service_name, "tracing initialized");
}
