//! Logical control-plane header (docs/04-ipc-strategy.md, specs/IPC.md).
//!
//! This `repr(C)` struct is the *logical* envelope. Physical transports may
//! represent the same fields as JSON members, typed arrays, or native
//! structures. It is not a claim that these bytes cross every browser
//! boundary unchanged.

use serde::{Deserialize, Serialize};

/// "KRI1"
pub const MAGIC: [u8; 4] = *b"KRI1";
/// Current logical protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

/// Control-plane flag bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlFlags(pub u16);

impl ControlFlags {
    pub const REQUEST: ControlFlags = ControlFlags(0x0001);
    pub const RESPONSE: ControlFlags = ControlFlags(0x0002);
    pub const ERROR: ControlFlags = ControlFlags(0x0004);
    pub const BULK: ControlFlags = ControlFlags(0x0008);

    pub const fn empty() -> Self {
        ControlFlags(0)
    }

    pub const fn new(bits: u16) -> Self {
        ControlFlags(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, flag: ControlFlags) -> bool {
        self.0 & flag.0 != 0
    }
}

/// Logical control header (docs/04-ipc-strategy.md).
///
/// Field order is chosen so the `repr(C)` struct packs to exactly 32 bytes
/// with natural alignment: the `u64` request id is placed at offset 8.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHeader {
    pub magic: [u8; 4],
    pub version: u16,
    pub flags: u16,
    pub request_id: u64,
    pub command_id: u32,
    pub payload_len: u32,
    pub codec: u16,
    pub reserved: u16,
    pub resource_count: u32,
}

impl ControlHeader {
    pub const SIZE: usize = std::mem::size_of::<ControlHeader>();

    pub fn request(command_id: u32, request_id: u64, payload_len: u32, codec: u16) -> Self {
        ControlHeader {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            flags: ControlFlags::REQUEST.bits(),
            command_id,
            request_id,
            payload_len,
            codec,
            reserved: 0,
            resource_count: 0,
        }
    }

    pub fn response(request_id: u64, payload_len: u32, codec: u16) -> Self {
        ControlHeader {
            magic: MAGIC,
            version: PROTOCOL_VERSION,
            flags: ControlFlags::RESPONSE.bits(),
            command_id: 0,
            request_id,
            payload_len,
            codec,
            reserved: 0,
            resource_count: 0,
        }
    }

    /// Validate magic and protocol version. Length validation against the
    /// actual representation happens in `validate::RequestValidator`.
    pub fn validate(&self) -> Result<(), String> {
        if self.magic != MAGIC {
            return Err(format!("bad magic {:02x?} (expected KRI1)", self.magic));
        }
        if self.version != PROTOCOL_VERSION {
            return Err(format!(
                "unsupported protocol version {} (supported {})",
                self.version, PROTOCOL_VERSION
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_is_repr_c_and_stable() {
        // C layout: magic(4) version(2) flags(2) request_id(8) command_id(4)
        // payload_len(4) codec(2) reserved(2) resource_count(4) = 32 bytes.
        // The u64 request_id sits at offset 8 so no padding is required.
        assert_eq!(ControlHeader::SIZE, 32);
        let h = ControlHeader::request(17, 99, 1234, 1);
        let bytes = unsafe {
            std::slice::from_raw_parts(&h as *const ControlHeader as *const u8, ControlHeader::SIZE)
        };
        assert_eq!(&bytes[0..4], b"KRI1");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1);
        assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 99);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 17);
    }

    #[test]
    fn validate_accepts_valid_header() {
        let h = ControlHeader::request(1, 2, 0, 0);
        assert!(h.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_magic() {
        let mut h = ControlHeader::request(1, 2, 0, 0);
        h.magic = *b"NOPE";
        assert!(h.validate().unwrap_err().contains("bad magic"));
    }

    #[test]
    fn validate_rejects_unknown_version() {
        let mut h = ControlHeader::request(1, 2, 0, 0);
        h.version = 999;
        assert!(h.validate().unwrap_err().contains("unsupported protocol version"));
    }

    #[test]
    fn flags_roundtrip() {
        let f = ControlFlags::new(ControlFlags::REQUEST.bits() | ControlFlags::BULK.bits());
        assert!(f.contains(ControlFlags::REQUEST));
        assert!(f.contains(ControlFlags::BULK));
        assert!(!f.contains(ControlFlags::ERROR));
    }
}
