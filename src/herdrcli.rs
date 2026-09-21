//! The one seam between this crate and the herdr CLI itself.
//!
//! Every herdr invocation anywhere in this crate goes through
//! `call_herdr`. Tests point `HERDR_BIN_PATH` at a fake executable
//! (`crate::test_support::FakeHerdr`) and never run a real herdr binary.

use std::collections::BTreeMap;
use std::process::Command;

use serde::Deserialize;

use crate::errors::EstateError;
use crate::paths::non_empty_env;

/// Dropped from the environment of every herdr call: a stale API key (the
/// session should authenticate via the parent shell's own login) and
/// TMUX*/CLAUDE* markers that describe *this* process, not the workspace
/// herdr is about to create. Nothing under HERDR_* matches either rule, so
/// the control socket and the rest of herdr's own plugin env ride through
/// untouched.
const ENV_SCRUB_EXACT: &[&str] = &["ANTHROPIC_API_KEY"];
const ENV_SCRUB_PREFIX: &[&str] = &["TMUX", "CLAUDE"];

pub fn scrub_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .filter(|(k, _)| {
            !ENV_SCRUB_EXACT.contains(&k.as_str())
                && !ENV_SCRUB_PREFIX.iter().any(|p| k.starts_with(p))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Raised internally by `call_herdr(check: true)` on a non-zero herdr
/// exit. A separate type from `EstateError` — matching where Python
/// defines it, in this same module, distinct from `errors.py`'s four
/// subclasses — that converts into `EstateError::Protocol` at the point
/// `call_herdr` returns it, so every caller downstream of the herdr
/// boundary still deals with the one crate-wide error enum.
#[derive(Debug)]
pub struct HerdrError {
    pub herdr_args: Vec<String>,
    pub returncode: i32,
    pub stderr: String,
    pub stdout: String,
}

impl std::fmt::Display for HerdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw: &str = if !self.stderr.is_empty() {
            &self.stderr
        } else if !self.stdout.is_empty() {
            &self.stdout
        } else {
            ""
        };
        let detail: String = raw.trim().chars().take(300).collect();
        write!(f, "herdr {} failed: {detail}", self.herdr_args.join(" "))
    }
}

impl std::error::Error for HerdrError {}

impl From<HerdrError> for EstateError {
    fn from(e: HerdrError) -> Self {
        EstateError::Protocol(e.to_string())
    }
}

/// `agent_status` off a `workspace list`/`workspace get` row. Fails
/// closed: `is_alive` below treats everything except exactly `Unknown`
/// as alive, including a value this port has never seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Idle,
    Done,
    Unknown,
    Other(String),
}

impl<'de> Deserialize<'de> for AgentStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "working" => AgentStatus::Working,
            "idle" => AgentStatus::Idle,
            "done" => AgentStatus::Done,
            "unknown" => AgentStatus::Unknown,
            _ => AgentStatus::Other(s),
        })
    }
}

impl AgentStatus {
    /// The wire value this variant round-trips from, for display (the
    /// picker's row list shows this next to a live campaign).
    pub fn as_str(&self) -> &str {
        match self {
            AgentStatus::Working => "working",
            AgentStatus::Idle => "idle",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
            AgentStatus::Other(s) => s,
        }
    }
}

/// One row of `workspace list`'s `workspaces` array. `workspace_id` and
/// `label` are exactly the fields the ownership and alive guards depend
/// on, so neither carries `#[serde(default)]`: a row herdr sent without
/// one fails the whole listing (`ProtocolError`) rather than silently
/// dropping out of the guard's view the way a `.get()` default would.
/// `agent_status` is the one field this port was explicitly told counts
/// as alive when absent, so its `Option` carries the default instead —
/// `Option`'s own `Deserialize` treats a missing key and an explicit
/// `null` the same way, matching both cases in one rule.
#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceListResult {
    pub workspaces: Vec<Workspace>,
}

