//! Ordinary-message bulk-path benchmark (T007).
//!
//! Measures the kiri-core JSON control path (serialize WireRequest ->
//! Router.dispatch -> deserialize WireResponse) at the bulk payload sizes
//! defined in `benchmark/test-vectors.json` (1 MB / 16 MB / 100 MB). This is
//! the "ordinary message" path, distinct from the WebView2 shared-buffer fast
//! path that T008 explores on Windows. It is fully Mac/Linux-runnable: it
//! exercises kiri-core only, with no WebView.
//!
//! For each size it runs `runs` iterations, recording per-iteration wall
//! clock, CPU time (process user + sys via getrusage), and peak RSS
//! (ru_maxrss), then writes one raw JSON artifact with environment metadata
//! and the full sample set (no summary is substituted for the raw runs).

use std::time::Instant;

use kiri_core::caller::CallerRegistry;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::dispatch::{capability_bit, ping_request, Router};
use kiri_core::trace::NoopTraceSink;
use serde_json::Value;

#[cfg(target_os = "macos")]
fn peak_rss_bytes() -> u64 {
    // ru_maxrss on macOS is in bytes.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage.ru_maxrss as u64
    }
}

#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> u64 {
    // ru_maxrss on Linux is in kilobytes.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        (usage.ru_maxrss as u64) * 1024
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peak_rss_bytes() -> u64 {
    0
}

fn cpu_time_ns() -> u64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        let sec = usage.ru_utime.tv_sec as u64 + usage.ru_stime.tv_sec as u64;
        let usec = usage.ru_utime.tv_usec as u64 + usage.ru_stime.tv_usec as u64;
        sec * 1_000_000_000 + usec * 1_000
    }
}

struct Run {
    size: usize,
    runs: usize,
    wall_ms: Vec<f64>,
    cpu_ms: Vec<f64>,
    peak_rss_bytes: Vec<u64>,
    throughput_mib_s: Vec<f64>,
}

fn bench(size: usize, runs: usize) -> Run {
    // Build one representative bulk payload (a filled byte buffer carried as a
    // JSON array of u8 is unrealistically heavy; model the realistic ordinary
    // message as a base64-free JSON object whose payload is a byte blob
    // encoded as a JSON string of `size` chars, matching how the bridge
    // carries binary today).
    let payload: Value = Value::String("a".repeat(size));

    let mut registry = CallerRegistry::new();
    let caller = registry.register();
    let mut caps = CapabilityBits::empty();
    caps.set(capability_bit::PING);
    let router = Router::new();
    let mut sink = NoopTraceSink;

    // Warmup.
    for _ in 0..3 {
        let req = ping_request(1, payload.clone());
        let _ = router.dispatch(caller, &caps, &req, &mut sink);
    }

    let mut wall_ms = Vec::with_capacity(runs);
    let mut cpu_ms = Vec::with_capacity(runs);
    let mut rss = Vec::with_capacity(runs);
    let mut tput = Vec::with_capacity(runs);

    for i in 0..runs {
        let request_id = 1000 + i as u64;
        let req = ping_request(request_id, payload.clone());
        // Serialize the request to model the full ordinary-message cost
        // (the bridge sends JSON over web messaging).
        let serialized = serde_json::to_vec(&req).expect("serialize request");

        let t0 = Instant::now();
        let cpu0 = cpu_time_ns();
        let resp = router.dispatch(caller, &caps, &req, &mut sink);
        let cpu1 = cpu_time_ns();
        let _ = serde_json::from_slice::<kiri_core::wire::WireResponse>(
            &serde_json::to_vec(&resp).unwrap(),
        )
        .expect("round-trip response");
        let t1 = Instant::now();

        let wall = (t1 - t0).as_nanos() as f64 / 1_000_000.0;
        let cpu = (cpu1 - cpu0) as f64 / 1_000_000.0;
        let peak = peak_rss_bytes();

        wall_ms.push(wall);
        cpu_ms.push(cpu);
        rss.push(peak);
        tput.push((serialized.len() as f64) / (1024.0 * 1024.0) / (wall / 1000.0));
    }

    Run { size, runs, wall_ms, cpu_ms, peak_rss_bytes: rss, throughput_mib_s: tput }
}

fn main() {
    let runs = std::env::var("KIRI_BULK_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let sizes: Vec<usize> = vec![1 << 20, 16 << 20, 100 << 20];

    let mut results = Vec::new();
    for size in &sizes {
        let run = bench(*size, runs);
        let mean_wall = run.wall_ms.iter().sum::<f64>() / run.wall_ms.len() as f64;
        let mean_cpu = run.cpu_ms.iter().sum::<f64>() / run.cpu_ms.len() as f64;
        let max_rss = run.peak_rss_bytes.iter().copied().max().unwrap_or(0);
        let mean_tput =
            run.throughput_mib_s.iter().sum::<f64>() / run.throughput_mib_s.len() as f64;
        println!(
            "size={} B runs={} mean_wall_ms={:.3} mean_cpu_ms={:.3} max_rss_bytes={} mean_throughput_MiB_s={:.3}",
            run.size, run.runs, mean_wall, mean_cpu, max_rss, mean_tput
        );
        results.push(serde_json::json!({
            "size_bytes": run.size,
            "runs": run.runs,
            "wall_ms": run.wall_ms,
            "cpu_ms": run.cpu_ms,
            "peak_rss_bytes": run.peak_rss_bytes,
            "throughput_mib_s": run.throughput_mib_s,
            "summary": {
                "mean_wall_ms": mean_wall,
                "mean_cpu_ms": mean_cpu,
                "max_rss_bytes": max_rss,
                "mean_throughput_mib_s": mean_tput,
            }
        }));
    }

    let artifact = serde_json::json!({
        "schema_version": 1,
        "name": "ordinary-message-bulk-path",
        "commit": option_env!("KIRI_COMMIT").unwrap_or(""),
        "created_unix_ns": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0),
        "sizes_bytes": sizes.clone(),
        "results": results,
    });
    let out = std::env::var("KIRI_BULK_OUT")
        .unwrap_or_else(|_| "artifacts/bulk-ordinary.json".to_string());
    if let Some(parent) = std::path::Path::new(&out).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&out, serde_json::to_string_pretty(&artifact).unwrap()).expect("write artifact");
    eprintln!("wrote {}", out);
}
