//! Shared startup result output used by every backend and both binaries.

use std::path::PathBuf;

use crate::markers::StartupMarkers;

/// Write the startup result JSON (WP1 acceptance: startup result JSON).
///
/// Prints to stdout, or writes to `path` when given.
pub fn write_startup_result(markers: &StartupMarkers, path: Option<&PathBuf>) {
    let json = serde_json::to_string_pretty(&markers.result()).expect("startup result serializes");
    match path {
        Some(path) => {
            if let Err(e) = std::fs::write(path, json) {
                eprintln!("[kiri] failed to write startup result to {}: {e}", path.display());
            }
        }
        None => println!("{json}"),
    }
}
