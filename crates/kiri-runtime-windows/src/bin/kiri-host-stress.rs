//! `kiri-host-stress`: launch-close stress driver (WINDOWS_MVP WP5).
//!
//! Runs `--cycles` full host sessions in smoke mode and fails if any cycle
//! misses required startup markers or hits the watchdog.

#![cfg(target_os = "windows")]

use kiri_runtime_windows::require_smoke_markers;

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

    let options = kiri_runtime_windows::host_options_from_args(
        Some(frontend_dir),
        "Kiri stress".into(),
        1024,
        768,
        true,
        exit_after_ready_ms,
        watchdog_ms,
    );

    let mut failures = 0u32;
    for cycle in 1..=cycles {
        match kiri_runtime_windows::WindowsHost::run(&options) {
            Ok(markers) => {
                if let Err(err) = require_smoke_markers(&markers) {
                    failures += 1;
                    eprintln!("[stress] cycle {cycle}: {err}");
                }
            }
            Err(err) => {
                failures += 1;
                eprintln!("[stress] cycle {cycle}: {err}");
            }
        }
        if failures >= 5 {
            eprintln!("[stress] aborting after {failures} failures");
            std::process::exit(1);
        }
    }

    println!("[stress] {cycles} cycles, {failures} failures");
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
