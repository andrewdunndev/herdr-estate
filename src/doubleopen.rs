//! Two independent double-open guards, plus the live-workspace lookup
//! `ownership` and the alive check build on.
//!
//! `registry_holder`/`transcript_exists` read Claude Code's own
//! app-local state (`~/.claude/sessions`, `~/.claude/projects`) — no
//! herdr, no shell but the one process-liveness probe below. They
//! answer: is *this conversation* already open in some other live pane,
//! and does it even have a transcript yet. Both are claude-specific;
//! `verbs` only calls them for a campaign whose agent kind is `claude`.
//!
//! `live_workspace` answers a different question over the herdr CLI:
//! does a live workspace already carry *this address*. `workspace list`
//! returns `{"result": {"workspaces": [{"workspace_id", "label",
//! "agent_status", ...}]}}`; a non-zero exit (`call_herdr`'s default
//! `check: true`) or a 0 exit whose JSON didn't shape into a
//! `workspaces` array both raise `ProtocolError` — an error that stops
//! the verb, never a silent "no workspace, carry on". Every call also
//! prunes `ownership`'s record to the workspace ids this listing still
//! reports, since this is the one place the full listing is already in
//! hand. Every row carrying the address is collected, not just the
//! first: herdr's ids are reused across a server restart, so a second
//! workspace under the same label is a real possibility, and picking one
//! row over the other would hide it from both the alive guard and the
//! ownership guard. More than one match raises `GuardError` naming every
//! id found, before either guard runs on any of them.
//!
//! `workspace_alive` reads `agent_status` off the row `live_workspace`
//! already fetched — no separate `agent list` call. "working", "idle"
//! and "done" mean an agent is attached to the workspace; "unknown"
//! means a bare shell with none. It fails closed: a missing key, a
//! null, or any value other than exactly "unknown" counts as alive.

use std::collections::BTreeSet;
use std::path::Path;

use crate::errors::EstateError;
use crate::herdrcli::{self, AgentStatus, Workspace};
use crate::ownership;
use crate::paths::home_dir;

