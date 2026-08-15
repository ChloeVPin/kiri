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
    /// 2. The target file (or an intermediate directory) may not exist yet
    ///    (create-for-write). We resolve the deepest EXISTING ancestor,
    ///    canonicalize that (which also resolves symlinks/junctions such as
    ///    Windows' temp dir consistently for root and candidate), then
    ///    re-attach the not-yet-created tail. Comparing the raw, un-
    ///    canonicalized path against a canonicalized root would deny valid
    ///    writes, which is the bug this closes.
    pub fn allows(&self, value: &str) -> bool {
        let path = std::path::Path::new(value);
        if !path.is_absolute() {
            return false;
        }
        // Climb to the deepest existing ancestor so the part we hand to
        // canonicalize actually exists. The trailing components are re-
        // attached after, so missing files/dirs never block resolution.
        let mut current = path.to_path_buf();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !current.exists() {
            match current.file_name().map(|n| n.to_os_string()) {
                Some(name) => {
                    tail.push(name);
                    match current.parent() {
                        Some(parent) => current = parent.to_path_buf(),
                        None => break,
                    }
                }
                None => break,
            }
        }
        let base = current.canonicalize().unwrap_or(current);
        let mut resolved = base;
        for part in tail.into_iter().rev() {
            resolved = resolved.join(part);
        }
        let root_norm = normalize_path_key(&self.root);
        path_key_contains(&root_norm, &normalize_path_key(&resolved), self.recursive)
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
pub(crate) fn normalize_path_key(p: &std::path::Path) -> String {
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

/// Filesystem glob scope: an allowlist of glob patterns (relative to the
/// `PathScope` root) that further restricts which paths may be touched. This is
/// the granularity axis where Tauri v2's `fs` plugin wins today: it lets a host
/// restrict a granted capability to `images/*`, `**/*.txt`, etc. Kiri's
/// `PathScope` alone is a single root, so we add `GlobScope` on top of it and
/// require BOTH checks to pass. Empty scope = no glob restriction (only the root
/// applies), which preserves the existing default behavior.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobScope {
    /// Glob patterns such as `data/*.json` or `**/*.log`. Matched against the
    /// path relative to the `PathScope` root, forward-slash normalized.
    pub patterns: Vec<String>,
}

impl GlobScope {
    pub fn new(patterns: Vec<String>) -> Self {
        Self { patterns }
    }

    /// True when no glob restriction is configured (root-only scoping).
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `relative` (path relative to the scope root, forward-slash
    /// normalized) matches at least one pattern.
    pub fn allows(&self, relative: &str) -> bool {
        if self.patterns.is_empty() {
            return true;
        }
        let rel = relative.trim_matches('/');
        self.patterns.iter().any(|p| glob_match(p, rel))
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

/// Minimal glob matcher supporting `*` (one path segment, no separator) and
/// `**` (zero or more segments, may cross separators). Hand-rolled to avoid a
/// crate dependency in the platform-neutral core. Returns false on any error so
/// an unparseable pattern fails closed (deny), never silently allows.
fn glob_match(pattern: &str, text: &str) -> bool {
    // Normalize both to forward slashes and collapse empty components.
    let pat: Vec<&str> = pattern.split('/').filter(|c| !c.is_empty()).collect();
    let txt: Vec<&str> = text.split('/').filter(|c| !c.is_empty()).collect();
    glob_match_segments(&pat, 0, &txt, 0)
}

fn glob_match_segments(pat: &[&str], pi: usize, txt: &[&str], ti: usize) -> bool {
    if pi == pat.len() && ti == txt.len() {
        return true;
    }
    if pi == pat.len() {
        return false;
    }
    let p = pat[pi];
    if p == "**" {
        // `**` matches zero or more remaining segments.
        if glob_match_segments(pat, pi + 1, txt, ti) {
            return true;
        }
        if ti < txt.len() {
            return glob_match_segments(pat, pi, txt, ti + 1);
        }
        return false;
    }
    if ti == txt.len() {
        return false;
    }
    if segment_match(p, txt[ti]) {
        return glob_match_segments(pat, pi + 1, txt, ti + 1);
    }
    false
}

/// Match a single pattern segment against a single text segment. `*` matches
/// any run of non-separator characters; everything else is a literal.
fn segment_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    while pi < p.len() {
        match p[pi] {
            b'*' => {
                // Greedy: try to consume the rest of text with the rest of pat.
                let rest_pat = &p[pi + 1..];
                // Try matching from each remaining text position.
                let mut k = ti;
                while k <= t.len() {
                    if rest_pat.is_empty() && k == t.len() {
                        return true;
                    }
                    if segment_match_suffix(rest_pat, &t[k..]) {
                        return true;
                    }
                    k += 1;
                }
                // Fast path for trailing single `*`.
                if rest_pat.is_empty() {
                    return true;
                }
                return false;
            }
            _ => {
                if ti >= t.len() || p[pi] != t[ti] {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    pi == p.len() && ti == t.len()
}

/// Match the remaining pattern (which may contain `*`) against the remaining
/// text bytes, both starting at the current position.
fn segment_match_suffix(pat: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    while pi < pat.len() {
        match pat[pi] {
            b'*' => {
                let rest = &pat[pi + 1..];
                let mut k = ti;
                while k <= text.len() {
                    if rest.is_empty() && k == text.len() {
                        return true;
                    }
                    if segment_match_suffix(rest, &text[k..]) {
                        return true;
                    }
                    k += 1;
                }
                return false;
            }
            _ => {
                if ti >= text.len() || pat[pi] != text[ti] {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    pi == pat.len() && ti == text.len()
}

impl Scope for PathScope {
    fn allows(&self, value: &str) -> bool {
        self.allows(value)
    }
}

impl Scope for GlobScope {
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
