//! Shared-memory slot arena for the zero-copy through-webview IPC spike.
//!
//! Pure layout and framing logic with no platform dependencies: the Windows
//! host maps a WebView2 `ICoreWebView2SharedBuffer` onto these helpers, and
//! the injected bench script mirrors the same constants in JavaScript. All
//! multi-byte fields are little-endian.
//!
//! Arena layout:
//! - `GLOBAL_HEADER_BYTES` global header at offset 0: magic `KRIR`, layout
//!   version, slot count, per-slot byte stride, payload capacity.
//! - `SLOT_COUNT` slots, each `SLOT_BYTES` long: a `SLOT_HEADER_BYTES` slot
//!   header (`KRSL` magic, state byte, flags byte, codec, seq, request_id,
//!   command_id, payload_len) followed by `SLOT_PAYLOAD_CAP` payload bytes.
//!
//! Ownership: the page is the only allocator of a slot (free -> request);
//! the host is the only writer of the response phase (request -> response);
//! the page returns the slot to free. A slot carries one request and then
//! its response, so request and reply share the same index and no separate
//! reply arena is needed.

use kiri_core::caller::CallerId;
use kiri_core::zc_ipc_gate::ZcIpcGrant;

/// Global header magic: "KRIR" little-endian.
pub const GLOBAL_MAGIC: u32 = u32::from_le_bytes(*b"KRIR");
/// Slot header magic: "KRSL" little-endian.
pub const SLOT_MAGIC: u32 = u32::from_le_bytes(*b"KRSL");
/// Layout version of the arena described here.
pub const RING_VERSION: u32 = 1;
/// Bytes reserved for the global header at offset 0.
pub const GLOBAL_HEADER_BYTES: usize = 64;
/// Per-slot header size.
pub const SLOT_HEADER_BYTES: usize = 32;
/// Payload capacity per slot; matches the 1 MiB control-payload ceiling.
pub const SLOT_PAYLOAD_CAP: usize = 1024 * 1024;
/// Total bytes per slot (header + payload region).
pub const SLOT_BYTES: usize = SLOT_HEADER_BYTES + SLOT_PAYLOAD_CAP;
/// Number of slots in the arena.
pub const SLOT_COUNT: u32 = 16;
/// Total shared-buffer size the host allocates once.
pub const RING_BUFFER_BYTES: u64 = (GLOBAL_HEADER_BYTES + SLOT_COUNT as usize * SLOT_BYTES) as u64;

/// Slot is unclaimed; the page may take it.
pub const STATE_FREE: u8 = 0;
/// Page wrote a complete request; the host may read it.
pub const STATE_REQUEST: u8 = 1;
/// Host wrote a complete response; the page may read it.
pub const STATE_RESPONSE: u8 = 2;

/// Request payload region holds the JSON encoding of the payload value.
pub const CODEC_JSON: u16 = 1;
/// Request payload region holds raw UTF-8 bytes of a string payload value.
pub const CODEC_UTF8_STRING: u16 = 2;

/// Response payload region holds the serialized `WireResponse` JSON.
pub const RESP_JSON: u8 = 0;
/// Response payload region holds the raw bytes of the echoed string for a
/// `kiri.ping` response (`{pong:true, echo:<string>}`) so the page can rebuild
/// the payload without parsing a megabyte of JSON.
pub const RESP_ECHO_STRING: u8 = 1;

// Slot header field offsets (all little-endian).
const OFF_MAGIC: usize = 0;
const OFF_STATE: usize = 4;
const OFF_FLAGS: usize = 5;
const OFF_CODEC: usize = 6;
const OFF_SEQ: usize = 8;
const OFF_REQUEST_ID: usize = 16;
const OFF_COMMAND_ID: usize = 24;
const OFF_PAYLOAD_LEN: usize = 28;

/// Byte offset of slot `index` inside the arena buffer.
pub fn slot_offset(index: u32) -> usize {
    GLOBAL_HEADER_BYTES + index as usize * SLOT_BYTES
}

