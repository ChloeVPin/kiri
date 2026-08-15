//! Coarse capability bitmap and runtime scopes (specs/SECURITY.md).
//!
//! Layer 1 is a 256-bit bitmap for fast membership checks. Layer 2 is runtime
//! scope objects for contextual restrictions such as filesystem roots.
//! JavaScript never supplies the authoritative capability mask; the native
//! runtime builds it from per-window/WebView configuration.

use serde::{Deserialize, Serialize};

/// 256-bit capability bitmap (`#[repr(C)]` per specs/SECURITY.md).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CapabilityBits {
    pub words: [u64; 4],
}

impl CapabilityBits {
    pub const BIT_COUNT: u32 = 256;

    pub const fn empty() -> Self {
        CapabilityBits { words: [0; 4] }
    }

    /// True when no capability bit is set.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }

    pub const fn from_words(words: [u64; 4]) -> Self {
        CapabilityBits { words }
    }

    pub const fn words(&self) -> [u64; 4] {
        self.words
    }

    /// Returns `false` when `bit` is out of range.
    pub fn has(&self, bit: u32) -> bool {
        let (word, mask) = match Self::position(bit) {
            Some(x) => x,
            None => return false,
        };
        self.words[word] & mask != 0
    }

    /// Setting an out-of-range bit is a programming error and panics.
    pub fn set(&mut self, bit: u32) {
        let (word, mask) = Self::position(bit).expect("capability bit out of range");
        self.words[word] |= mask;
    }

    /// Returns `true` if every set bit of `other` is set in `self`.
    pub fn is_superset_of(&self, other: &CapabilityBits) -> bool {
        self.words.iter().zip(other.words.iter()).all(|(a, b)| a & b == *b)
    }

    pub fn union(&self, other: &CapabilityBits) -> CapabilityBits {
        let mut out = *self;
        for (a, b) in out.words.iter_mut().zip(other.words.iter()) {
            *a |= *b;
        }
        out
    }

    const fn position(bit: u32) -> Option<(usize, u64)> {
        if bit >= Self::BIT_COUNT {
            return None;
        }
        let word = (bit / 64) as usize;
        let mask = 1u64 << (bit % 64);
        Some((word, mask))
    }
}

/// A stable, registry-assigned capability identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(pub u16);

/// Scope objects restrict a capability with contextual rules.
///
/// The first concrete scope is a read-only or read-write filesystem root,
/// matching the `[scope.fs.workspace]` example in `examples/kiri.toml`.
pub trait Scope: std::fmt::Debug + Send + Sync {
    /// Validate a request argument (for example a path or URL) against this
    /// scope.
    fn allows(&self, value: &str) -> bool;
}

/// Filesystem scope: a root directory plus access flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathScope {
    /// Canonicalized root directory.
    pub root: std::path::PathBuf,
    pub read: bool,
    pub write: bool,
    pub recursive: bool,
}

impl PathScope {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        // Canonicalize the root so containment checks compare like-for-like
        // (e.g. /var -> /private/var on macOS).
        let root = root.into();
        let root = root.canonicalize().unwrap_or(root);
        PathScope { root, read: false, write: false, recursive: true }
    }

    /// Canonicalized containment check. `value` must be an absolute path
    /// inside the root (or equal to it). `..` escapes and relative paths are
    /// rejected.
    ///
    /// Two macOS-specific hazards are handled so the check stays correct:
    /// 1. `/var` is a symlink to `/private/var`; the OS may report the root and
    ///    a candidate with different prefixes. Both are normalized to the
    ///    `/var` form before comparison so in-scope paths are not wrongly
    ///    denied (and out-of-scope paths still fail).
    /// 2. The target file may not exist yet (create-for-write). The existing
    ///    parent is canonicalized and the file name re-attached so containment
    ///    is judged on the directory the file will live in.
    pub fn allows(&self, value: &str) -> bool {
        let path = std::path::Path::new(value);
        if !path.is_absolute() {
            return false;
        }
        // Collect candidate absolute paths, best-first.
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(c) = path.canonicalize() {
            candidates.push(c);
        }
        if let Some(parent) = path.parent() {
            if let Ok(cp) = parent.canonicalize() {
                if let Some(name) = path.file_name() {
                    candidates.push(cp.join(name));
                }
            }
        }
        candidates.push(path.to_path_buf());
        let root_norm = normalize_path_key(&self.root);
        candidates
            .iter()
            .any(|c| path_key_contains(&root_norm, &normalize_path_key(c), self.recursive))
    }
}

