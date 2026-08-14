//! Physical wire envelope for WebView2's ordinary web messaging transport.
//!
//! The logical `ControlHeader` fields map onto JSON members here because
//! `window.chrome.webview.postMessage` carries JSON values. This is the
//! documented WebView2 physical transport (wv2-interop source); the logical
//! protocol stays transport-independent (docs/04-ipc-strategy.md).

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::header::{ControlFlags, MAGIC, PROTOCOL_VERSION};

/// `magic` is the 4-byte tag `KRI1`. On the JSON physical transport it is
/// carried as the 4-character string `"KRI1"` (what `window.chrome.webview
/// .postMessage` and the wry IPC handler actually emit), not as a numeric
/// array. These adapters keep the internal `[u8; 4]` representation while
/// mapping to and from that string form, so a bridge message round-trips.
mod magic_serde {
    use serde::de::{Error as _, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S: Serializer>(magic: &[u8; 4], s: S) -> Result<S::Ok, S::Error> {
        let as_str = std::str::from_utf8(magic).unwrap_or("\0\0\0\0");
        s.serialize_str(as_str)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 4], D::Error> {
        struct MagicVisitor;
        impl<'de> Visitor<'de> for MagicVisitor {
            type Value = [u8; 4];

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a 4-byte magic as a 4-character string or a 4-element number array")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.len() != 4 {
                    return Err(E::custom(format!("magic must be 4 bytes, got {}", v.len())));
                }
                let mut out = [0u8; 4];
                out.copy_from_slice(v.as_bytes());
                Ok(out)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = [0u8; 4];
                for slot in out.iter_mut() {
                    *slot = seq
                        .next_element::<u8>()?
                        .ok_or_else(|| A::Error::custom("magic array shorter than 4 bytes"))?;
                }
                Ok(out)
            }
        }
        d.deserialize_any(MagicVisitor)
    }
}

/// Request as carried by the physical transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireRequest {
    #[serde(with = "magic_serde")]
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
    #[serde(with = "magic_serde")]
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
