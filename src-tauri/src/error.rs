use std::io;

use serde::Serialize;
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
    #[error("request failed with status {status}")]
    HttpStatus { status: reqwest::StatusCode },
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

    pub fn http_status(status: reqwest::StatusCode) -> Self {
        Self::HttpStatus { status }
    }

    pub fn is_http_status(&self, expected: reqwest::StatusCode) -> bool {
        matches!(self, Self::HttpStatus { status } if *status == expected)
    }

    pub fn is_auth_protocol_code(code: i32) -> bool {
        matches!(
            code,
            CODE_INVALID_REFRESH_TOKEN | CODE_REFRESH_TOKEN_REVOKED | CODE_UNAUTHORIZED
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: String,
    pub code: Option<i32>,
    pub status: Option<u16>,
    pub message: String,
}

impl From<AppError> for CommandError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::Network(error) => Self {
                kind: "network".to_string(),
                code: None,
                status: error.status().map(|status| status.as_u16()),
                message: error.to_string(),
            },
            AppError::HttpStatus { status } => Self {
                kind: "http".to_string(),
                code: None,
                status: Some(status.as_u16()),
                message: format!("request failed with status {status}"),
            },
            AppError::Protocol { code, message } => Self {
                kind: "protocol".to_string(),
                code: Some(code),
                status: None,
                message,
            },
            error => Self {
                kind: "application".to_string(),
                code: None,
                status: None,
                message: error.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::{AppError, CommandError};

    #[test]
    fn command_error_preserves_protocol_code() {
        let error = CommandError::from(AppError::protocol(1010, "invalid credentials"));

        assert_eq!(error.kind, "protocol");
        assert_eq!(error.code, Some(1010));
        assert_eq!(error.status, None);
    }

    #[test]
    fn command_error_preserves_http_status() {
        let error = CommandError::from(AppError::http_status(StatusCode::NOT_FOUND));

        assert_eq!(error.kind, "http");
        assert_eq!(error.code, None);
        assert_eq!(error.status, Some(404));
    }

    #[test]
    fn auth_protocol_codes_are_explicit() {
        assert!(AppError::is_auth_protocol_code(1020));
        assert!(AppError::is_auth_protocol_code(1021));
        assert!(AppError::is_auth_protocol_code(1030));
        assert!(!AppError::is_auth_protocol_code(1010));
        assert!(!AppError::is_auth_protocol_code(2001));
    }
}