/// Bounds-checked mutable view of one slot inside the arena buffer.
pub fn slot_mut(buf: &mut [u8], index: u32) -> Option<&mut [u8]> {
    if index >= SLOT_COUNT {
        return None;
    }
    let start = slot_offset(index);
    buf.get_mut(start..start + SLOT_BYTES)
}

/// Immutable view of one slot inside the arena buffer.
pub fn slot_ref(buf: &[u8], index: u32) -> Option<&[u8]> {
    if index >= SLOT_COUNT {
        return None;
    }
    let start = slot_offset(index);
    buf.get(start..start + SLOT_BYTES)
}

/// Write the global header describing the arena. Callers must post the buffer
/// to script only after this returns Ok so the page can validate the layout.
pub fn write_global_header(buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < GLOBAL_HEADER_BYTES {
        return Err("buffer smaller than global header");
    }
    buf[0..4].copy_from_slice(&GLOBAL_MAGIC.to_le_bytes());
    buf[4..8].copy_from_slice(&RING_VERSION.to_le_bytes());
    buf[8..12].copy_from_slice(&SLOT_COUNT.to_le_bytes());
    buf[12..16].copy_from_slice(&(SLOT_BYTES as u32).to_le_bytes());
    buf[16..20].copy_from_slice(&(SLOT_PAYLOAD_CAP as u32).to_le_bytes());
    Ok(())
}

/// A request read out of a slot header. The payload bytes stay in the slot;
/// callers read `payload(slot)` with `payload_len`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRequest {
    pub seq: u64,
    pub request_id: u64,
    pub command_id: u32,
    pub codec: u16,
    pub payload_len: u32,
}

