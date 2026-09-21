//! Small path helpers shared across the crate.

use std::env;
use std::path::PathBuf;

/// A non-empty environment variable, or `None` — matches Python's `if d:`
/// truthiness check on `os.environ.get(...)`, where an empty string is
/// treated the same as unset. `pub(crate)`: `herdrcli` and `verbs` both
/// read env vars with this same truthiness rule.
pub(crate) fn non_empty_env(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// The invoking user's home directory, read from `$HOME` the way Python's
/// `Path.home()` does on POSIX. There is no stdlib fallback to a passwd
/// database lookup here (that would need an extra dependency); an unset
/// `$HOME` yields an empty path, so a leading `~` is left as `~` by
/// `expand` below rather than resolving to nothing useful.
pub(crate) fn home_dir() -> PathBuf {
    match non_empty_env("HOME") {
        Some(h) => PathBuf::from(h),
        None => PathBuf::new(),
    }
}

/// `$VAR` and `${VAR}` expansion, matching Python's `os.path.expandvars`:
/// an unset or malformed reference is left in the string unexpanded.
fn expand_vars(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'{' {
                if let Some(end) = s[i + 2..].find('}') {
                    let name = &s[i + 2..i + 2 + end];
                    if let Ok(val) = env::var(name) {
                        out.push_str(&val);
                        i = i + 2 + end + 1;
                        continue;
                    }
                }
            } else if is_word_byte(bytes[i + 1]) {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && is_word_byte(bytes[end]) {
                    end += 1;
                }
                let name = &s[start..end];
                if let Ok(val) = env::var(name) {
                    out.push_str(&val);
                    i = end;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().expect("i is a valid char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// A leading `~` or `~/rest` expanded against `$HOME`. `~otheruser` (a
/// different user's home, via a passwd lookup in Python) is left
/// unexpanded — not reachable from any estate or campaign file in this
/// project, and not worth an extra dependency.
fn expand_user(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(p)
}

/// `~` and `$VAR` expansion, the same tolerance campaign files and the
/// estate table both rely on.
pub fn expand<S: AsRef<str>>(p: S) -> PathBuf {
    expand_user(&expand_vars(p.as_ref()))
}

/// Where this plugin's own runtime files (assembled prompts, worktree
/// checkouts) live: `$HERDR_PLUGIN_STATE_DIR` when herdr sets it (running
/// as a plugin action), else a plain XDG-ish fallback for a bare shell
/// invocation.
pub fn state_dir() -> PathBuf {
    if let Some(d) = non_empty_env("HERDR_PLUGIN_STATE_DIR") {
        return PathBuf::from(d);
    }
    let base = match non_empty_env("XDG_STATE_HOME") {
        Some(x) => PathBuf::from(x),
        None => home_dir().join(".local").join("state"),
    };
    base.join("herdr-estate")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvGuard;

    #[test]
    fn expand_leaves_plain_path_alone() {
        let _g = EnvGuard::unset(&["HOME"]);
        assert_eq!(expand("/a/b"), PathBuf::from("/a/b"));
    }

    #[test]
    fn expand_tilde_uses_home() {
        let _g = EnvGuard::set(&[("HOME", "/tmp/fake-home")]);
        assert_eq!(expand("~/code"), PathBuf::from("/tmp/fake-home/code"));
    }

    #[test]
    fn expand_bare_tilde_is_home() {
        let _g = EnvGuard::set(&[("HOME", "/tmp/fake-home")]);
        assert_eq!(expand("~"), PathBuf::from("/tmp/fake-home"));
    }

    #[test]
    fn expand_dollar_var() {
        let _g = EnvGuard::set(&[("HERDR_ESTATE_TEST_VAR", "value")]);
        assert_eq!(expand("$HERDR_ESTATE_TEST_VAR/x"), PathBuf::from("value/x"));
    }

    #[test]
    fn expand_braced_var() {
        let _g = EnvGuard::set(&[("HERDR_ESTATE_TEST_VAR", "value")]);
        assert_eq!(
            expand("${HERDR_ESTATE_TEST_VAR}/x"),
            PathBuf::from("value/x")
        );
    }

    #[test]
    fn expand_unset_var_left_literal() {
        let _g = EnvGuard::unset(&["HERDR_ESTATE_TEST_UNSET"]);
        assert_eq!(
            expand("$HERDR_ESTATE_TEST_UNSET/x"),
            PathBuf::from("$HERDR_ESTATE_TEST_UNSET/x")
        );
    }

    #[test]
    fn state_dir_uses_plugin_state_dir_when_set() {
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", "/tmp/plugin-state")]);
        assert_eq!(state_dir(), PathBuf::from("/tmp/plugin-state"));
    }

    #[test]
    fn state_dir_falls_back_to_xdg_state_home() {
        let _g = EnvGuard::new(
            &[("XDG_STATE_HOME", "/tmp/xdg-state")],
            &["HERDR_PLUGIN_STATE_DIR"],
        );
        assert_eq!(
            state_dir(),
            PathBuf::from("/tmp/xdg-state").join("herdr-estate")
        );
    }

    #[test]
    fn state_dir_falls_back_to_home_local_state() {
        let _g = EnvGuard::new(
            &[("HOME", "/tmp/fake-home")],
            &["HERDR_PLUGIN_STATE_DIR", "XDG_STATE_HOME"],
        );
        assert_eq!(
            state_dir(),
            PathBuf::from("/tmp/fake-home/.local/state/herdr-estate")
        );
    }

    #[test]
    fn empty_env_value_counts_as_unset() {
        let _g = EnvGuard::new(
            &[("HERDR_PLUGIN_STATE_DIR", ""), ("HOME", "/tmp/fake-home")],
            &["XDG_STATE_HOME"],
        );
        assert_eq!(
            state_dir(),
            PathBuf::from("/tmp/fake-home/.local/state/herdr-estate")
        );
    }
}
