use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::{error::AppError, state::AppState};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub uid: i64,
    pub name: String,
    pub role: String,
    pub exp: usize,
    pub iat: usize,
}

pub fn hash_password(pwd: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    Ok(Argon2::default()
        .hash_password(pwd.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(pwd: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(pwd.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub fn issue_token(
    secret: &str,
    ttl_hours: i64,
    uid: i64,
    name: &str,
    role: &str,
) -> Result<String, AppError> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        uid,
        name: name.to_string(),
        role: role.to_string(),
        iat: now as usize,
        exp: (now + ttl_hours * 3600) as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::internal(format!("签发 token 失败: {e}")))
}

/// 登录态提取器：所有业务接口用它拿到当前操作人
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub name: String,
    pub role: String,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?
            .to_string();

        let data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(state.cfg.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized)?;

        Ok(AuthUser {
            id: data.claims.uid,
            name: data.claims.name,
            role: data.claims.role,
        })
    }
}

/// 管理员校验
pub fn require_admin(user: &AuthUser) -> Result<(), AppError> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}