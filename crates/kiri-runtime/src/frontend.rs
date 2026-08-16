//! Resolve the directory that `kiri-host` serves as the application origin.
//!
//! `--frontend DIR` still wins. When it is omitted (double-clicked `.app`,
//! unzipped Windows folder), the host looks next to the binary so a packaged
//! app does not need a wrapper script.

use std::path::{Path, PathBuf};

/// True when `dir/index.html` exists.
pub fn frontend_has_index(dir: &Path) -> bool {
    dir.join("index.html").is_file()
}

/// Candidate frontend directories derived from the running host binary.
///
/// Order:
/// 1. `Contents/Resources/frontend` when the binary lives in `Contents/MacOS`
/// 2. `../Resources/frontend` relative to the executable directory
/// 3. `frontend` next to the executable (Windows/Linux zip layout)
pub fn bundled_frontend_candidates(exe_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(exe_dir) = exe_path.parent() else {
        return out;
    };
    if exe_dir.file_name().is_some_and(|n| n == "MacOS") {
        if let Some(contents) = exe_dir.parent() {
            out.push(contents.join("Resources").join("frontend"));
        }
    }
    out.push(exe_dir.join("..").join("Resources").join("frontend"));
    out.push(exe_dir.join("frontend"));
    out
}

/// Pick the frontend directory.
///
/// 1. explicit `--frontend`
/// 2. `KIRI_FRONTEND`
/// 3. first bundled candidate that contains `index.html`
pub fn resolve_frontend_dir(
    explicit: Option<PathBuf>,
    env_frontend: Option<PathBuf>,
    exe_path: &Path,
) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        if frontend_has_index(&dir) {
            return Ok(dir);
        }
        return Err(format!(
            "kiri-host: --frontend {} does not contain index.html",
            dir.display()
        ));
    }
    if let Some(dir) = env_frontend {
        if frontend_has_index(&dir) {
            return Ok(dir);
        }
        return Err(format!(
            "kiri-host: KIRI_FRONTEND={} does not contain index.html",
            dir.display()
        ));
    }
    for candidate in bundled_frontend_candidates(exe_path) {
        if frontend_has_index(&candidate) {
            return Ok(candidate);
        }
    }
    Err(
        "kiri-host: no frontend found. Pass --frontend DIR, set KIRI_FRONTEND, \
         or place index.html in Resources/frontend (macOS app) or ./frontend \
         next to the binary."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kiri-frontend-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn macos_bundle_layout_is_first_candidate() {
        let exe = PathBuf::from("/tmp/Kiri.app/Contents/MacOS/kiri-host");
        let c = bundled_frontend_candidates(&exe);
        assert_eq!(c[0], PathBuf::from("/tmp/Kiri.app/Contents/Resources/frontend"));
        assert!(c.iter().any(|p| p.ends_with("frontend")));
    }

    #[test]
    fn windows_zip_layout_is_beside_the_exe() {
        let exe = PathBuf::from("C:/kiri/kiri-host.exe");
        let c = bundled_frontend_candidates(&exe);
        assert!(c.iter().any(|p| p == &PathBuf::from("C:/kiri/frontend")));
    }

    #[test]
    fn explicit_dir_wins_when_index_exists() {
        let root = temp_tree("explicit");
        let front = root.join("ui");
        fs::create_dir_all(&front).unwrap();
        fs::write(front.join("index.html"), "<html></html>").unwrap();
        let got = resolve_frontend_dir(Some(front.clone()), None, &root.join("kiri-host")).unwrap();
        assert_eq!(got, front);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_index_is_an_error() {
        let root = temp_tree("missing");
        let err = resolve_frontend_dir(Some(root.clone()), None, &root.join("kiri-host")).unwrap_err();
        assert!(err.contains("index.html"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_macos_resources_are_discovered() {
        let root = temp_tree("bundle");
        let macos = root.join("Contents").join("MacOS");
        let front = root.join("Contents").join("Resources").join("frontend");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&front).unwrap();
        fs::write(front.join("index.html"), "<html></html>").unwrap();
        let exe = macos.join("kiri-host");
        let got = resolve_frontend_dir(None, None, &exe).unwrap();
        assert_eq!(got, front);
        let _ = fs::remove_dir_all(root);
    }
}
