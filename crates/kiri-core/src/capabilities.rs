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
        let root_norm = normalize_var(&self.root);
        candidates
            .iter()
            .any(|c| lexical_contains_norm(&root_norm, &normalize_var(c), self.recursive))
    }
}

/// Normalize the macOS `/private/var` <-> `/var` symlink equivalence so two
/// paths that refer to the same location compare equal regardless of which
/// prefix the OS reported.
fn normalize_var(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix("/private/var/") {
        std::path::PathBuf::from(format!("/var/{rest}"))
    } else if let Some(rest) = s.strip_prefix("/private/") {
        // General /private/* normalization (covers /private/tmp etc.).
        std::path::PathBuf::from(format!("/{rest}"))
    } else {
        p.to_path_buf()
    }
}

/// Lexically normalize a path by resolving `.` and `..` components without
/// touching the filesystem. This makes `starts_with` containment safe: an
/// escape like `root/../../etc/passwd` collapses to `root/../etc/passwd`,
/// which no longer starts with `root`. `root` is already canonical and free
/// of `..`, so only the candidate needs collapsing.
fn lexical_normalize(p: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Drop the last normal segment; keep leading `..` if present.
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Lexical containment on already `/var`-normalized paths. The candidate is
/// first collapsed so `..` escapes cannot defeat `starts_with`.
fn lexical_contains_norm(root: &std::path::Path, path: &std::path::Path, recursive: bool) -> bool {
    let path = lexical_normalize(path);
    if recursive {
        path.starts_with(root)
    } else {
        path.parent() == Some(root)
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
