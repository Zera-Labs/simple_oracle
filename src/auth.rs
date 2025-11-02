use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use rocket::http::Status;
use rocket::outcome::Outcome;
use rocket::request::{FromRequest, Outcome as RequestOutcome, Request};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::errors::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    Admin,
    Reader,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub exp: usize,
}

#[derive(Debug)]
pub struct AuthUser {
    pub subject: String,
    pub role: Role,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AuthUser {
    type Error = AppError;
    async fn from_request(req: &'r Request<'_>) -> RequestOutcome<Self, Self::Error> {
        let auth = req.headers().get_one("Authorization");
        if let Some(bearer) = auth.and_then(|h| h.strip_prefix("Bearer ")) {
            let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".into());
            let mut validation = Validation::new(Algorithm::HS256);
            validation.validate_exp = true;
            match decode::<Claims>(
                bearer,
                &DecodingKey::from_secret(secret.as_bytes()),
                &validation,
            ) {
                Ok(data) => {
                    let role = match data.claims.role.as_str() {
                        "admin" | "Admin" => Role::Admin,
                        _ => Role::Reader,
                    };
                    return Outcome::Success(AuthUser {
                        subject: data.claims.sub,
                        role,
                    });
                }
                Err(_) => return Outcome::Error((Status::Unauthorized, AppError::Unauthorized)),
            }
        }
        Outcome::Success(AuthUser {
            subject: "public".into(),
            role: Role::Reader,
        })
    }
}

impl AuthUser {
    pub fn require_admin(&self) -> AppResult<()> {
        if self.role != Role::Admin {
            return Err(AppError::Forbidden);
        }
        Ok(())
    }
}

// Optional ed25519 body signature verification
pub fn verify_detached_signature(
    _body: &[u8],
    _signature_b64: Option<&str>,
    _pubkey_b58: Option<&str>,
) -> AppResult<()> {
    // Placeholder: wire in ed25519-dalek + bs58 if desired
    Ok(())
}
