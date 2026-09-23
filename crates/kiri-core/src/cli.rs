//! Capability-scoped command-line surface (`kiri.cli`).
//!
//! Closes the Tauri `process.argv` parity gap (G-5) and exceeds it: instead of
//! handing the frontend a raw, untyped `process.argv` array (Tauri's model),
//! Kiri parses the process arguments into a structured, typed view
//! (positional args + named flags + options) behind a dedicated `CLI`
//! capability. The native host owns a static argument schema (which flags are
//! even permitted), so the frontend learns only the structured, host-approved
//! slice of argv, and a granted capability still cannot read arbitrary process
//! arguments or environment the host did not declare. The `raw` array in the
//! payload is projected through the same allowlist (argv[0], positionals, and
//! declared flags/options only); it is never the verbatim process argv, so an
//! undeclared option and its value cannot leak through it. Parsing is pure and
//! dependency-free so it stays headless-testable on macOS without spawning a
//! process.

use serde_json::Value;
use std::sync::Arc;

use crate::error::Result;

/// Authorizes the `kiri.cli.*` commands.
pub const CLI_CAPABILITY: u32 = 24;

/// A parsed command-line argument surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    /// The raw argument vector (argv[0] is the executable). Internal source of
    /// truth for the projection; `CliService::describe` never returns it
    /// verbatim, it rebuilds a filtered copy through the host allowlist.
    pub raw: Vec<String>,
    /// Per-token classification parallel to `raw`, recorded during parsing so
    /// `CliService::describe` can project `raw` through the host allowlist
    /// token by token.
    roles: Vec<TokenRole>,
    /// Positional arguments (everything that is not a flag/option).
    pub positionals: Vec<String>,
    /// Named boolean flags (`--flag`, `-f`).
    pub flags: Vec<String>,
    /// Named options (`--key value`, `--key=value`, `-k value`).
    pub options: std::collections::HashMap<String, String>,
}

/// How one token in `ParsedArgs::raw` was classified during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenRole {
    /// argv[0]: the executable path.
    Executable,
    /// A positional argument (including every token after `--`).
    Positional,
    /// The `--` end-of-options marker itself.
    Separator,
    /// A token that produced one or more boolean flags (`--flag`, `-f`, or a
    /// cluster like `-abc`); holds the canonical names it produced.
    Flags(Vec<String>),
    /// A token carrying an option key (`--key=value`, `--key` with a separate
    /// value, `-k=value`); holds the canonical key.
    Option(String),
    /// The separate value token consumed by a preceding `--key`; holds the
    /// same canonical key so the value is projected with its option.
    OptionValue(String),
}

