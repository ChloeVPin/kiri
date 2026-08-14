//! Physical wire envelope for WebView2's ordinary web messaging transport.
//!
//! The logical `ControlHeader` fields map onto JSON members here because
//! `window.chrome.webview.postMessage` carries JSON values. This is the
//! documented WebView2 physical transport (wv2-interop source); the logical
//! protocol stays transport-independent (docs/04-ipc-strategy.md).

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::header::{ControlFlags, MAGIC, PROTOCOL_VERSION};

/// Request as carried by the physical transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireRequest {
    pub magic: [u8; 4],
    pub version: u16,
    pub flags: u16,
    pub command_id: u32,
    pub request_id: u64,
    pub payload_len: u32,
    pub codec: u16,
    pub payload: serde_json::Value,
}

impl WireRequest {
    pub fn new(command_id: u32, request_id: u64, codec: u16, payload: serde_json::Value) -> Self {
        let payload_len = serde_json::to_vec(&payload).unwrap_or_default().len() as u32;
        WireRequest {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            flags: ControlFlags::REQUEST.bits(),
            command_id,
            request_id,
            payload_len,
            codec,
            payload,
        }
    }
}

/// Response as carried by the physical transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireResponse {
    pub magic: [u8; 4],
    pub version: u16,
    pub flags: u16,
    pub request_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

impl WireResponse {
    pub fn ok(request_id: u64, payload: serde_json::Value) -> Self {
        WireResponse {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            flags: ControlFlags::RESPONSE.bits(),
            request_id,
            payload: Some(payload),
            error: None,
        }
    }

    pub fn err(request_id: u64, error: Error) -> Self {
        WireResponse {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            flags: ControlFlags::RESPONSE.bits() | ControlFlags::ERROR.bits(),
            request_id,
            payload: None,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_request_roundtrip() {
        let req = WireRequest::new(17, 42, 1, serde_json::json!({"id": 5}));
        let json = serde_json::to_string(&req).unwrap();
        let back: WireRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
        assert_eq!(back.payload_len, req.payload_len);
    }

    #[test]
    fn wire_response_roundtrip() {
        let resp = WireResponse::ok(42, serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&resp).unwrap();
        let back: WireResponse = serde_json::from_str(&json).unwrap();
        assert!(back.error.is_none());
        assert_eq!(back.payload, Some(serde_json::json!({"ok": true})));

        let err = WireResponse::err(43, crate::error::Error::unauthorized("nope"));
        let json = serde_json::to_string(&err).unwrap();
        let back: WireResponse = serde_json::from_str(&json).unwrap();
        assert!(back.payload.is_none());
        assert_eq!(back.error.unwrap().code, crate::error::ErrorCode::Unauthorized);
    }
}
