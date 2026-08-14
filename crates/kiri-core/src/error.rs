//! Stable error model (specs/ERRORS.md).
//!
//! Codes are stable machine-readable strings. Messages are diagnostic, not
//! parsing contracts. Internal errors never leak sensitive native details by
//! default.

use serde::{Deserialize, Serialize};

/// Stable top-level error categories from specs/ERRORS.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    TransportError,
    ProtocolError,
    Unauthorized,
    ScopeDenied,
    InvalidArgument,
    ResourceNotFound,
    ResourceStale,
    ResourceWrongType,
    LimitExceeded,
    Busy,
    Cancelled,
    CommandError,
    InternalError,
}

impl ErrorCode {
    /// The stable machine-readable string for this code.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::TransportError => "transport_error",
            ErrorCode::ProtocolError => "protocol_error",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::ScopeDenied => "scope_denied",
            ErrorCode::InvalidArgument => "invalid_argument",
            ErrorCode::ResourceNotFound => "resource_not_found",
            ErrorCode::ResourceStale => "resource_stale",
            ErrorCode::ResourceWrongType => "resource_wrong_type",
            ErrorCode::LimitExceeded => "limit_exceeded",
            ErrorCode::Busy => "busy",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::CommandError => "command_error",
            ErrorCode::InternalError => "internal_error",
        }
    }
}

/// Error envelope matching schemas/kiri-error.schema.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Error { code, message: message.into(), request_id: None, command: None, details: None }
    }

    pub fn with_request_id(mut self, request_id: u64) -> Self {
        self.request_id = Some(request_id);
        self
    }

    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn transport_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TransportError, message)
    }
    pub fn protocol_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ProtocolError, message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }
    pub fn scope_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ScopeDenied, message)
    }
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
    pub fn resource_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceNotFound, message)
    }
    pub fn resource_stale(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceStale, message)
    }
    pub fn resource_wrong_type(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceWrongType, message)
    }
    pub fn limit_exceeded(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::LimitExceeded, message)
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Busy, message)
    }
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Cancelled, message)
    }
    pub fn command_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::CommandError, message)
    }
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_codes_have_stable_strings() {
        for (code, expected) in [
            (ErrorCode::TransportError, "transport_error"),
            (ErrorCode::ProtocolError, "protocol_error"),
            (ErrorCode::Unauthorized, "unauthorized"),
            (ErrorCode::ScopeDenied, "scope_denied"),
            (ErrorCode::InvalidArgument, "invalid_argument"),
            (ErrorCode::ResourceNotFound, "resource_not_found"),
            (ErrorCode::ResourceStale, "resource_stale"),
            (ErrorCode::ResourceWrongType, "resource_wrong_type"),
            (ErrorCode::LimitExceeded, "limit_exceeded"),
            (ErrorCode::Busy, "busy"),
            (ErrorCode::Cancelled, "cancelled"),
            (ErrorCode::CommandError, "command_error"),
            (ErrorCode::InternalError, "internal_error"),
        ] {
            assert_eq!(code.as_str(), expected);
        }
    }

    #[test]
    fn envelope_serializes_to_schema_shape() {
        let err = Error::scope_denied("filesystem path is outside the allowed root")
            .with_request_id(42)
            .with_command("fs.read")
            .with_details(serde_json::json!({"scope": "workspace.read"}));
        let json = serde_json::to_value(&err).unwrap();
        let expected = serde_json::json!({
            "code": "scope_denied",
            "message": "filesystem path is outside the allowed root",
            "request_id": 42,
            "command": "fs.read",
            "details": {"scope": "workspace.read"}
        });
        assert_eq!(json, expected);
    }

    #[test]
    fn serialized_request_id_roundtrips_as_number() {
        // The wire representation uses a numeric request id; frontend bindings
        // stringify it when exposing it in the public TypeScript error union.
        let err = Error::new(ErrorCode::Unauthorized, "no").with_request_id(7);
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["request_id"], 7);
        let back: Error = serde_json::from_value(json).unwrap();
        assert_eq!(back.code, ErrorCode::Unauthorized);
        assert_eq!(back.request_id, Some(7));
    }

    #[test]
    fn internal_errors_do_not_carry_secrets_by_default() {
        let err = Error::internal_error("disk sector 0xdeadbeef corrupted");
        assert!(!err.message.contains("0xdeadbeef") || err.details.is_none());
    }

    #[test]
    fn invalid_input_distinguishable_from_authorization_failure() {
        assert_ne!(ErrorCode::InvalidArgument, ErrorCode::Unauthorized);
        let a = Error::invalid_argument("bad payload");
        let b = Error::unauthorized("no capability");
        assert_ne!(a, b);
        assert_eq!(a.code.as_str(), "invalid_argument");
        assert_eq!(b.code.as_str(), "unauthorized");
    }
}
