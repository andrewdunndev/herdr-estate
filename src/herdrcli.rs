use std::collections::BTreeMap;
use std::process::Command;

use serde::Deserialize;

use crate::errors::EstateError;
use crate::paths::non_empty_env;

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

pub(crate) const ENV_VALUE_MASK: &str = "<from estates.toml>";

fn redacted_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut after_env = false;
    for a in args {
        out.push(match a.split_once('=') {
            Some((k, v)) if after_env && !v.is_empty() && k != "HERDR_ESTATE_ADDRESS" => {
                format!("{k}={ENV_VALUE_MASK}")
            }
            _ => a.clone(),
        });
        after_env = a == "--env";
    }
    out
}

fn failure_message(herdr_args: &[String], stderr: &str, stdout: &str) -> String {
    let raw: &str = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        ""
    };
    let detail: String = raw.trim().chars().take(300).collect();
    format!(
        "herdr {} failed: {detail}",
        redacted_args(herdr_args).join(" ")
    )
}

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
        Ok(AgentStatus::from_wire(&String::deserialize(deserializer)?))
    }
}

impl AgentStatus {
    pub fn from_wire(s: &str) -> AgentStatus {
        match s {
            "working" => AgentStatus::Working,
            "idle" => AgentStatus::Idle,
            "done" => AgentStatus::Done,
            "unknown" => AgentStatus::Unknown,
            _ => AgentStatus::Other(s.to_string()),
        }
    }

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

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
    #[serde(default)]
    pub pane_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceListResult {
    pub workspaces: Vec<Workspace>,
}

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
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
    #[serde(default)]
    pub pane_count: Option<usize>,
}

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
    pub data: serde_json::Value,
}

fn resolve_bin_path() -> String {
    non_empty_env("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".to_string())
}

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
        return Err(EstateError::Protocol(failure_message(
            args, &stderr, &stdout,
        )));
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

    Ok(HerdrResult { ok, data })
}

pub fn call_api(method: &str, params: serde_json::Value) -> Result<serde_json::Value, EstateError> {
    use std::io::{BufRead, BufReader, Write};
    let failed = |why: String| EstateError::Protocol(format!("{method}: {why}"));
    let path = non_empty_env("HERDR_SOCKET_PATH")
        .ok_or_else(|| failed("$HERDR_SOCKET_PATH is unset".to_string()))?;
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| failed(format!("could not reach herdr at {path}: {e}")))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .map_err(|e| failed(e.to_string()))?;
    let request = serde_json::json!({
        "id": format!("herdr-estate:{method}"),
        "method": method,
        "params": params,
    });
    writeln!(stream, "{request}").map_err(|e| failed(e.to_string()))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| failed(e.to_string()))?;
    let reply: serde_json::Value = serde_json::from_str(&line)
        .map_err(|_| failed("herdr's reply was not JSON".to_string()))?;
    if let Some(result) = reply.get("result") {
        return Ok(result.clone());
    }
    let message = reply
        .pointer("/error/message")
        .and_then(|m| m.as_str())
        .unwrap_or("herdr's reply carried no result");
    Err(failed(message.to_string()))
}

pub fn workspace_list() -> Result<Vec<Workspace>, EstateError> {
    let result = call_herdr(&["workspace".to_string(), "list".to_string()], true)?;
    let list: WorkspaceListResult = serde_json::from_value(result.data).map_err(|_| {
        EstateError::Protocol("workspace list: response carried no 'workspaces' array".to_string())
    })?;
    Ok(list.workspaces)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentRow {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_status: Option<AgentStatus>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaneRow {
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

pub fn agent_list() -> Result<Vec<AgentRow>, EstateError> {
    let result = call_herdr(&["agent".to_string(), "list".to_string()], true)?;
    let agents = result.data.get("agents").cloned().ok_or_else(|| {
        EstateError::Protocol("agent list: response carried no 'agents' array".to_string())
    })?;
    serde_json::from_value(agents).map_err(|_| {
        EstateError::Protocol("agent list: 'agents' was not an array of agents".to_string())
    })
}

pub fn pane_list(workspace_id: &str) -> Result<Vec<PaneRow>, EstateError> {
    let result = call_herdr(
        &[
            "pane".to_string(),
            "list".to_string(),
            "--workspace".to_string(),
            workspace_id.to_string(),
        ],
        true,
    )?;
    let panes = result.data.get("panes").cloned().ok_or_else(|| {
        EstateError::Protocol("pane list: response carried no 'panes' array".to_string())
    })?;
    serde_json::from_value(panes).map_err(|_| {
        EstateError::Protocol("pane list: 'panes' was not an array of panes".to_string())
    })
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

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_failed_call_message_masks_every_configured_env_value() {
        let msg = failure_message(
            &args(&[
                "workspace",
                "create",
                "--label",
                "home/garden",
                "--env",
                "HERDR_ESTATE_ADDRESS=home/garden",
                "--env",
                "ANTHROPIC_API_KEY=",
                "--env",
                "SECRET_TOKEN=sk-do-not-log",
            ]),
            "boom",
            "",
        );
        assert_eq!(
            msg,
            "herdr workspace create --label home/garden \
             --env HERDR_ESTATE_ADDRESS=home/garden --env ANTHROPIC_API_KEY= \
             --env SECRET_TOKEN=<from estates.toml> failed: boom"
        );
    }

    #[test]
    fn redaction_touches_only_the_argument_after_an_env_flag() {
        assert_eq!(
            redacted_args(&args(&[
                "--cwd",
                "/a=b",
                "--",
                "--flag=value",
                "--env",
                "K=V"
            ])),
            args(&[
                "--cwd",
                "/a=b",
                "--",
                "--flag=value",
                "--env",
                "K=<from estates.toml>"
            ])
        );
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
