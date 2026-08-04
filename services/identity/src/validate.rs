//! Pure request validation for the auth endpoints — no AWS, no state, so every rule unit-tests
//! directly and the handlers stay thin I/O.
//!
//! `invite_code` and `email` are both DynamoDB **key** values (the `invite_codes` hash key and the
//! users `email-index` key condition), so a blank one is rejected here rather than at the SDK, which
//! answers an empty string with a `ValidationException` — a client mistake that would otherwise
//! surface as a 500 (DESIGN §10.10) and let an unauthenticated caller drive the identity 5xx alarm.

use shared::error::AppError;
use shared::validate::{key_field, non_empty};

use crate::models::{LoginRequest, RegisterRequest};

/// Minimum password length, enforced on both registration and password change.
pub const MIN_PASSWORD_LEN: usize = 8;

pub fn register(req: &RegisterRequest) -> Result<(), AppError> {
    // Invite code first: it is the platform's real gate, and validating it before anything else
    // keeps a rejected registration from touching the users table at all.
    key_field("invite_code", &req.invite_code)?;
    key_field("email", &req.email)?;
    non_empty("display_name", &req.display_name)?;
    password(&req.password)
}

pub fn login(req: &LoginRequest) -> Result<(), AppError> {
    key_field("email", &req.email)?;
    // Password is compared against a stored hash, never used as a key — presence is enough. An empty
    // password still needs to reach the argon2 verify so a blank submission is a 401 (bad
    // credentials) rather than a 400 that would confirm the account exists.
    Ok(())
}

/// Enforce the password length floor. Shared by registration and `/change-password` so the two can't
/// drift apart.
pub fn password(password: &str) -> Result<(), AppError> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn valid_register() -> RegisterRequest {
        RegisterRequest {
            email: "someone@example.com".to_string(),
            password: "correct horse".to_string(),
            display_name: "Someone".to_string(),
            invite_code: "ABC123".to_string(),
        }
    }

    fn valid_login() -> LoginRequest {
        LoginRequest {
            email: "someone@example.com".to_string(),
            password: "correct horse".to_string(),
        }
    }

    #[test]
    fn register_accepts_a_well_formed_request() {
        assert!(register(&valid_register()).is_ok());
    }

    #[test]
    fn register_rejects_an_empty_invite_code_with_400() {
        // The exact request shape observed producing a 500.
        let req = RegisterRequest {
            invite_code: String::new(),
            ..valid_register()
        };
        let err = register(&req).unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn register_rejects_a_blank_invite_code() {
        let req = RegisterRequest {
            invite_code: "   ".to_string(),
            ..valid_register()
        };
        assert!(register(&req).is_err());
    }

    #[test]
    fn register_rejects_an_empty_email_with_400() {
        let req = RegisterRequest {
            email: String::new(),
            ..valid_register()
        };
        let err = register(&req).unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn register_rejects_an_empty_display_name() {
        let req = RegisterRequest {
            display_name: String::new(),
            ..valid_register()
        };
        assert!(register(&req).is_err());
    }

    #[test]
    fn register_rejects_a_short_password() {
        let req = RegisterRequest {
            password: "short".to_string(),
            ..valid_register()
        };
        let err = register(&req).unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn register_checks_the_invite_code_before_the_email() {
        // Ordering matters: a bad invite must short-circuit before the users table is read.
        let req = RegisterRequest {
            invite_code: String::new(),
            email: String::new(),
            ..valid_register()
        };
        assert_eq!(
            register(&req).unwrap_err().to_string(),
            "bad request: invite_code is required"
        );
    }

    #[test]
    fn register_rejects_an_oversized_email() {
        let req = RegisterRequest {
            email: format!("{}@example.com", "a".repeat(shared::validate::MAX_KEY_LEN)),
            ..valid_register()
        };
        assert!(register(&req).is_err());
    }

    #[test]
    fn login_accepts_a_well_formed_request() {
        assert!(login(&valid_login()).is_ok());
    }

    #[test]
    fn login_rejects_an_empty_email_with_400() {
        let req = LoginRequest {
            email: String::new(),
            ..valid_login()
        };
        let err = login(&req).unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn login_allows_an_empty_password_so_it_fails_as_401_not_400() {
        // A 400 here would distinguish "no password supplied" from "wrong password" and, combined
        // with the email lookup, leak whether the account exists.
        let req = LoginRequest {
            password: String::new(),
            ..valid_login()
        };
        assert!(login(&req).is_ok());
    }

    #[test]
    fn password_boundary_is_inclusive() {
        assert!(password(&"a".repeat(MIN_PASSWORD_LEN)).is_ok());
        assert!(password(&"a".repeat(MIN_PASSWORD_LEN - 1)).is_err());
    }
}
