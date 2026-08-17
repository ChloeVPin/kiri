//! Host-pinned release feed. JavaScript never names this URL.

use kiri_core::error::{Error, Result};

/// Latest signed `RELEASES.json`. The host fetches this when
/// `kiri.updater.check` is called without a manifest.
pub const PINNED_RELEASE_FEED: &str =
    "https://github.com/ChloeVPin/kiri/releases/latest/download/RELEASES.json";

pub fn fetch_pinned_release_manifest() -> Result<String> {
    // curl is on macOS, Linux, and current Windows runners. Using it avoids
    // pulling rustls/ring into the host, which breaks the
    // x86_64-pc-windows-msvc cross-check on this Mac.
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "-A", "kiri-host", "--max-time", "15", PINNED_RELEASE_FEED])
        .output()
        .map_err(|e| Error::invalid_argument(format!("update feed: {e}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Error::invalid_argument(format!(
            "update feed: curl exited {}: {err}",
            output.status
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| Error::invalid_argument(format!("update feed: {e}")))
}