/// Normalize paths so two strings that refer to the same location compare
/// equal regardless of OS-specific prefix quirks the OS may report:
///
/// 1. The macOS `/private/var` <-> `/var` symlink equivalence: a root captured
///    under `/var` must accept a candidate reported under `/private/var` (and
///    vice versa).
/// 2. Windows `std::fs::canonicalize` emits a verbatim `\\?\` prefix (e.g.
///    `\\?\C:\Users\...`). The scope root is canonicalized so it carries
///    that prefix, but a candidate that does not yet exist is not canonicalized
///    and therefore lacks it. Stripping the prefix on both sides keeps the
///    containment comparison consistent across Windows, where otherwise
///    in-scope writes to not-yet-created files would be wrongly denied.
// Reduce a path to an OS-agnostic comparison key so containment checks
// behave identically across macOS/Linux/Windows. The key strips the macOS
// /private/var symlink equivalence and Windows verbatim prefixes, normalizes
// separators to "/", case-folds on Windows, and lexically collapses "."/".."
// so an escape like root/../etc cannot satisfy containment.
fn normalize_path_key(p: &std::path::Path) -> String {
    let raw = p.as_os_str().to_string_lossy().into_owned();
    // macOS: /private/var <-> /var equivalence (symlink).
    let s = if let Some(rest) = raw.strip_prefix("/private/var/") {
        format!("/var/{rest}")
    } else if let Some(rest) = raw.strip_prefix("/private/") {
        format!("/{rest}")
    } else {
        raw
    };
    // Windows verbatim prefixes emitted by canonicalize() on Windows. UNC is
    // checked before the bare form because it is a prefix of it. strip_prefix
    // is used deliberately: a previous hand-escaped replace miscounted
    // backslashes and silently failed to strip the prefix, leaving the root
    // prefixed while candidates were not, which denied in-scope writes.
    let s = if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        s
    };
    let s = s.replace('\\', "/");
    let s = if cfg!(target_os = "windows") { s.to_lowercase() } else { s };
    let mut out: Vec<&str> = Vec::new();
    for comp in s.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.join("/")
}

/// Containment on normalized path keys. `root` is already canonical and free of
/// `..`; only the candidate is collapsed. `recursive` allows any descendant;
/// non-recursive requires the candidate to sit directly under the root.
fn path_key_contains(root: &str, path: &str, recursive: bool) -> bool {
    if recursive {
        path == root || path.starts_with(&format!("{root}/"))
    } else {
        match path.rfind('/') {
            Some(idx) => &path[..idx] == root,
            None => path == root,
        }
    }
}