/// Parse a raw argv vector into a structured form. Pure: no IO, no process
/// access, so it is fully exercised by unit tests on any platform.
///
/// Rules (kept deliberately close to common CLI conventions so the structured
/// output matches what a developer expects from `process.argv` tooling):
/// - `--key=value` is an option.
/// - `--key value` / `-k value` is an option when the next token is not itself
///   a flag (a bare `-` or `--` token counts as a value, not a flag).
/// - `--flag` / `-f` is a boolean flag.
/// - `--` (end-of-options) makes every following token positional, matching
///   Unix semantics.
/// - Any token before the first flag is positional.
/// - argv[0] (the executable path) is always preserved in `raw` but never
///   treated as a flag/option even if it begins with `-` (executables on some
///   platforms are launchd paths with leading dashes).
pub fn parse_args(argv: &[String]) -> ParsedArgs {
    let raw = argv.to_vec();
    let mut positionals = Vec::new();
    let mut flags = Vec::new();
    let mut options = std::collections::HashMap::new();
    // argv[0] keeps the Executable role; every later index is assigned below.
    let mut roles = vec![TokenRole::Executable; raw.len()];

    let mut iter = argv.iter().enumerate().peekable();
    // Skip argv[0]; it is the executable, never a flag.
    if let Some(&(idx, _)) = iter.peek() {
        if idx == 0 {
            iter.next();
        }
    }

    let mut end_of_options = false;
    while let Some((i, token)) = iter.next() {
        if end_of_options {
            positionals.push(token.clone());
            roles[i] = TokenRole::Positional;
            continue;
        }
        if token == "--" {
            end_of_options = true;
            roles[i] = TokenRole::Separator;
            continue;
        }
        if let Some(rest) = token.strip_prefix("--") {
            // Long form: --key=value or --key value
            if let Some((key, value)) = rest.split_once('=') {
                options.insert(key.to_string(), value.to_string());
                roles[i] = TokenRole::Option(key.to_string());
            } else if let Some(next) = iter.peek() {
                // Value only if the next token is not a flag.
                // A token starting with '-' (including '--') is never a
                // value; it is a flag/end-of-options marker.
                let next_is_flag = next.1.starts_with('-');
                if next_is_flag {
                    flags.push(rest.to_string());
                    roles[i] = TokenRole::Flags(vec![rest.to_string()]);
                } else {
                    let (j, val) = iter.next().unwrap();
                    options.insert(rest.to_string(), val.clone());
                    roles[i] = TokenRole::Option(rest.to_string());
                    roles[j] = TokenRole::OptionValue(rest.to_string());
                }
            } else {
                flags.push(rest.to_string());
                roles[i] = TokenRole::Flags(vec![rest.to_string()]);
            }
        } else if let Some(rest) = token.strip_prefix('-') {
            if rest.is_empty() {
                // A bare "-" is positional.
                positionals.push(token.clone());
                roles[i] = TokenRole::Positional;
                continue;
            }
            // Short form: -k value, or clustered -abc (all flags). We treat a
            // single short option with a following non-flag value as an option,
            // otherwise as one-or-more flags.
            let mut chars = rest.chars();
            let first = chars.next().unwrap();
            let following = chars.as_str();
            if following.is_empty() {
                // -k is a boolean flag (does not consume the next token). A
                // value-bearing short option uses -= (e.g. -k=value), matching
                // the long form and keeping the default deterministic so a
                // flag never silently eats a positional.
                flags.push(first.to_string());
                roles[i] = TokenRole::Flags(vec![first.to_string()]);
            } else {
                // Clustered short flags like -abc (and possibly =value).
                if let Some((key, value)) = following.split_once('=') {
                    let key = format!("{first}{key}");
                    options.insert(key.clone(), value.to_string());
                    roles[i] = TokenRole::Option(key);
                } else {
                    let mut names = vec![first.to_string()];
                    flags.push(first.to_string());
                    for c in following.chars() {
                        flags.push(c.to_string());
                        names.push(c.to_string());
                    }
                    roles[i] = TokenRole::Flags(names);
                }
            }
        } else {
            positionals.push(token.clone());
            roles[i] = TokenRole::Positional;
        }
    }

    ParsedArgs { raw, roles, positionals, flags, options }
}

/// Host-owned CLI service. Holds the argument vector (supplied by the native
/// host at startup) and an allowlist of option/flag names the frontend is
/// permitted to see. A granted `CLI` capability still cannot read an option the
/// host did not declare, so a careless or malicious frontend cannot scrape
/// arbitrary argv (for example a `--secret` token passed at launch). Every
/// field returned by `describe` fails closed on the allowlist, including `raw`.
#[derive(Clone)]
pub struct CliService {
    parsed: ParsedArgs,
    allowed_flags: std::collections::HashSet<String>,
    allowed_options: std::collections::HashSet<String>,
}