/// Whether `pid` names a live process this user can signal — `kill -0`,
/// via the `kill` binary rather than a `libc`-crate `kill(2)` wrapper
/// (the same dependency-free tradeoff `continuity::hostname` makes for
/// `gethostname(2)`). A "not found" and a "found but owned by someone
/// else" exit the same way here: either is "not a holder".
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Where `conversation_id` is currently open, if anywhere. A registry
/// entry whose pid is dead is crash residue, not a live holder.
pub fn registry_holder(conversation_id: &str) -> Option<String> {
    let sessions_dir = home_dir().join(".claude").join("sessions");
    let entries = std::fs::read_dir(&sessions_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("sessionId").and_then(|v| v.as_str()) != Some(conversation_id) {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let pid: u32 = match stem.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !pid_alive(pid) {
            continue;
        }
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unnamed");
        return Some(format!("{name} (pid {stem})"));
    }
    None
}

/// A conversation persists only once it has at least one turn.
pub fn transcript_exists(conversation_id: &str) -> bool {
    let projects_dir: &Path = &home_dir().join(".claude").join("projects");
    let file_name = format!("{conversation_id}.jsonl");
    match std::fs::read_dir(projects_dir) {
        Ok(entries) => entries
            .flatten()
            .any(|e| e.path().join(&file_name).is_file()),
        Err(_) => false,
    }
}

/// The workspace herdr already has open under `address`, if any. Errs
/// when more than one row carries it: nothing here is safe to pick over
/// the other, so this stops the verb rather than silently acting on
/// whichever row happened to come first.
pub fn live_workspace(address: &str) -> Result<Option<Workspace>, EstateError> {
    let workspaces = herdrcli::workspace_list()?;
    let existing_ids: BTreeSet<String> =
        workspaces.iter().map(|w| w.workspace_id.clone()).collect();
    ownership::prune(&existing_ids)
        .map_err(|e| EstateError::Config(format!("could not update the ownership record: {e}")))?;
    let mut matching: Vec<Workspace> = workspaces
        .into_iter()
        .filter(|w| w.label == address)
        .collect();
    if matching.len() > 1 {
        let mut ids: Vec<&str> = matching.iter().map(|w| w.workspace_id.as_str()).collect();
        ids.sort_unstable();
        return Err(EstateError::Guard(format!(
            "{address}: {} workspaces already carry this label ({}) — close the extras yourself",
            matching.len(),
            ids.join(", ")
        )));
    }
    Ok(matching.pop())
}

/// Fail closed: anything other than exactly "unknown" counts as alive,
/// including a missing key, a null, or an unrecognised value.
pub fn workspace_alive(ws: &Workspace) -> bool {
    match &ws.agent_status {
        None => true,
        Some(status) => *status != AgentStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};

    fn write_session(sessions_dir: &Path, pid: u32, session_id: &str, name: &str) {
        std::fs::create_dir_all(sessions_dir).unwrap();
        let body = serde_json::json!({"sessionId": session_id, "name": name});
        std::fs::write(
            sessions_dir.join(format!("{pid}.json")),
            serde_json::to_string(&body).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn live_pid_is_a_holder() {
        let home = TempDir::new();
        let sessions = home.path().join(".claude").join("sessions");
        write_session(&sessions, std::process::id(), "sid-1", "andrew");
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        let holder = registry_holder("sid-1");
        assert!(holder.is_some());
        assert!(holder.unwrap().contains(&std::process::id().to_string()));
    }

    #[test]
    fn empty_session_name_prints_as_unnamed() {
        let home = TempDir::new();
        let sessions = home.path().join(".claude").join("sessions");
        write_session(&sessions, std::process::id(), "sid-1", "");
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        let holder = registry_holder("sid-1").unwrap();
        assert!(holder.starts_with("unnamed "), "{holder}");
    }

    #[test]
    fn dead_pid_is_not_a_holder() {
        let home = TempDir::new();
        let sessions = home.path().join(".claude").join("sessions");
        // A pid this unlikely to be a live process on the test machine.
        write_session(&sessions, 999_999, "sid-1", "andrew");
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        assert!(registry_holder("sid-1").is_none());
    }

    #[test]
    fn no_match_is_none() {
        let home = TempDir::new();
        let sessions = home.path().join(".claude").join("sessions");
        write_session(&sessions, std::process::id(), "sid-other", "andrew");
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        assert!(registry_holder("sid-1").is_none());
    }

    #[test]
    fn missing_sessions_dir_is_none() {
        let home = TempDir::new();
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        assert!(registry_holder("sid-1").is_none());
    }

    #[test]
    fn transcript_found() {
        let home = TempDir::new();
        let proj = home
            .path()
            .join(".claude")
            .join("projects")
            .join("some-project");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("sid-1.jsonl"), "{}\n").unwrap();
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        assert!(transcript_exists("sid-1"));
    }

    #[test]
    fn transcript_missing() {
        let home = TempDir::new();
        let _g = EnvGuard::set(&[("HOME", home.path().to_str().unwrap())]);
        assert!(!transcript_exists("sid-1"));
    }

    /// A single combined guard for both env vars these tests need. Two
    /// live `EnvGuard`s on one thread would each try to lock the same
    /// non-reentrant mutex (see `EnvGuard`'s own doc comment) and
    /// deadlock the whole suite, so every test below sets
    /// `HERDR_PLUGIN_STATE_DIR` and `HERDR_BIN_PATH` in one call instead
    /// of two.
    fn isolated_state(bin_path: &str) -> (TempDir, EnvGuard) {
        let d = TempDir::new();
        let g = EnvGuard::set(&[
            ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
            ("HERDR_BIN_PATH", bin_path),
        ]);
        (d, g)
    }

    #[test]
    fn finds_matching_label_via_workspace_list() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/taxes", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        let ws = live_workspace("home/garden").unwrap();
        assert_eq!(ws.unwrap().workspace_id, "w2");
        assert_eq!(fake.call_count(), 1);
        assert_eq!(fake.argv(0), vec!["workspace", "list"]);
    }

    #[test]
    fn no_match_is_none_for_live_workspace() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/taxes", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        assert!(live_workspace("home/garden").unwrap().is_none());
        assert_eq!(fake.call_count(), 1);
    }

    #[test]
    fn missing_workspaces_array_raises() {
        // A 0 exit whose stdout didn't parse into an object leaves
        // HerdrResult.data == {} — that must stop the verb, not read
        // as zero workspaces.
        let fake = FakeHerdr::new();
        fake.set_response(0, "not json", 0);
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        let err = live_workspace("home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Protocol(_)));
    }

    #[test]
    fn a_row_missing_label_raises_rather_than_dropping_silently() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        let err = live_workspace("home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Protocol(_)));
    }

    #[test]
    fn a_row_missing_workspace_id_raises_rather_than_dropping_silently() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        let err = live_workspace("home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Protocol(_)));
    }

    #[test]
    fn duplicate_label_rows_raise_a_guard_error_naming_both_ids() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        let err = live_workspace("home/garden").unwrap_err();
        match err {
            EstateError::Guard(msg) => {
                assert!(msg.contains("w1"), "{msg}");
                assert!(msg.contains("w2"), "{msg}");
            }
            other => panic!("expected a guard error, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_label_rows_raise_even_when_one_is_alive() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        assert!(matches!(
            live_workspace("home/garden"),
            Err(EstateError::Guard(_))
        ));
    }

    #[test]
    fn prunes_ownership_records_not_in_the_listing() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let (_state, _g) = isolated_state(fake.bin_path().to_str().unwrap());
        ownership::record("home/gone", "stale-id").unwrap();
        ownership::record("home/garden", "w2").unwrap();
        live_workspace("home/garden").unwrap();
        assert!(!ownership::is_ours("stale-id"));
        assert!(ownership::is_ours("w2"));
    }

    fn row(agent_status: Option<AgentStatus>) -> Workspace {
        Workspace {
            workspace_id: "w1".to_string(),
            label: "home/garden".to_string(),
            agent_status,
        }
    }

    #[test]
    fn unknown_is_not_alive() {
        assert!(!workspace_alive(&row(Some(AgentStatus::Unknown))));
    }

    #[test]
    fn working_is_alive() {
        assert!(workspace_alive(&row(Some(AgentStatus::Working))));
    }

    #[test]
    fn idle_is_alive() {
        assert!(workspace_alive(&row(Some(AgentStatus::Idle))));
    }

    #[test]
    fn done_is_alive() {
        assert!(workspace_alive(&row(Some(AgentStatus::Done))));
    }

    #[test]
    fn missing_key_is_alive() {
        assert!(workspace_alive(&row(None)));
    }

    #[test]
    fn unrecognised_value_is_alive() {
        assert!(workspace_alive(&row(Some(AgentStatus::Other(
            "something-new".to_string()
        )))));
    }

    #[test]
    fn unrecognised_wire_value_deserializes_as_alive() {
        // Drives the guard from JSON, not a hand-built enum: this is the
        // only test that exercises AgentStatus's custom Deserialize on a
        // string it has never seen, the code that decides whether an
        // unrecognised herdr status becomes Other (alive) or Unknown
        // (closeable).
        let ws: Workspace = serde_json::from_value(serde_json::json!({
            "workspace_id": "w1", "label": "home/garden", "agent_status": "busy"
        }))
        .unwrap();
        assert!(workspace_alive(&ws));
    }

    #[test]
    fn json_null_agent_status_deserializes_as_alive() {
        // Distinct from a missing key: herdr sent the key with an
        // explicit `null` value, which `Option`'s own Deserialize
        // collapses to the same `None` a missing key produces.
        let ws: Workspace = serde_json::from_value(serde_json::json!({
            "workspace_id": "w1", "label": "home/garden", "agent_status": null
        }))
        .unwrap();
        assert!(workspace_alive(&ws));
    }
}