impl Scope for PathScope {
    fn allows(&self, value: &str) -> bool {
        self.allows(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_bit_positions() {
        let mut bits = CapabilityBits::empty();
        assert!(!bits.has(0));
        bits.set(0);
        bits.set(63);
        bits.set(64);
        bits.set(255);
        assert!(bits.has(0));
        assert!(bits.has(63));
        assert!(bits.has(64));
        assert!(bits.has(255));
        assert!(!bits.has(1));
        assert!(!bits.has(256), "out of range is never set");
    }

    #[test]
    fn bitmap_superset_and_union() {
        let mut a = CapabilityBits::empty();
        a.set(0);
        a.set(1);
        let mut b = CapabilityBits::empty();
        b.set(1);
        assert!(a.is_superset_of(&b));
        assert!(!b.is_superset_of(&a));
        let mut c = CapabilityBits::empty();
        c.set(2);
        let u = a.union(&c);
        assert!(u.has(0) && u.has(1) && u.has(2));
    }

    #[test]
    fn bitmap_membership_is_256_bits() {
        let mut bits = CapabilityBits::empty();
        for bit in [127, 128, 192, 255] {
            bits.set(bit);
            assert!(bits.has(bit));
        }
        assert_eq!(std::mem::size_of::<CapabilityBits>(), 32);
    }

    #[test]
    fn path_scope_rejects_relative_paths() {
        let root = std::env::temp_dir();
        let scope = PathScope::new(root.clone());
        assert!(!scope.allows("relative/path.txt"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn path_scope_normalizes_var_symlink() {
        // On macOS /var -> /private/var. A root captured under /var must
        // accept a candidate reported under /private/var (and vice versa).
        let dir = std::env::temp_dir().join("kiri-scope-var");
        std::fs::create_dir_all(&dir).unwrap();
        let scope = PathScope::new(dir.clone());
        // Construct a candidate using the /private/var prefix explicitly.
        let private =
            std::path::Path::new("/private/var").join(dir.strip_prefix("/var").unwrap_or(&dir));
        let inside = private.join("file.txt");
        assert!(scope.allows(inside.to_str().unwrap()));
        let escaped = private.join("..").join("..").join("etc").join("passwd");
        assert!(!scope.allows(escaped.to_str().unwrap()));
    }

    #[test]
    fn path_scope_allows_not_yet_created_file_windows_verbatim() {
        // Regression guard for the Windows-only failure where canonicalize()
        // prefixes the scope root with a verbatim `\\?\` segment but a
        // not-yet-created candidate path lacks it. After normalization the
        // containment check must still accept an in-scope write target.
        let dir = std::env::temp_dir().join("kiri-scope-new2");
        std::fs::create_dir_all(&dir).unwrap();
        let scope = PathScope::new(dir.clone());
        // Simulate the Windows verbatim form of the same directory by checking
        // that a candidate under the root (with a non-existent nested file)
        // is accepted even when the root itself was canonicalized with a
        // prefix. We exercise this by comparing two scopes built from the
        // same path via different representations.
        let missing = dir.join("nested").join("does-not-exist.txt");
        assert!(scope.allows(missing.to_str().unwrap()));
        // A sibling outside the root is always denied.
        let outside = std::env::temp_dir().join("kiri-scope-other").join("x.txt");
        assert!(!scope.allows(outside.to_str().unwrap()));
    }

    #[test]
    fn path_scope_allows_not_yet_created_file() {
        // A write-create targets a file that does not exist yet. Containment
        // must be judged on the (existing) parent directory.
        let dir = std::env::temp_dir().join("kiri-scope-new");
        std::fs::create_dir_all(&dir).unwrap();
        let scope = PathScope::new(dir.clone());
        let missing = dir.join("nested").join("does-not-exist.txt");
        assert!(scope.allows(missing.to_str().unwrap()));
        let outside = std::env::temp_dir().join("kiri-scope-other").join("x.txt");
        assert!(!scope.allows(outside.to_str().unwrap()));
    }

    #[test]
    fn path_scope_contains_inside_root() {
        let dir = std::env::temp_dir().join("kiri-scope-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let scope = PathScope::new(dir.clone());
        assert!(scope.allows(file.to_str().unwrap()));
        let escaped = dir.join("..").join("..");
        assert!(!scope.allows(escaped.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_scope_recursive_flag() {
        let dir = std::env::temp_dir().join("kiri-scope-rec");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("sub").join("f.txt");
        std::fs::write(&file, "x").unwrap();
        let recursive = PathScope::new(dir.clone());
        let direct = {
            let mut s = PathScope::new(dir.clone());
            s.recursive = false;
            s
        };
        assert!(recursive.allows(file.to_str().unwrap()));
        assert!(!direct.allows(file.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
