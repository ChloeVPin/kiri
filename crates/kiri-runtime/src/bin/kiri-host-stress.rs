//! `kiri-host-stress`: launch-close stress driver.
//!
//! Runs `--cycles` full host sessions in smoke mode and fails if any cycle
//! misses required startup markers or hits the watchdog.
//!
//! Each cycle spawns a fresh `kiri-host` subprocess (best-effort discovery of
//! the sibling binary in the same target dir). This gives genuine
//! process-level launch/close isolation and works identically on every
//! platform: on macOS/Linux the cross backend's event loop never returns, so
//! an in-process `run()` would terminate the whole stress process after a
//! single cycle. The sibling binary is launched with `--smoke
//! --markers-out <tmpfile>`; markers are read back from the file and checked.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use kiri_runtime::markers::StartupMarkers;
use kiri_runtime::require_smoke_markers;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut frontend_dir: Option<std::path::PathBuf> = None;
    let mut cycles = 100u32;
    let mut exit_after_ready_ms = 250u32;
    let mut watchdog_ms = 30_000u32;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frontend" => {
                i += 1;
                frontend_dir = args.get(i).map(std::path::PathBuf::from);
            }
            "--cycles" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    cycles = v.parse().unwrap_or(cycles);
                }
            }
            "--exit-after-ready-ms" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    exit_after_ready_ms = v.parse().unwrap_or(exit_after_ready_ms);
                }
            }
            "--watchdog-ms" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    watchdog_ms = v.parse().unwrap_or(watchdog_ms);
                }
            }
            "--help" | "-h" => {
                println!(
                    "kiri-host-stress: launch-close stress driver\n\
                     usage: kiri-host-stress --frontend DIR [--cycles N]\n\
                     \x20  [--exit-after-ready-ms N] [--watchdog-ms N]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let frontend_dir = match frontend_dir {
        Some(dir) => dir,
        None => {
            eprintln!("kiri-host-stress: --frontend DIR is required");
            std::process::exit(2);
        }
    };

    // Discover the sibling kiri-host binary. `CARGO_BIN_EXE_kiri-host` is set
    // when built/tested by cargo; otherwise probe the current exe's directory.
    let kiri_host =
        std::env::var("CARGO_BIN_EXE_kiri-host").ok().map(PathBuf::from).or_else(|| {
            std::env::current_exe().ok().and_then(|exe| {
                exe.parent()
                    .map(|d| d.join(if cfg!(windows) { "kiri-host.exe" } else { "kiri-host" }))
            })
        });

    let Some(kiri_host) = kiri_host else {
        eprintln!("[stress] could not locate kiri-host binary");
        std::process::exit(1);
    };
    if !kiri_host.exists() {
        eprintln!("[stress] kiri-host binary not found at {}", kiri_host.display());
        std::process::exit(1);
    }

    let tmpdir = std::env::temp_dir();
    let mut failures = 0u32;
    for cycle in 1..=cycles {
        let markers_path =
            tmpdir.join(format!("kiri-stress-{}-{}.json", std::process::id(), cycle));
        let mut cmd = Command::new(&kiri_host);
        cmd.arg("--smoke")
            .arg("--frontend")
            .arg(&frontend_dir)
            .arg("--markers-out")
            .arg(&markers_path)
            .arg("--exit-after-ready-ms")
            .arg(exit_after_ready_ms.to_string())
            .arg("--watchdog-ms")
            .arg(watchdog_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = match cmd.status() {
            Ok(status) => status,
            Err(e) => {
                failures += 1;
                eprintln!("[stress] cycle {cycle}: failed to spawn kiri-host: {e}");
                if failures >= 5 {
                    break;
                }
                continue;
            }
        };

        if !status.success() {
            failures += 1;
            eprintln!(
                "[stress] cycle {cycle}: kiri-host exited with {}",
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
            );
            if failures >= 5 {
                break;
            }
            continue;
        }

        let Ok(bytes) = std::fs::read(&markers_path) else {
            failures += 1;
            eprintln!("[stress] cycle {cycle}: no markers file produced");
            if failures >= 5 {
                break;
            }
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            failures += 1;
            eprintln!("[stress] cycle {cycle}: markers file not valid JSON");
            if failures >= 5 {
                break;
            }
            continue;
        };
        // Reconstruct StartupMarkers from the JSON result for the shared check.
        let markers = markers_from_result(&value);
        if let Err(err) = require_smoke_markers(&markers) {
            failures += 1;
            eprintln!("[stress] cycle {cycle}: {err}");
        }
        let _ = std::fs::remove_file(&markers_path);
        if failures >= 5 {
            eprintln!("[stress] aborting after {failures} failures");
            break;
        }
    }

    println!("[stress] {cycles} cycles, {failures} failures");
    std::process::exit(if failures == 0 { 0 } else { 1 });
}

/// Rebuild a `StartupMarkers` snapshot from a startup-result JSON value so the
/// shared `require_smoke_markers` check can run on subprocess output.
fn markers_from_result(value: &serde_json::Value) -> StartupMarkers {
    let mut markers = StartupMarkers::new();
    // `since_first_ns` is monotonic across the recorded set; stable reference
    // ordering keeps the delta math meaningful even after a round-trip.
    let mut ordered: Vec<(String, u64, u64)> = Vec::new();
    if let Some(arr) = value.get("markers").and_then(|m| m.as_array()) {
        for item in arr {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let ts = item.get("timestamp_ns").and_then(|t| t.as_u64()).unwrap_or(0);
            let since = item.get("since_first_ns").and_then(|t| t.as_u64()).unwrap_or(0);
            ordered.push((name, ts, since));
        }
    }
    // Re-record using the original absolute timestamps so `has()` works.
    for (name, ts, _) in ordered {
        if let Some(marker) = marker_by_name(&name) {
            markers.record(marker, ts);
        }
    }
    markers
}

/// Map a marker name string back to the enum for reconstruction.
fn marker_by_name(name: &str) -> Option<kiri_runtime::markers::Marker> {
    use kiri_runtime::markers::Marker;
    match name {
        "process_spawn_requested" => Some(Marker::ProcessSpawnRequested),
        "native_entry" => Some(Marker::NativeEntry),
        "platform_initialized" => Some(Marker::PlatformInit),
        "webview_creation_requested" => Some(Marker::WebViewCreationRequested),
        "webview_ready" => Some(Marker::WebViewReady),
        "bridge_ready" => Some(Marker::BridgeReady),
        "dom_ready" => Some(Marker::DomReady),
        "app_ready" => Some(Marker::AppReady),
        "first_animation_frame" => Some(Marker::FirstAnimationFrame),
        _ => None,
    }
}
