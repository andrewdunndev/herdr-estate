//! What herdr-estate says while it works. Every step that changes the
//! board prints one line to stderr and appends the same line, timestamped,
//! to `events.log` in the state dir.
//!
//! The two sinks cover the two ways this binary runs. In the picker popup
//! or a shell, stderr is the terminal: `agent start` blocks until the
//! agent is ready for input, several seconds for claude, and with nothing
//! on screen that wait reads as a hang. Run as a herdr plugin action there
//! is no terminal: herdr keeps that run's output in `herdr plugin log
//! list`, but nothing records a picker popup's run once it closes. The
//! log is the one record that spans every launch path, one timestamped
//! line per step.

use std::fmt::Display;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use crate::continuity::now_utc_iso;
use crate::paths::{non_empty_env, state_dir};

pub const LOG_FILE_NAME: &str = "events.log";

/// `events.log` under `paths::state_dir()`: herdr's per-plugin state dir
/// when herdr ran this binary, `~/.local/state/herdr-estate` from a shell
/// — the same split the ownership record has.
pub fn log_path() -> PathBuf {
    state_dir().join(LOG_FILE_NAME)
}

/// One step: `herdr-estate: <msg>` on stderr, then the same text in the
/// log.
pub fn step(msg: &str) {
    say(&format!("herdr-estate: {msg}"));
    append(msg);
}

/// A failure the run is about to exit on, printed the way `cli::run` has
/// always printed one (`herdr-estate: <message>`) and logged as
/// `error: <message>`, so the log shows where a run stopped.
pub fn report_error(err: &dyn Display) {
    say(&format!("herdr-estate: {err}"));
    append(&format!("error: {err}"));
}

/// One line to stderr that can never panic. `eprintln!` panics when the
/// write fails (a closed pipe, say), and a step runs between `workspace
/// create` and `agent start`: a line nobody can read is never worth
/// stopping a verb halfway.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// How this process was started, from the variables herdr sets on each
/// of its launch paths: an action gets `HERDR_PLUGIN_ACTION_ID`, a plugin
/// pane (the picker popup) gets `HERDR_PLUGIN_ENTRYPOINT_ID`, and a
/// shell sets neither. The log's run line carries it, because the same
/// argv (`pick`) behaves differently on each path.
pub fn launched_via() -> String {
    if let Some(id) = non_empty_env("HERDR_PLUGIN_ACTION_ID") {
        return format!("herdr action {id}");
    }
    if let Some(id) = non_empty_env("HERDR_PLUGIN_ENTRYPOINT_ID") {
        return format!("herdr pane {id}");
    }
    "shell".to_string()
}

/// Append `<time> <msg>` to the log, newlines folded to ` | ` so one
/// event is always one line (a herdr failure's message carries its
/// stderr). Created owner-only (0600): error text lands here, and a
/// herdr failure echoes herdr's own output. Best-effort by design: a
/// state dir that can't be created or written never fails the verb
/// reporting through it, since the verb's own result still stands.
pub(crate) fn append(msg: &str) {
    // A test that reaches this with no scratch state dir would write the
    // operator's real events.log, the way an unguarded test once wrote
    // the real owned.json. Fail that test instead.
    #[cfg(test)]
    assert!(
        non_empty_env("HERDR_PLUGIN_STATE_DIR").is_some(),
        "progress::append reached with HERDR_PLUGIN_STATE_DIR unset: this test would write the real events.log"
    );
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let line = msg.lines().collect::<Vec<_>>().join(" | ");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
    {
        let _ = writeln!(f, "{} {}", now_utc_iso(), line);
    }
}

/// The log's lines with their timestamps stripped, oldest first — what
/// a test compares against. Empty when there is no log yet.
#[cfg(test)]
pub(crate) fn logged_messages() -> Vec<String> {
    std::fs::read_to_string(log_path())
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map(|(_, m)| m).unwrap_or("").to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    #[test]
    fn step_appends_one_timestamped_line_per_call_in_order() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        step("first");
        step("second");
        let raw = std::fs::read_to_string(d.path().join(LOG_FILE_NAME)).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        // `<ISO UTC time> <message>`: the continuity log's timestamp shape.
        assert!(lines[0].ends_with(" first"));
        assert!(lines[0].contains('T') && lines[0].contains("+00:00 "));
        assert!(lines[1].ends_with(" second"));
    }

    #[test]
    fn the_log_is_created_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        append("x");
        let mode = std::fs::metadata(d.path().join(LOG_FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn append_creates_a_missing_state_dir() {
        let d = TempDir::new();
        let nested = d.path().join("not").join("yet");
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", nested.to_str().unwrap())]);
        append("hello");
        assert_eq!(logged_messages(), vec!["hello"]);
    }

    #[test]
    fn append_folds_a_multi_line_message_onto_one_line() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        append("herdr failed\nstderr: boom\nstdout:");
        assert_eq!(
            logged_messages(),
            vec!["herdr failed | stderr: boom | stdout:"]
        );
    }

    #[test]
    fn append_to_an_unwritable_state_dir_does_not_panic() {
        // The state dir path is an existing *file*: create_dir_all and
        // open both fail, and the step must still return normally.
        let d = TempDir::new();
        let blocker = d.path().join("a-file");
        std::fs::write(&blocker, "x").unwrap();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", blocker.to_str().unwrap())]);
        step("still fine");
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "x");
    }

    #[test]
    fn report_error_logs_with_an_error_prefix() {
        let d = TempDir::new();
        let _g = EnvGuard::set(&[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())]);
        report_error(&"home/garden: refused");
        assert_eq!(logged_messages(), vec!["error: home/garden: refused"]);
    }

    #[test]
    fn launched_via_names_an_action() {
        let _g = EnvGuard::new(
            &[("HERDR_PLUGIN_ACTION_ID", "pick")],
            &["HERDR_PLUGIN_ENTRYPOINT_ID"],
        );
        assert_eq!(launched_via(), "herdr action pick");
    }

    #[test]
    fn launched_via_names_a_plugin_pane() {
        let _g = EnvGuard::new(
            &[("HERDR_PLUGIN_ENTRYPOINT_ID", "picker")],
            &["HERDR_PLUGIN_ACTION_ID"],
        );
        assert_eq!(launched_via(), "herdr pane picker");
    }

    #[test]
    fn launched_via_prefers_the_action_when_both_are_set() {
        let _g = EnvGuard::set(&[
            ("HERDR_PLUGIN_ACTION_ID", "pick"),
            ("HERDR_PLUGIN_ENTRYPOINT_ID", "picker"),
        ]);
        assert_eq!(launched_via(), "herdr action pick");
    }

    #[test]
    fn launched_via_is_shell_when_herdr_set_neither() {
        let _g = EnvGuard::unset(&["HERDR_PLUGIN_ACTION_ID", "HERDR_PLUGIN_ENTRYPOINT_ID"]);
        assert_eq!(launched_via(), "shell");
    }
}
