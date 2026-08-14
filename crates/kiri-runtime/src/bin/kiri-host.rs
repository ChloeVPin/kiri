//! `kiri-host`: the native host binary.
//!
//! Serves the shared blank frontend and runs the startup sequence, emitting
//! the startup result JSON. The backend is selected automatically (direct
//! Win32 + WebView2 on Windows, wry/tao elsewhere). In smoke mode (`--smoke`)
//! it exits by itself after the first animation frame plus
//! `--exit-after-ready-ms`, gated by a watchdog so CI cannot hang.

use std::path::PathBuf;

use kiri_runtime::host_options_from_args;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut frontend_dir: Option<PathBuf> = None;
    let mut markers_out: Option<PathBuf> = None;
    let mut title = "Kiri host".to_string();
    let mut width = 1024u32;
    let mut height = 768u32;
    let mut smoke = false;
    let mut exit_after_ready_ms = 250u32;
    let mut watchdog_ms = 30_000u32;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frontend" => {
                i += 1;
                frontend_dir = args.get(i).map(PathBuf::from);
            }
            "--markers-out" => {
                i += 1;
                markers_out = args.get(i).map(PathBuf::from);
            }
            "--title" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    title = v.clone();
                }
            }
            "--width" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    width = v.parse().unwrap_or(width);
                }
            }
            "--height" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    height = v.parse().unwrap_or(height);
                }
            }
            "--smoke" => smoke = true,
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
                    "kiri-host: native host (cross-platform)\n\
                     usage: kiri-host --frontend DIR [--markers-out PATH] [--smoke]\n\
                     \x20  [--title T] [--width N] [--height N]\n\
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

    if frontend_dir.is_none() {
        eprintln!("kiri-host: --frontend DIR is required");
        std::process::exit(2);
    }

    let options = host_options_from_args(
        frontend_dir,
        markers_out,
        title,
        width,
        height,
        smoke,
        exit_after_ready_ms,
        watchdog_ms,
    );
    let code = kiri_runtime::run_session(&options);
    std::process::exit(code);
}
