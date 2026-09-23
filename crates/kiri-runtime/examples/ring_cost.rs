//! In-process cost model for the zero-copy ring spike (NOT a through-webview
//! IPC benchmark). It times, in isolation, the two costs the ring removes
//! from the T008/JSON wire path per large reply:
//!
//! - host side: `serde_json::to_vec` of a `{"pong":true,"echo":"a" * N}`
//!   WireResponse versus a raw `memcpy` of the same bytes into a slot, and
//! - the same JSON buffer parsed back (`serde_json::from_slice`) as the page
//!   side's JSON.parse stand-in.
//!
//! The ring still pays one bounded copy into shared memory; this measures
//! what remains once per-reply `CreateSharedBuffer` and the megabyte JSON
//! encode/decode are gone. Run with `--release` for meaningful numbers.

use std::hint::black_box;
use std::time::Instant;

use kiri_core::wire::WireResponse;

const ITERS: u32 = 50;
const SIZES: &[usize] = &[16_384, 262_144, 1_048_574];

fn mean_ms(samples: &[f64]) -> f64 {
    samples.iter().sum::<f64>() / samples.len().max(1) as f64
}

fn main() {
    println!("in-process serialization cost model (not through-webview IPC)");
    println!("iters per size: {ITERS}");
    println!("size_bytes, json_encode_ms, json_parse_ms, memcpy_ms");
    for &size in SIZES {
        let payload = serde_json::json!({ "pong": true, "echo": "a".repeat(size) });
        let response = WireResponse::ok(1, payload);
        let mut enc = Vec::with_capacity(ITERS as usize);
        let mut dec = Vec::with_capacity(ITERS as usize);
        let mut cp = Vec::with_capacity(ITERS as usize);
        let mut dst = vec![0u8; size + 4096];
        for _ in 0..ITERS {
            let t = Instant::now();
            let bytes = serde_json::to_vec(black_box(&response)).unwrap();
            enc.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let back: serde_json::Value = serde_json::from_slice(black_box(&bytes)).unwrap();
            dec.push(t.elapsed().as_secs_f64() * 1000.0);
            black_box(&back);
            let t = Instant::now();
            dst[..bytes.len()].copy_from_slice(black_box(&bytes));
            cp.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{size}, {:.3}, {:.3}, {:.3}", mean_ms(&enc), mean_ms(&dec), mean_ms(&cp));
    }
}