fn put_u16(slot: &mut [u8], off: usize, v: u16) {
    slot[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(slot: &mut [u8], off: usize, v: u32) {
    slot[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(slot: &mut [u8], off: usize, v: u64) {
    slot[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn get_u16(slot: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(slot[off..off + 2].try_into().unwrap())
}
fn get_u32(slot: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(slot[off..off + 4].try_into().unwrap())
}
fn get_u64(slot: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(slot[off..off + 8].try_into().unwrap())
}

/// Current state byte of a slot.
pub fn slot_state(slot: &[u8]) -> u8 {
    slot[OFF_STATE]
}

/// Payload bytes declared by the slot header (bounds-checked against the
/// payload region).
pub fn slot_payload(slot: &[u8]) -> Option<&[u8]> {
    let len = get_u32(slot, OFF_PAYLOAD_LEN) as usize;
    if len > SLOT_PAYLOAD_CAP {
        return None;
    }
    slot.get(SLOT_HEADER_BYTES..SLOT_HEADER_BYTES + len)
}

/// Page-side claim: mark a free slot as owned-in-progress by leaving state
/// FREE until `publish_request` flips it. Returns false if the slot is not
/// free. This helper only validates; the JS side performs the same check.
pub fn claim_slot(slot: &[u8]) -> bool {
    slot_state(slot) == STATE_FREE
}

/// Page-side publish: write a request into a previously claimed slot. The
/// state byte is written last so the host never observes a half-written
/// request (paired with the magic/seq checks on the read side).
pub fn publish_request(
    slot: &mut [u8],
    seq: u64,
    request_id: u64,
    command_id: u32,
    codec: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
    if slot.len() < SLOT_BYTES {
        return Err("slot view too small");
    }
    if payload.len() > SLOT_PAYLOAD_CAP {
        return Err("payload exceeds slot capacity");
    }
    if !claim_slot(slot) {
        return Err("slot is not free");
    }
    slot[SLOT_HEADER_BYTES..SLOT_HEADER_BYTES + payload.len()].copy_from_slice(payload);
    put_u32(slot, OFF_MAGIC, SLOT_MAGIC);
    slot[OFF_FLAGS] = 0;
    put_u16(slot, OFF_CODEC, codec);
    put_u64(slot, OFF_SEQ, seq);
    put_u64(slot, OFF_REQUEST_ID, request_id);
    put_u32(slot, OFF_COMMAND_ID, command_id);
    put_u32(slot, OFF_PAYLOAD_LEN, payload.len() as u32);
    // Release-store equivalent: the control message posted after this write
    // is the synchronization point; the state byte must be the final store.
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    slot[OFF_STATE] = STATE_REQUEST;
    Ok(())
}

/// Host-side read of a request slot. Fails unless the slot holds a complete
/// request (magic valid, state REQUEST, payload_len within capacity).
pub fn read_request(slot: &[u8]) -> Result<SlotRequest, &'static str> {
    if slot.len() < SLOT_BYTES {
        return Err("slot view too small");
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    if get_u32(slot, OFF_MAGIC) != SLOT_MAGIC {
        return Err("bad slot magic");
    }
    if slot_state(slot) != STATE_REQUEST {
        return Err("slot is not in request state");
    }
    let payload_len = get_u32(slot, OFF_PAYLOAD_LEN);
    if payload_len as usize > SLOT_PAYLOAD_CAP {
        return Err("declared payload_len exceeds slot capacity");
    }
    Ok(SlotRequest {
        seq: get_u64(slot, OFF_SEQ),
        request_id: get_u64(slot, OFF_REQUEST_ID),
        command_id: get_u32(slot, OFF_COMMAND_ID),
        codec: get_u16(slot, OFF_CODEC),
        payload_len,
    })
}

/// Host-side publish: write a response into the same slot the request came
/// from. `flags` selects the payload encoding (RESP_JSON or
/// RESP_ECHO_STRING). The state byte is written last.
pub fn publish_response(
    slot: &mut [u8],
    seq: u64,
    request_id: u64,
    flags: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    if slot.len() < SLOT_BYTES {
        return Err("slot view too small");
    }
    if payload.len() > SLOT_PAYLOAD_CAP {
        return Err("response exceeds slot capacity");
    }
    slot[SLOT_HEADER_BYTES..SLOT_HEADER_BYTES + payload.len()].copy_from_slice(payload);
    put_u64(slot, OFF_SEQ, seq);
    put_u64(slot, OFF_REQUEST_ID, request_id);
    slot[OFF_FLAGS] = flags;
    put_u32(slot, OFF_PAYLOAD_LEN, payload.len() as u32);
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    slot[OFF_STATE] = STATE_RESPONSE;
    Ok(())
}

/// Host-side reply-leg gate for the shared-slot write. The host may publish
/// a response into the arena only while a live [`ZcIpcGrant`] bound to
/// `caller` + `command_id` releases the reply bytes. A denied request
/// carries no grant, and an expired or misbound grant fails closed the same
/// way: the slot must not be written and the reply takes the ordinary JSON
/// wire. This is the same authority check the T008 one-shot shared-buffer
/// post applies, so both shared-memory reply legs accept one currency.
pub fn ring_reply_authorized(
    grant: Option<&ZcIpcGrant>,
    caller: CallerId,
    command_id: u32,
    body: &[u8],
    now_ns: u64,
) -> bool {
    grant.and_then(|g| g.reply_bytes(caller, command_id, body, now_ns)).is_some()
}

/// Page-side read of a response slot: (flags, payload_len) if the slot holds
/// a complete response for `request_id`.
pub fn read_response_header(slot: &[u8], request_id: u64) -> Result<(u8, u32), &'static str> {
    if slot.len() < SLOT_BYTES {
        return Err("slot view too small");
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    if get_u32(slot, OFF_MAGIC) != SLOT_MAGIC {
        return Err("bad slot magic");
    }
    if slot_state(slot) != STATE_RESPONSE {
        return Err("slot is not in response state");
    }
    if get_u64(slot, OFF_REQUEST_ID) != request_id {
        return Err("response request_id mismatch");
    }
    Ok((slot[OFF_FLAGS], get_u32(slot, OFF_PAYLOAD_LEN)))
}

/// Page-side release: return the slot to the free pool.
pub fn release_slot(slot: &mut [u8]) {
    slot[OFF_STATE] = STATE_FREE;
}

/// Borrow the raw bytes of the echoed string for a `kiri.ping` response so a
/// binary reply frame can carry them verbatim (RESP_ECHO_STRING) instead of a
/// JSON encoding the page would have to parse. Shared by every ring-flavored
/// reply leg (Windows slot publish, protocol_ring frame).
pub fn response_echo_bytes(response: &kiri_core::wire::WireResponse) -> Option<&[u8]> {
    if response.error.is_some() {
        return None;
    }
    let serde_json::Value::Object(map) = response.payload.as_ref()? else {
        return None;
    };
    if map.get("pong") != Some(&serde_json::Value::Bool(true)) {
        return None;
    }
    match map.get("echo") {
        Some(serde_json::Value::String(s)) => Some(s.as_bytes()),
        _ => None,
    }
}

/// Read one request frame carried on a byte channel. Same header fields and
/// checks as `read_request`, but the buffer is exactly one frame (header +
/// declared payload) rather than a padded slot inside an arena. Used by
/// transports that carry slot framing over a request/response channel
/// instead of shared memory (protocol_ring).
pub fn read_request_frame(buf: &[u8]) -> Result<(SlotRequest, &[u8]), &'static str> {
    if buf.len() < SLOT_HEADER_BYTES {
        return Err("frame smaller than slot header");
    }
    if get_u32(buf, OFF_MAGIC) != SLOT_MAGIC {
        return Err("bad slot magic");
    }
    if buf[OFF_STATE] != STATE_REQUEST {
        return Err("frame is not in request state");
    }
    let payload_len = get_u32(buf, OFF_PAYLOAD_LEN);
    if payload_len as usize > SLOT_PAYLOAD_CAP {
        return Err("declared payload_len exceeds slot capacity");
    }
    if buf.len() != SLOT_HEADER_BYTES + payload_len as usize {
        return Err("frame length does not match declared payload_len");
    }
    Ok((
        SlotRequest {
            seq: get_u64(buf, OFF_SEQ),
            request_id: get_u64(buf, OFF_REQUEST_ID),
            command_id: get_u32(buf, OFF_COMMAND_ID),
            codec: get_u16(buf, OFF_CODEC),
            payload_len,
        },
        &buf[SLOT_HEADER_BYTES..],
    ))
}

/// Encode one response frame (header + payload) into a fresh buffer. Same
/// fields and write order as `publish_response`, but sized to the payload
/// instead of a padded arena slot. Used by byte-channel reply legs
/// (protocol_ring); the `flags` byte selects RESP_JSON or RESP_ECHO_STRING.
pub fn encode_response_frame(
    seq: u64,
    request_id: u64,
    flags: u8,
    payload: &[u8],
) -> Result<Vec<u8>, &'static str> {
    if payload.len() > SLOT_PAYLOAD_CAP {
        return Err("response exceeds slot capacity");
    }
    let mut buf = vec![0u8; SLOT_HEADER_BYTES + payload.len()];
    buf[SLOT_HEADER_BYTES..].copy_from_slice(payload);
    put_u32(&mut buf, OFF_MAGIC, SLOT_MAGIC);
    buf[OFF_STATE] = STATE_RESPONSE;
    buf[OFF_FLAGS] = flags;
    put_u64(&mut buf, OFF_SEQ, seq);
    put_u64(&mut buf, OFF_REQUEST_ID, request_id);
    put_u32(&mut buf, OFF_PAYLOAD_LEN, payload.len() as u32);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::capabilities::CapabilityBits;
    use kiri_core::dispatch::{capability_bit, command_id, Router};
    use kiri_core::trace::NoopTraceSink;
    use kiri_core::wire::WireRequest;
    use kiri_core::zc_ipc_gate::ZcIpcGate;
    use serde_json::Value;

    fn arena() -> Vec<u8> {
        let mut buf = vec![0u8; RING_BUFFER_BYTES as usize];
        write_global_header(&mut buf).unwrap();
        buf
    }

    #[test]
    fn arena_size_and_layout() {
        assert_eq!(RING_BUFFER_BYTES, 16_777_792);
        assert_eq!(slot_offset(0), 64);
        assert_eq!(slot_offset(SLOT_COUNT - 1) + SLOT_BYTES, RING_BUFFER_BYTES as usize);
        assert!(slot_mut(&mut [0u8; 128], 0).is_none());
    }

    #[test]
    fn global_header_roundtrip_shape() {
        let buf = arena();
        assert_eq!(&buf[0..4], b"KRIR");
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), RING_VERSION);
        assert_eq!(u32::from_le_bytes(buf[8..12].try_into().unwrap()), SLOT_COUNT);
        assert_eq!(u32::from_le_bytes(buf[12..16].try_into().unwrap()), SLOT_BYTES as u32);
        assert_eq!(u32::from_le_bytes(buf[16..20].try_into().unwrap()), SLOT_PAYLOAD_CAP as u32);
    }

    #[test]
    fn request_response_cycle_on_one_slot() {
        let mut buf = arena();
        let payload = b"hello";
        {
            let slot = slot_mut(&mut buf, 3).unwrap();
            publish_request(slot, 7, 42, 1, CODEC_UTF8_STRING, payload).unwrap();
        }
        let req = {
            let slot = slot_ref(&buf, 3).unwrap();
            read_request(slot).unwrap()
        };
        assert_eq!(req.seq, 7);
        assert_eq!(req.request_id, 42);
        assert_eq!(req.command_id, 1);
        assert_eq!(req.codec, CODEC_UTF8_STRING);
        assert_eq!(req.payload_len, 5);
        {
            let slot = slot_ref(&buf, 3).unwrap();
            assert_eq!(slot_payload(slot).unwrap(), payload);
        }
        {
            let slot = slot_mut(&mut buf, 3).unwrap();
            publish_response(slot, 7, 42, RESP_ECHO_STRING, payload).unwrap();
        }
        {
            let slot = slot_ref(&buf, 3).unwrap();
            let (flags, len) = read_response_header(slot, 42).unwrap();
            assert_eq!(flags, RESP_ECHO_STRING);
            assert_eq!(len, 5);
            assert_eq!(slot_payload(slot).unwrap(), payload);
        }
        {
            let slot = slot_mut(&mut buf, 3).unwrap();
            release_slot(slot);
            assert!(claim_slot(slot));
        }
    }

    #[test]
    fn read_request_rejects_bad_states() {
        let mut buf = arena();
        {
            let slot = slot_mut(&mut buf, 0).unwrap();
            // Never published: state FREE must not read as a request.
            assert!(read_request(slot).is_err());
        }
        {
            let slot = slot_mut(&mut buf, 1).unwrap();
            publish_request(slot, 1, 2, 1, CODEC_JSON, b"{}").unwrap();
            slot[0] = b'X'; // corrupt magic
            assert!(read_request(slot).is_err());
        }
        {
            let slot = slot_mut(&mut buf, 2).unwrap();
            publish_request(slot, 1, 2, 1, CODEC_JSON, b"{}").unwrap();
            put_u32(slot, OFF_PAYLOAD_LEN, SLOT_PAYLOAD_CAP as u32 + 1);
            assert!(read_request(slot).is_err());
        }
    }

    #[test]
    fn publish_request_enforces_capacity_and_ownership() {
        let mut buf = arena();
        {
            let slot = slot_mut(&mut buf, 4).unwrap();
            let big = vec![0u8; SLOT_PAYLOAD_CAP + 1];
            assert!(publish_request(slot, 1, 2, 1, CODEC_JSON, &big).is_err());
            let exact = vec![7u8; SLOT_PAYLOAD_CAP];
            assert!(publish_request(slot, 1, 2, 1, CODEC_JSON, &exact).is_ok());
            // Second publish on a non-free slot is rejected.
            assert!(publish_request(slot, 1, 2, 1, CODEC_JSON, b"x").is_err());
        }
    }

    #[test]
    fn response_header_checks_request_id() {
        let mut buf = arena();
        {
            let slot = slot_mut(&mut buf, 5).unwrap();
            publish_request(slot, 1, 9, 1, CODEC_JSON, b"{}").unwrap();
            publish_response(slot, 1, 9, RESP_JSON, b"{}").unwrap();
        }
        let slot = slot_ref(&buf, 5).unwrap();
        assert!(read_response_header(slot, 9).is_ok());
        assert!(read_response_header(slot, 10).is_err());
    }

    #[test]
    fn request_frame_roundtrips_through_response_frame() {
        // The page builds a frame exactly like the injected JS does: header +
        // raw payload, no arena padding.
        let payload = b"aaaa";
        let mut frame = vec![0u8; SLOT_HEADER_BYTES + payload.len()];
        frame[SLOT_HEADER_BYTES..].copy_from_slice(payload);
        put_u32(&mut frame, OFF_MAGIC, SLOT_MAGIC);
        frame[OFF_STATE] = STATE_REQUEST;
        frame[OFF_FLAGS] = 0;
        put_u16(&mut frame, OFF_CODEC, CODEC_UTF8_STRING);
        put_u64(&mut frame, OFF_SEQ, 9);
        put_u64(&mut frame, OFF_REQUEST_ID, 77);
        put_u32(&mut frame, OFF_COMMAND_ID, 1);
        put_u32(&mut frame, OFF_PAYLOAD_LEN, payload.len() as u32);

        let (req, got) = read_request_frame(&frame).unwrap();
        assert_eq!(req.seq, 9);
        assert_eq!(req.request_id, 77);
        assert_eq!(req.command_id, 1);
        assert_eq!(req.codec, CODEC_UTF8_STRING);
        assert_eq!(got, payload);

        let out = encode_response_frame(req.seq, req.request_id, RESP_ECHO_STRING, got).unwrap();
        assert_eq!(get_u32(&out, OFF_MAGIC), SLOT_MAGIC);
        assert_eq!(out[OFF_STATE], STATE_RESPONSE);
        assert_eq!(out[OFF_FLAGS], RESP_ECHO_STRING);
        assert_eq!(get_u64(&out, OFF_SEQ), 9);
        assert_eq!(get_u64(&out, OFF_REQUEST_ID), 77);
        assert_eq!(get_u32(&out, OFF_PAYLOAD_LEN) as usize, payload.len());
        assert_eq!(&out[SLOT_HEADER_BYTES..], payload);
    }

    #[test]
    fn read_request_frame_rejects_bad_states_and_lengths() {
        assert!(read_request_frame(&[]).is_err());
        assert!(read_request_frame(&[0u8; SLOT_HEADER_BYTES]).is_err());
        // Valid header, wrong state.
        let mut frame = vec![0u8; SLOT_HEADER_BYTES];
        put_u32(&mut frame, OFF_MAGIC, SLOT_MAGIC);
        frame[OFF_STATE] = STATE_RESPONSE;
        assert!(read_request_frame(&frame).is_err());
        // Valid request header, declared payload longer than the frame.
        frame[OFF_STATE] = STATE_REQUEST;
        put_u32(&mut frame, OFF_PAYLOAD_LEN, 10);
        assert!(read_request_frame(&frame).is_err());
        // Declared payload over the slot capacity.
        put_u32(&mut frame, OFF_PAYLOAD_LEN, SLOT_PAYLOAD_CAP as u32 + 1);
        assert!(read_request_frame(&frame).is_err());
        // Corrupt magic.
        put_u32(&mut frame, OFF_PAYLOAD_LEN, 0);
        frame[0] = b'X';
        assert!(read_request_frame(&frame).is_err());
    }

    #[test]
    fn encode_response_frame_enforces_capacity() {
        let big = vec![0u8; SLOT_PAYLOAD_CAP + 1];
        assert!(encode_response_frame(1, 2, RESP_JSON, &big).is_err());
        let exact = vec![7u8; SLOT_PAYLOAD_CAP];
        assert_eq!(
            encode_response_frame(1, 2, RESP_JSON, &exact).unwrap().len(),
            SLOT_HEADER_BYTES + SLOT_PAYLOAD_CAP
        );
    }

    #[test]
    fn response_echo_bytes_only_for_pong_echo_strings() {
        use kiri_core::wire::WireResponse;
        let pong = WireResponse::ok(1, serde_json::json!({"pong": true, "echo": "abc"}));
        assert_eq!(response_echo_bytes(&pong), Some(b"abc".as_slice()));
        let no_echo = WireResponse::ok(1, serde_json::json!({"pong": true}));
        assert!(response_echo_bytes(&no_echo).is_none());
        let not_pong = WireResponse::ok(1, serde_json::json!({"echo": "abc"}));
        assert!(response_echo_bytes(&not_pong).is_none());
        let err = WireResponse::err(1, kiri_core::error::Error::protocol_error("x"));
        assert!(response_echo_bytes(&err).is_none());
    }

    fn ping_caps() -> CapabilityBits {
        let mut c = CapabilityBits::empty();
        c.set(capability_bit::PING);
        c
    }

    const T0: u64 = 1_000_000_000;

    // The ring shared-slot write is a gated object: it runs only while a
    // grant minted + redeemed for this caller and command is still live.
    // Wrong caller, wrong command, and expiry all deny the arena write.
    #[test]
    fn ring_shared_buffer_reply_requires_live_grant() {
        let router = Router::new();
        let mut gate = ZcIpcGate::with_ttl_ns(1_000);
        let req = WireRequest::new(command_id::PING, 1, 1, Value::Null);
        let permit = gate.mint(&router, CallerId(1), &ping_caps(), &req, T0).unwrap();
        let grant = gate.redeem(&permit, CallerId(1), &req, T0).unwrap();
        let body = b"{}";
        assert!(ring_reply_authorized(Some(&grant), CallerId(1), command_id::PING, body, T0));
        assert!(!ring_reply_authorized(Some(&grant), CallerId(2), command_id::PING, body, T0));
        assert!(!ring_reply_authorized(Some(&grant), CallerId(1), command_id::HTTP_GET, body, T0));
        assert!(!ring_reply_authorized(
            Some(&grant),
            CallerId(1),
            command_id::PING,
            body,
            T0 + 1_001
        ));
    }

    // A denied dispatch produces an error response and no grant; the reply
    // decision must then refuse the shared-slot write so the denial can only
    // cross on the ordinary JSON wire. A bare missing grant denies the same
    // way.
    #[test]
    fn ring_reply_denies_without_grant() {
        let router = Router::new();
        let mut gate = ZcIpcGate::new();
        let out = gate.dispatch_through_webview(
            &router,
            CallerId(1),
            &CapabilityBits::empty(),
            &WireRequest::new(command_id::PING, 1, 1, Value::Null),
            &mut NoopTraceSink,
            T0,
        );
        assert!(out.response.error.is_some());
        assert!(out.grant.is_none(), "a denied dispatch must not carry a grant");
        assert!(!ring_reply_authorized(
            out.grant.as_ref(),
            CallerId(1),
            command_id::PING,
            b"{}",
            T0
        ));
        assert!(!ring_reply_authorized(None, CallerId(1), command_id::PING, b"{}", T0));
    }
}