impl CliService {
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            parsed: parse_args(&argv),
            // Default: expose nothing beyond positionals. The host tightens or
            // widens this with `with_allowlist`.
            allowed_flags: std::collections::HashSet::new(),
            allowed_options: std::collections::HashSet::new(),
        }
    }

    /// Restrict which flags/options the frontend may observe. Names match the
    /// canonical (de-dashed) key, e.g. `verbose` for `--verbose`, `k` for `-k`.
    pub fn with_allowlist(mut self, flags: Vec<String>, options: Vec<String>) -> Self {
        self.allowed_flags = flags.into_iter().collect();
        self.allowed_options = options.into_iter().collect();
        self
    }

    /// Return the host-approved structured view of argv. Every field is
    /// projected through the allowlist: `raw` is rebuilt token by token from
    /// argv[0], positionals, and declared flags/options, so an undeclared
    /// option drops both its key token and any separate value token.
    pub fn describe(&self) -> Result<Value> {
        let flags: Vec<String> =
            self.parsed.flags.iter().filter(|f| self.allowed_flags.contains(*f)).cloned().collect();
        let options: std::collections::HashMap<String, String> = self
            .parsed
            .options
            .iter()
            .filter(|(k, _)| self.allowed_options.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let raw: Vec<String> = self
            .parsed
            .raw
            .iter()
            .enumerate()
            .filter(|(i, _)| self.token_visible(*i))
            .map(|(_, t)| t.clone())
            .collect();
        Ok(serde_json::json!({
            "raw": raw,
            "positionals": self.parsed.positionals,
            "flags": flags,
            "options": options,
        }))
    }

    /// Whether the raw token at `index` may be shown to the frontend. Fails
    /// closed: a token with no recorded role is hidden, and a clustered flag
    /// token (`-abc`) is kept only when every name it produced is declared.
    fn token_visible(&self, index: usize) -> bool {
        match self.parsed.roles.get(index) {
            Some(TokenRole::Flags(names)) => names.iter().all(|n| self.allowed_flags.contains(n)),
            Some(TokenRole::Option(key)) | Some(TokenRole::OptionValue(key)) => {
                self.allowed_options.contains(key)
            }
            Some(TokenRole::Executable)
            | Some(TokenRole::Positional)
            | Some(TokenRole::Separator) => true,
            None => false,
        }
    }
}