/// `workspace get`'s envelope: tolerant, because a workspace herdr
/// itself carries may have no label at all — an ordinary condition
/// callers translate into their own error (`UsageError`, a skipped
/// close), never a `ProtocolError` on its own.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceGetResult {
    #[serde(default)]
    pub workspace: Option<WorkspaceRef>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceRef {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// `workspace create`'s envelope. Both ids ARE guard-critical here —
/// every later call in the verb addresses this exact workspace and pane
/// — so the caller (`verbs::created_ids`) turns a missing one into its
/// own `ProtocolError`, naming the field, rather than this type failing
/// to parse at all.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceCreateResult {
    #[serde(default)]
    pub workspace: Option<WorkspaceRef>,
    #[serde(default)]
    pub root_pane: Option<PaneRef>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaneRef {
    #[serde(default)]
    pub pane_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HerdrResult {
    pub ok: bool,
    pub returncode: i32,
    /// The `"result"` object of herdr's `{"result": ...}` envelope, or
    /// the whole parsed payload when it carried no `"result"` key.
    /// Always a JSON object — empty when herdr's stdout was empty,
    /// malformed, or shaped as something other than an object.
    pub data: serde_json::Value,
    pub stderr: String,
}

/// `$HERDR_BIN_PATH` when set to a non-empty value, else the bare name
/// `herdr` for `Command` to resolve against `$PATH` — split out as its
/// own pure function so the fallback is unit-testable without spawning
/// anything (a plugin test must never spawn a real `herdr`).
fn resolve_bin_path() -> String {
    non_empty_env("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".to_string())
}

/// Run `$HERDR_BIN_PATH <args>` (falling back to `herdr` on `$PATH`),
/// scrubbing the child's environment, and parse its JSON envelope into
/// `HerdrResult.data`.
///
/// `check: true` turns a non-zero herdr exit into an `Err`; pass `false`
/// for a probe that reports failure as `HerdrResult.ok == false` instead
/// (the label-match-before-close pattern needs this).
pub fn call_herdr(args: &[String], check: bool) -> Result<HerdrResult, EstateError> {
    let bin_path = resolve_bin_path();
    let scrubbed = scrub_env(&std::env::vars().collect());

    let output = Command::new(&bin_path)
        .args(args)
        .env_clear()
        .envs(&scrubbed)
        .output()
        .map_err(|e| EstateError::Protocol(format!("failed to run {bin_path}: {e}")))?;

    let returncode = output.status.code().unwrap_or(-1);
    let ok = returncode == 0;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if check && !ok {
        return Err(EstateError::from(HerdrError {
            herdr_args: args.to_vec(),
            returncode,
            stderr,
            stdout,
        }));
    }

    let mut data = serde_json::Value::Object(serde_json::Map::new());
    if ok && !stdout.trim().is_empty() {
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(&stdout)
        {
            let payload = serde_json::Value::Object(map.clone());
            let result = map.get("result").cloned().unwrap_or(payload);
            if result.is_object() {
                data = result;
            }
        }
    }

    Ok(HerdrResult {
        ok,
        returncode,
        data,
        stderr,
    })
}

/// Every workspace herdr currently has open, via `workspace list`. A
/// non-zero exit (`call_herdr`'s `check: true`) or a response that
/// doesn't parse into a `workspaces` array both raise `ProtocolError` —
/// callers (the ownership/alive guards, the picker's row list) must
/// never treat that as "no workspaces" and carry on over a half-true
/// read.
pub fn workspace_list() -> Result<Vec<Workspace>, EstateError> {
    let result = call_herdr(&["workspace".to_string(), "list".to_string()], true)?;
    let list: WorkspaceListResult = serde_json::from_value(result.data).map_err(|_| {
        EstateError::Protocol("workspace list: response carried no 'workspaces' array".to_string())
    })?;
    Ok(list.workspaces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, FakeHerdr};

    #[test]
    fn scrub_drops_anthropic_api_key() {
        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-x".to_string());
        env.insert("PATH".to_string(), "/bin".to_string());
        let scrubbed = scrub_env(&env);
        assert!(!scrubbed.contains_key("ANTHROPIC_API_KEY"));
        assert!(scrubbed.contains_key("PATH"));
    }

    #[test]
    fn scrub_drops_tmux_prefixed() {
        let mut env = BTreeMap::new();
        env.insert("TMUX".to_string(), "1".to_string());
        env.insert("TMUX_PANE".to_string(), "%3".to_string());
        env.insert("PATH".to_string(), "/bin".to_string());
        let scrubbed = scrub_env(&env);
        assert!(!scrubbed.contains_key("TMUX"));
        assert!(!scrubbed.contains_key("TMUX_PANE"));
    }

    #[test]
    fn scrub_drops_claude_prefixed() {
        let mut env = BTreeMap::new();
        env.insert("CLAUDECODE".to_string(), "1".to_string());
        env.insert("CLAUDE_FOO".to_string(), "x".to_string());
        env.insert("PATH".to_string(), "/bin".to_string());
        let scrubbed = scrub_env(&env);
        assert!(!scrubbed.contains_key("CLAUDECODE"));
        assert!(!scrubbed.contains_key("CLAUDE_FOO"));
    }

    fn deserialize_status(s: &str) -> AgentStatus {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    #[test]
    fn agent_status_working_is_exactly_working() {
        assert_eq!(deserialize_status("working"), AgentStatus::Working);
    }

    #[test]
    fn agent_status_idle_is_exactly_idle() {
        assert_eq!(deserialize_status("idle"), AgentStatus::Idle);
    }

    #[test]
    fn agent_status_done_is_exactly_done() {
        assert_eq!(deserialize_status("done"), AgentStatus::Done);
    }

    #[test]
    fn agent_status_unknown_is_exactly_unknown() {
        assert_eq!(deserialize_status("unknown"), AgentStatus::Unknown);
    }

    #[test]
    fn scrub_leaves_control_socket_untouched() {
        let mut env = BTreeMap::new();
        env.insert("HERDR_SOCKET_PATH".to_string(), "/tmp/sock".to_string());
        env.insert("HERDR_WORKSPACE_ID".to_string(), "w1".to_string());
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-x".to_string());
        let scrubbed = scrub_env(&env);
        assert_eq!(scrubbed["HERDR_SOCKET_PATH"], "/tmp/sock");
        assert_eq!(scrubbed["HERDR_WORKSPACE_ID"], "w1");
    }

    #[test]
    fn resolve_bin_path_falls_back_to_bare_herdr() {
        let _g = EnvGuard::unset(&["HERDR_BIN_PATH"]);
        assert_eq!(resolve_bin_path(), "herdr");
    }

    #[test]
    fn resolve_bin_path_uses_env_when_set() {
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", "/opt/herdr/herdr")]);
        assert_eq!(resolve_bin_path(), "/opt/herdr/herdr");
    }

    #[test]
    fn call_herdr_uses_herdr_bin_path_and_sends_exact_argv() {
        let fake = FakeHerdr::new();
        fake.set_response(0, r#"{"result": {}}"#, 0);
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        call_herdr(&["--version".to_string()], true).unwrap();
        assert_eq!(fake.argv(0), vec!["--version"]);
    }

    #[test]
    fn call_herdr_parses_result_envelope() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        let result = call_herdr(
            &["workspace".to_string(), "get".to_string(), "w1".to_string()],
            true,
        )
        .unwrap();
        assert!(result.ok);
        assert_eq!(
            result.data["workspace"]["label"].as_str(),
            Some("home/garden")
        );
    }

    #[test]
    fn call_herdr_check_true_raises_on_failure() {
        let fake = FakeHerdr::new();
        fake.set_response(0, "", 1);
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        let err = call_herdr(
            &[
                "workspace".to_string(),
                "close".to_string(),
                "w1".to_string(),
            ],
            true,
        )
        .unwrap_err();
        assert!(matches!(err, EstateError::Protocol(_)));
    }

    #[test]
    fn call_herdr_check_false_reports_failure_without_raising() {
        let fake = FakeHerdr::new();
        fake.set_response(0, "", 1);
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        let result = call_herdr(
            &["workspace".to_string(), "get".to_string(), "w1".to_string()],
            false,
        )
        .unwrap();
        assert!(!result.ok);
        assert_eq!(
            result.data,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn call_herdr_check_false_failure_with_json_stdout_still_yields_empty_data() {
        // A non-zero exit means the JSON on stdout, if any, describes an
        // error — never parsed into `data`, whether or not it happens to
        // be well-formed.
        let fake = FakeHerdr::new();
        fake.set_response(0, r#"{"result": {"workspace": {"label": "x"}}}"#, 1);
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        let result = call_herdr(
            &["workspace".to_string(), "get".to_string(), "w1".to_string()],
            false,
        )
        .unwrap();
        assert!(!result.ok);
        assert_eq!(
            result.data,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn call_herdr_malformed_json_yields_empty_data() {
        let fake = FakeHerdr::new();
        fake.set_response(0, "not json", 0);
        let _g = EnvGuard::set(&[(
            "HERDR_BIN_PATH",
            fake.bin_path().to_str().expect("utf8 path"),
        )]);
        let result = call_herdr(&["workspace".to_string(), "list".to_string()], true).unwrap();
        assert!(result.ok);
        assert_eq!(
            result.data,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn call_herdr_scrubs_env_passed_to_the_child() {
        let fake = FakeHerdr::new();
        fake.set_response(0, r#"{"result": {}}"#, 0);
        let _g = EnvGuard::set(&[
            (
                "HERDR_BIN_PATH",
                fake.bin_path().to_str().expect("utf8 path"),
            ),
            ("ANTHROPIC_API_KEY", "sk-x"),
            ("HERDR_SOCKET_PATH", "/tmp/sock"),
        ]);
        call_herdr(&["--version".to_string()], true).unwrap();
        assert!(fake.env_value(0, "ANTHROPIC_API_KEY").is_none());
        assert_eq!(
            fake.env_value(0, "HERDR_SOCKET_PATH"),
            Some("/tmp/sock".to_string())
        );
    }
}
