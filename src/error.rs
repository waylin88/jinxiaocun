use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// 统一错误类型：所有 handler 返回 `AppResult<T> = Result<T, AppError>`
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("登录已失效，请重新登录")]
    Unauthorized,

    #[error("没有权限执行此操作")]
    Forbidden,

    #[error("{0}不存在")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("内部错误: {0}")]
    Internal(String),
}

impl AppError {
    fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden => "forbidden",
            AppError::NotFound(_) => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::Database(_) => "database",
            AppError::Config(_) => "config",
            AppError::Internal(_) => "internal",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // 数据库错误只在服务端日志里输出细节，客户端只拿到统一文案，避免泄漏 schema
        if let AppError::Database(ref e) = self {
            tracing::error!(error = %e, "数据库错误");
        }
        let body = json!({
            "ok": false,
            "code": self.code(),
            "message": self.to_string(),
        });
        (self.status(), Json(body)).into_response()
    }
}

impl From<sqlx::migrate::MigrateError> for AppError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        AppError::Internal(format!("数据库迁移失败: {e}"))
    }
}

impl From<argon2::password_hash::Error> for AppError {
    fn from(e: argon2::password_hash::Error) -> Self {
        AppError::Internal(format!("密码哈希错误: {e}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(format!("IO 错误: {e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// 便捷构造
impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        AppError::BadRequest(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        AppError::NotFound(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        AppError::Conflict(msg.into())
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        AppError::Internal(msg.into())
    }

    /// 用户名或密码错误时，对外统一说"登录失效"，不暴露哪个字段错了
    pub fn clone_for_login(&self) -> Self {
        AppError::Unauthorized
    }
}