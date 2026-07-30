use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn setup() -> axum::Router {
    // Point at DynamoDB Local
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    std::env::set_var("DISABLE_SES", "1");
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", "us-west-2");
    std::env::set_var("TABLE_PREFIX", "test_");

    let config = shared::config::ServiceConfig::from_env("identity");
    let db = shared::dynamo::create_client(&config).await;

    // Drop this suite's tables first so its schema always wins over a divergent one left in the
    // shared DynamoDB Local by another suite — notably the sessions `user-id-index` GSI that
    // change-password relies on, which other suites create test_sessions without (mirrors social).
    for t in [
        "test_users",
        "test_sessions",
        "test_verification_tokens",
        "test_invite_codes",
    ] {
        let _ = db.delete_table().table_name(t).send().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Create tables (ignore errors if they already exist)
    let _ = db
        .create_table()
        .table_name("test_users")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("user_id")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("user_id")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("email")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .global_secondary_indexes(
            aws_sdk_dynamodb::types::GlobalSecondaryIndex::builder()
                .index_name("email-index")
                .key_schema(
                    aws_sdk_dynamodb::types::KeySchemaElement::builder()
                        .attribute_name("email")
                        .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .projection(
                    aws_sdk_dynamodb::types::Projection::builder()
                        .projection_type(aws_sdk_dynamodb::types::ProjectionType::All)
                        .build(),
                )
                .provisioned_throughput(
                    aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                        .read_capacity_units(5)
                        .write_capacity_units(5)
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    let _ = db
        .create_table()
        .table_name("test_sessions")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("session_token")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("session_token")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("user_id")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .global_secondary_indexes(
            aws_sdk_dynamodb::types::GlobalSecondaryIndex::builder()
                .index_name("user-id-index")
                .key_schema(
                    aws_sdk_dynamodb::types::KeySchemaElement::builder()
                        .attribute_name("user_id")
                        .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .projection(
                    aws_sdk_dynamodb::types::Projection::builder()
                        .projection_type(aws_sdk_dynamodb::types::ProjectionType::KeysOnly)
                        .build(),
                )
                .provisioned_throughput(
                    aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                        .read_capacity_units(5)
                        .write_capacity_units(5)
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    let _ = db
        .create_table()
        .table_name("test_verification_tokens")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("token")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("token")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    let _ = db
        .create_table()
        .table_name("test_invite_codes")
        .key_schema(
            aws_sdk_dynamodb::types::KeySchemaElement::builder()
                .attribute_name("code")
                .key_type(aws_sdk_dynamodb::types::KeyType::Hash)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            aws_sdk_dynamodb::types::AttributeDefinition::builder()
                .attribute_name("code")
                .attribute_type(aws_sdk_dynamodb::types::ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .provisioned_throughput(
            aws_sdk_dynamodb::types::ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .unwrap(),
        )
        .send()
        .await;

    let state = identity::state::AppState {
        db,
        ses: None,
        base_url: "http://localhost:3001".to_string(),
        from_email: "test@example.com".to_string(),
    };

    axum::Router::new()
        .route(
            "/register",
            axum::routing::post(identity::handlers::register),
        )
        .route(
            "/verify",
            axum::routing::get(identity::handlers::verify_email),
        )
        .route("/login", axum::routing::post(identity::handlers::login))
        .route("/logout", axum::routing::post(identity::handlers::logout))
        .route(
            "/change-password",
            axum::routing::post(identity::handlers::change_password),
        )
        .route("/me", axum::routing::get(identity::handlers::me))
        .with_state(state)
}

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn seed_invite_code(code: &str) {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://localhost:8000")
        .region(aws_config::Region::new("us-west-2"))
        .load()
        .await;
    let db = aws_sdk_dynamodb::Client::new(&config);
    let _ = db
        .put_item()
        .table_name("test_invite_codes")
        .item(
            "code",
            aws_sdk_dynamodb::types::AttributeValue::S(code.to_string()),
        )
        .item("used", aws_sdk_dynamodb::types::AttributeValue::Bool(false))
        .item(
            "created_at",
            aws_sdk_dynamodb::types::AttributeValue::S("2026-01-01T00:00:00Z".to_string()),
        )
        .send()
        .await;
}

/// Insert a session row directly with a chosen `ttl` (epoch seconds). Used to exercise app-level
/// session-expiry enforcement (the DynamoDB-side TTL sweep is an AWS-managed background process and
/// is not exercisable in tests).
async fn seed_session(token: &str, user_id: &str, ttl_epoch: i64) {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://localhost:8000")
        .region(aws_config::Region::new("us-west-2"))
        .load()
        .await;
    let db = aws_sdk_dynamodb::Client::new(&config);
    let _ = db
        .put_item()
        .table_name("test_sessions")
        .item(
            "session_token",
            aws_sdk_dynamodb::types::AttributeValue::S(token.to_string()),
        )
        .item(
            "user_id",
            aws_sdk_dynamodb::types::AttributeValue::S(user_id.to_string()),
        )
        .item(
            "ttl",
            aws_sdk_dynamodb::types::AttributeValue::N(ttl_epoch.to_string()),
        )
        .send()
        .await;
}

#[tokio::test]
async fn full_auth_flow() {
    let app = setup().await;
    let email = format!("test-{}@example.com", uuid::Uuid::new_v4());
    let invite = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite).await;

    // 1. Register
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "securepass123",
                        "display_name": "Test User",
                        "invite_code": invite
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    let token = body["token"].as_str().unwrap().to_string();
    let user_id = body["user_id"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    assert!(!user_id.is_empty());

    // 2. Get /me with session token
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["email"], email);
    assert_eq!(body["display_name"], "Test User");
    assert_eq!(body["email_verified"], false);

    // 3. Verify email (get token from DDB directly)
    std::env::set_var("DYNAMODB_ENDPOINT", "http://localhost:8000");
    let config = shared::config::ServiceConfig::from_env("identity");
    let db = shared::dynamo::create_client(&config).await;

    let tokens = db
        .scan()
        .table_name("test_verification_tokens")
        .send()
        .await
        .unwrap();
    let verify_token = tokens
        .items()
        .iter()
        .find(|item| {
            item.get("user_id")
                .and_then(|v| v.as_s().ok())
                .map(|s| s == &user_id)
                .unwrap_or(false)
        })
        .and_then(|item| item.get("token").and_then(|v| v.as_s().ok()).cloned())
        .expect("verification token not found");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/verify?token={}", verify_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // 4. Confirm email_verified is now true
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_json(resp).await;
    assert_eq!(body["email_verified"], true);

    // 5. Logout
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // 6. /me should now fail
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 7. Login again
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "securepass123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert!(!body["token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn register_duplicate_email_fails() {
    let app = setup().await;
    let email = format!("dup-{}@example.com", uuid::Uuid::new_v4());
    let invite1 = format!("TEST-{}", uuid::Uuid::new_v4());
    let invite2 = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite1).await;
    seed_invite_code(&invite2).await;

    let body = json!({
        "email": email,
        "password": "pass1234",
        "display_name": "User",
        "invite_code": invite1
    })
    .to_string();

    // First register succeeds
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second register with same email fails
    let body2 = json!({
        "email": email,
        "password": "pass1234",
        "display_name": "User",
        "invite_code": invite2
    })
    .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(body2))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn login_wrong_password_fails() {
    let app = setup().await;
    let email = format!("wrong-{}@example.com", uuid::Uuid::new_v4());
    let invite = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite).await;

    // Register
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "correct1",
                        "display_name": "User",
                        "invite_code": invite
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Login with wrong password
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "wrong"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invite_code_not_consumed_on_duplicate_email() {
    let app = setup().await;
    let email = format!("dupe-invite-{}@example.com", uuid::Uuid::new_v4());
    let invite1 = format!("TEST-{}", uuid::Uuid::new_v4());
    let invite2 = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite1).await;
    seed_invite_code(&invite2).await;

    // Register first user successfully
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "pass1234",
                        "display_name": "User1",
                        "invite_code": invite1
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Try to register with same email but different invite code — should fail
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "pass4567",
                        "display_name": "User2",
                        "invite_code": invite2
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // invite2 should still be usable (not consumed by the failed registration)
    let new_email = format!("fresh-{}@example.com", uuid::Uuid::new_v4());
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": new_email,
                        "password": "pass7890",
                        "display_name": "User3",
                        "invite_code": invite2
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn register_rejects_short_password() {
    let app = setup().await;
    let invite = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "email": "short@example.com",
                        "password": "1234567",
                        "display_name": "User",
                        "invite_code": invite
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let app = setup().await;
    let token = format!("expired-{}", uuid::Uuid::new_v4());
    // ttl one hour in the past.
    let past = chrono::Utc::now().timestamp() - 3600;
    seed_session(&token, "some-user", past).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unexpired_session_is_accepted() {
    let app = setup().await;

    // Create a real user so /me can resolve a profile, then attach a session with a future ttl.
    let invite = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite).await;
    let email = format!("ttl-ok-{}@example.com", uuid::Uuid::new_v4());
    let reg = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email": email, "password": "securepass123", "display_name": "TTL OK", "invite_code": invite}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let user_id = body_json(reg).await["user_id"]
        .as_str()
        .unwrap()
        .to_string();

    let token = format!("valid-{}", uuid::Uuid::new_v4());
    let future = chrono::Utc::now().timestamp() + 3600;
    seed_session(&token, &user_id, future).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// --- change-password helpers + tests ---

async fn register(app: &axum::Router, email: &str, password: &str) -> (String, String) {
    let invite = format!("TEST-{}", uuid::Uuid::new_v4());
    seed_invite_code(&invite).await;
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"email": email, "password": password, "display_name": "U", "invite_code": invite})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let b = body_json(resp).await;
    (
        b["token"].as_str().unwrap().to_string(),
        b["user_id"].as_str().unwrap().to_string(),
    )
}

async fn post_json(
    app: &axum::Router,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {}", t));
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn me_status(app: &axum::Router, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header("authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn change_password_success() {
    let app = setup().await;
    let email = format!("cp-ok-{}@example.com", uuid::Uuid::new_v4());
    let (token, _uid) = register(&app, &email, "oldpass123").await;

    let resp = post_json(
        &app,
        "/change-password",
        Some(&token),
        json!({"current_password": "oldpass123", "new_password": "newpass456"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Old password no longer works; new one does.
    let old = post_json(
        &app,
        "/login",
        None,
        json!({"email": email, "password": "oldpass123"}),
    )
    .await;
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
    let new = post_json(
        &app,
        "/login",
        None,
        json!({"email": email, "password": "newpass456"}),
    )
    .await;
    assert_eq!(new.status(), StatusCode::OK);

    // The session used to change the password stays valid.
    assert_eq!(me_status(&app, &token).await, StatusCode::OK);
}

#[tokio::test]
async fn change_password_wrong_current_rejected() {
    let app = setup().await;
    let email = format!("cp-wrong-{}@example.com", uuid::Uuid::new_v4());
    let (token, _uid) = register(&app, &email, "oldpass123").await;

    let resp = post_json(
        &app,
        "/change-password",
        Some(&token),
        json!({"current_password": "WRONGPASS", "new_password": "newpass456"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Password must be unchanged: the original still logs in.
    let still = post_json(
        &app,
        "/login",
        None,
        json!({"email": email, "password": "oldpass123"}),
    )
    .await;
    assert_eq!(still.status(), StatusCode::OK);
}

#[tokio::test]
async fn change_password_rejects_short_new() {
    let app = setup().await;
    let email = format!("cp-short-{}@example.com", uuid::Uuid::new_v4());
    let (token, _uid) = register(&app, &email, "oldpass123").await;

    let resp = post_json(
        &app,
        "/change-password",
        Some(&token),
        json!({"current_password": "oldpass123", "new_password": "short"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn change_password_invalidates_other_sessions() {
    let app = setup().await;
    let email = format!("cp-sessions-{}@example.com", uuid::Uuid::new_v4());
    // Session A from registration.
    let (token_a, _uid) = register(&app, &email, "oldpass123").await;

    // Session B from a second login.
    let b = post_json(
        &app,
        "/login",
        None,
        json!({"email": email, "password": "oldpass123"}),
    )
    .await;
    assert_eq!(b.status(), StatusCode::OK);
    let token_b = body_json(b).await["token"].as_str().unwrap().to_string();
    assert_eq!(me_status(&app, &token_b).await, StatusCode::OK);

    // Change password using session A.
    let resp = post_json(
        &app,
        "/change-password",
        Some(&token_a),
        json!({"current_password": "oldpass123", "new_password": "newpass456"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Current session (A) survives; the other session (B) is invalidated.
    assert_eq!(me_status(&app, &token_a).await, StatusCode::OK);
    assert_eq!(me_status(&app, &token_b).await, StatusCode::UNAUTHORIZED);
}
