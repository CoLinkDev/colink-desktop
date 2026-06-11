use std::io;

use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

const CODE_INVALID_REFRESH_TOKEN: i32 = 1020;
const CODE_REFRESH_TOKEN_REVOKED: i32 = 1021;
const CODE_UNAUTHORIZED: i32 = 1030;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),
    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("{message}")]
    Protocol { code: i32, message: String },
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("tauri error: {0}")]
    Tauri(#[from] tauri::Error),
    #[error("crypto error: {0}")]
    Crypto(String),
}

impl AppError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn protocol(code: i32, message: impl Into<String>) -> Self {
        Self::Protocol {
            code,
            message: message.into(),
        }
    }

    pub fn is_auth_protocol_code(code: i32) -> bool {
        matches!(
            code,
            CODE_INVALID_REFRESH_TOKEN | CODE_REFRESH_TOKEN_REVOKED | CODE_UNAUTHORIZED
        )
    }
}