/// Build the `kiri.cli.*` handlers bound to one CliService.
pub fn cli_handlers(
    service: CliService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(CLI_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::CLI_ARGS,
        required,
        Arc::new(move |_c, _rid, _p: &Value| {
            // The optional `full` parameter is a projection switch only: it
            // can never widen past the host allowlist, so describe() is the
            // single projected payload for both modes.
            svc.describe()
        }) as Handler,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_positionals_and_flags() {
        let p = parse_args(&v(&["/bin/kiri", "open", "--verbose", "-f", "file.txt"]));
        assert_eq!(p.raw[0], "/bin/kiri");
        assert_eq!(p.positionals, vec!["open", "file.txt"]);
        assert!(p.flags.contains(&"verbose".to_string()));
        assert!(p.flags.contains(&"f".to_string()));
    }

    #[test]
    fn parses_long_option_with_equals() {
        let p = parse_args(&v(&["app", "--mode=fast", "rest"]));
        assert_eq!(p.options.get("mode"), Some(&"fast".to_string()));
        assert_eq!(p.positionals, vec!["rest"]);
    }

    #[test]
    fn parses_long_option_with_separate_value() {
        let p = parse_args(&v(&["app", "--config", "/etc/x.toml"]));
        assert_eq!(p.options.get("config"), Some(&"/etc/x.toml".to_string()));
    }

    #[test]
    fn long_option_with_flag_value_is_flag_not_value() {
        // --flag --other : --other is a flag, so --flag has no value.
        let p = parse_args(&v(&["app", "--flag", "--other"]));
        assert!(p.flags.contains(&"flag".to_string()));
        assert!(p.flags.contains(&"other".to_string()));
        assert!(p.options.is_empty());
    }

    #[test]
    fn double_dash_ends_options() {
        let p = parse_args(&v(&["app", "--flag", "--", "--not-a-flag", "-x"]));
        assert!(p.flags.contains(&"flag".to_string()));
        assert_eq!(p.positionals, vec!["--not-a-flag", "-x"]);
    }

    #[test]
    fn short_option_with_value() {
        // Value-bearing short option uses the -= form to stay deterministic.
        let p = parse_args(&v(&["app", "-c=build"]));
        assert_eq!(p.options.get("c"), Some(&"build".to_string()));
        // A bare -c with a following token is a boolean flag, not a value.
        let q = parse_args(&v(&["app", "-c", "build"]));
        assert!(q.flags.contains(&"c".to_string()));
        assert_eq!(q.positionals, vec!["build".to_string()]);
    }

    #[test]
    fn clustered_short_flags() {
        let p = parse_args(&v(&["app", "-abc"]));
        assert_eq!(p.flags, vec!["a", "b", "c"]);
    }

    #[test]
    fn executable_leading_dash_is_not_a_flag() {
        let p = parse_args(&v(&["-weird-launchd-name", "positional"]));
        assert_eq!(p.flags.len(), 0);
        assert_eq!(p.positionals, vec!["positional"]);
    }

    #[test]
    fn allowlist_hides_undeclared_options() {
        let svc = CliService::new(v(&["app", "--secret=t0ps3cret", "--verbose"]))
            .with_allowlist(vec!["verbose".to_string()], vec![]);
        let out = svc.describe().unwrap();
        let flags = out["flags"].as_array().unwrap();
        assert_eq!(flags, &vec![Value::String("verbose".to_string())]);
        assert!(out["options"].as_object().unwrap().is_empty());
    }

    #[test]
    fn allowlist_exposes_declared_option() {
        let svc = CliService::new(v(&["app", "--secret=t0ps3cret", "--mode=fast"]))
            .with_allowlist(vec![], vec!["mode".to_string()]);
        let out = svc.describe().unwrap();
        let opts = out["options"].as_object().unwrap();
        assert_eq!(opts.get("mode"), Some(&Value::String("fast".to_string())));
        assert!(!opts.contains_key("secret"));
    }

    #[test]
    fn raw_is_projected_through_allowlist() {
        // Regression lock: describe() used to return self.parsed.raw verbatim,
        // so a granted CLI capability could still read an undeclared option.
        let svc = CliService::new(v(&[
            "app",
            "--secret=t0ps3cret",
            "--token",
            "hunter2",
            "--verbose",
            "--mode=fast",
            "run",
        ]))
        .with_allowlist(vec!["verbose".to_string()], vec!["mode".to_string()]);
        let out = svc.describe().unwrap();
        let blob = serde_json::to_string(&out).unwrap();
        for needle in ["secret", "t0ps3cret", "token", "hunter2"] {
            assert!(!blob.contains(needle), "payload leaked {needle}: {blob}");
        }
        // argv[0], declared flags/options, and positionals survive, in order.
        assert_eq!(
            out["raw"].as_array().unwrap(),
            &vec![
                Value::String("app".to_string()),
                Value::String("--verbose".to_string()),
                Value::String("--mode=fast".to_string()),
                Value::String("run".to_string()),
            ]
        );
        assert_eq!(out["flags"], serde_json::json!(["verbose"]));
        assert_eq!(out["options"], serde_json::json!({ "mode": "fast" }));
        assert_eq!(out["positionals"], serde_json::json!(["run"]));
    }

    #[test]
    fn empty_allowlist_raw_still_hides_secret() {
        let svc =
            CliService::new(v(&["app", "--secret=t0ps3cret", "--key", "hunter2", "file.txt"]));
        let out = svc.describe().unwrap();
        assert!(out["flags"].as_array().unwrap().is_empty());
        assert!(out["options"].as_object().unwrap().is_empty());
        let blob = serde_json::to_string(&out).unwrap();
        for needle in ["secret", "t0ps3cret", "key", "hunter2"] {
            assert!(!blob.contains(needle), "payload leaked {needle}: {blob}");
        }
        assert_eq!(
            out["raw"].as_array().unwrap(),
            &vec![Value::String("app".to_string()), Value::String("file.txt".to_string())]
        );
    }

    #[test]
    fn full_param_cannot_widen_past_allowlist() {
        let svc = CliService::new(v(&["app", "--secret=t0ps3cret", "--verbose"]))
            .with_allowlist(vec!["verbose".to_string()], vec![]);
        let handlers = cli_handlers(svc);
        let (_, _, handler) = &handlers[0];
        for payload in [serde_json::json!(null), serde_json::json!({ "full": true })] {
            let out = handler(crate::caller::CallerId(1), 1, &payload).unwrap();
            let blob = serde_json::to_string(&out).unwrap();
            assert!(!blob.contains("secret"), "full mode leaked name: {blob}");
            assert!(!blob.contains("t0ps3cret"), "full mode leaked value: {blob}");
        }
    }
}
