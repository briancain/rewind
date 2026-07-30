use axum::{
    routing::{get, post},
    Router,
};
use identity::{handlers, state::AppState};
use shared::config::ServiceConfig;

#[tokio::main]
async fn main() {
    // Admin CLI: `identity hash-password` reads a password from STDIN, prints an argon2 hash, and
    // exits. Pure compute (no AWS, no server). Reading from stdin (not argv) keeps the password out
    // of shell history and the process list. Used by scripts/admin-reset-password.sh.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("hash-password") {
        use std::io::Read;
        let mut pw = String::new();
        if std::io::stdin().read_to_string(&mut pw).is_err() {
            eprintln!("failed to read password from stdin");
            std::process::exit(2);
        }
        // Strip a single trailing newline (from echo/heredoc); leave other characters intact.
        let pw = pw.strip_suffix('\n').unwrap_or(&pw);
        let pw = pw.strip_suffix('\r').unwrap_or(pw);
        if pw.is_empty() {
            eprintln!("usage: printf '%s' '<password>' | identity hash-password");
            std::process::exit(2);
        }
        match identity::password::hash_password(pw) {
            Ok(hash) => {
                println!("{hash}");
                return;
            }
            Err(e) => {
                eprintln!("hash error: {e}");
                std::process::exit(1);
            }
        }
    }

    let config = ServiceConfig::from_env("identity");
    shared::tracing_setup::init(&config.service_name);

    let db = shared::dynamo::create_client(&config).await;

    let aws_config = shared::aws::base_config().await;
    let ses = if std::env::var("DISABLE_SES").is_ok() {
        None
    } else {
        Some(aws_sdk_ses::Client::new(&aws_config))
    };

    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());
    let from_email =
        std::env::var("FROM_EMAIL").unwrap_or_else(|_| "noreply@localhost".to_string());

    let state = AppState {
        db,
        ses,
        base_url,
        from_email,
    };

    let app = Router::new()
        .route("/health", get(shared::health::health_check))
        .route("/register", post(handlers::register))
        .route("/verify", get(handlers::verify_email))
        .route("/login", post(handlers::login))
        .route("/logout", post(handlers::logout))
        .route("/change-password", post(handlers::change_password))
        .route("/me", get(handlers::me))
        .route("/users/{id}", get(handlers::get_user))
        .with_state(state)
        .layer(shared::cors::permissive());

    let app = shared::middleware::with_logging(app);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_returns_200() {
        let app = Router::new().route("/health", get(shared::health::health_check));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
