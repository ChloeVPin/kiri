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
    pub fn allows(&self, value: &str) -> bool {
        let path = std::path::Path::new(value);
        if !path.is_absolute() {
            return false;
        }
        let Ok(canonical) = path.canonicalize() else {
            // The file may not exist yet (e.g. create-for-write); fall back to
            // a lexical containment check.
            return self.lexical_contains(path);
        };
        self.lexical_contains(&canonical)
    }

    fn lexical_contains(&self, path: &std::path::Path) -> bool {
        if self.recursive {
            path.starts_with(&self.root)
        } else {
            path.parent() == Some(self.root.as_path())
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
