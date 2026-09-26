//! A picker dispatch that outlives the popup it was chosen in.
//!
//! herdr runs the picker popup in the workspace it was opened over, and
//! closing that workspace ends the popup's whole session: SIGHUP, then
//! SIGTERM, then SIGKILL, to every process in it, whichever signals it
//! ignores and whichever process group it sits in. A dispatch that
//! closes the popup's own workspace (Ctrl-f's second press on its row,
//! or Enter or Ctrl-f on it once its agent has exited) died right
//! there, after the close and before the create.
//!
//! So the popup hands that dispatch to herdr: it invokes one of the
//! manifest's `dispatch-*` actions, which herdr runs as its own process,
//! outside every workspace, the way it runs any plugin action. The
//! action runs `pick --dispatch <action>`, and the address rides in the
//! invocation's context as the workspace label it is. The popup waits
//! on that run's plugin log entry, so a refusal made before the close
//! still shows, held, in the popup.

use std::time::Duration;

use crate::errors::EstateError;
use crate::herdrcli;
use crate::paths::non_empty_env;
use crate::picker::core::{Outcome, RowState};
use crate::picker::shell::PLUGIN_ID;
use crate::progress;

/// How often the popup re-reads the handed-off run's log entry.
const POLL: Duration = Duration::from_millis(150);

/// What a picker row asked for: Enter (`open`), Ctrl-f (`fresh`), or
/// Ctrl-f's second press on an idle or done agent (`stop`). Each has its
/// own manifest action, `dispatch-<arg>`, running `pick --dispatch <arg>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Open,
    Fresh,
    Stop,
}

impl RowAction {
    pub fn as_arg(self) -> &'static str {
        match self {
            RowAction::Open => "open",
            RowAction::Fresh => "fresh",
            RowAction::Stop => "stop",
        }
    }

    pub fn from_arg(s: &str) -> Option<Self> {
        match s {
            "open" => Some(RowAction::Open),
            "fresh" => Some(RowAction::Fresh),
            "stop" => Some(RowAction::Stop),
            _ => None,
        }
    }

    /// The manifest action that runs this one outside the popup.
    pub fn action_id(self) -> String {
        format!("dispatch-{}", self.as_arg())
    }
}

/// One string field of the context herdr hands a plugin pane or action,
/// `$HERDR_PLUGIN_CONTEXT_JSON`. `None` in a shell, or when the context
/// is missing, malformed or leaves the field empty.
fn context_field(key: &str) -> Option<String> {
    let json = non_empty_env("HERDR_PLUGIN_CONTEXT_JSON")?;
    let context: serde_json::Value = serde_json::from_str(&json).ok()?;
    context
        .get(key)?
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The workspace the popup was opened over. herdr sets no
/// `$HERDR_WORKSPACE_ID` in a plugin pane; it names the workspace in
/// the pane's context instead.
pub fn popup_workspace_id() -> Option<String> {
    context_field("workspace_id")
}

/// The address a `dispatch-*` action acts on: its invocation context's
/// workspace label, which `hand_off` sets to the row's address.
pub fn context_address() -> Result<String, EstateError> {
    context_field("workspace_label").ok_or_else(|| {
        EstateError::Usage(
            "--dispatch with no address needs a workspace label in $HERDR_PLUGIN_CONTEXT_JSON"
                .to_string(),
        )
    })
}

/// The action and address to hand off when `outcome` acts on the row
/// whose live workspace is `popup_workspace`. Only such a row can be
/// closed under the popup: a dormant or new row has no workspace, and a
/// live one elsewhere closes some other workspace. A live row's id is
/// the one the list build saw, and the popup's own workspace stays open
/// for as long as the popup does, so that id is still the popup's here.
pub fn self_hosted<'a>(
    outcome: &'a Outcome,
    popup_workspace: Option<&str>,
) -> Option<(RowAction, &'a str)> {
    let (action, row) = match outcome {
        Outcome::Selected(row) => (RowAction::Open, row),
        Outcome::Fresh(row) => (RowAction::Fresh, row),
        Outcome::FreshRunning(row) => (RowAction::Stop, row),
        Outcome::Cancelled | Outcome::Create { .. } => return None,
    };
    match &row.state {
        RowState::Live { workspace_id, .. } if Some(workspace_id.as_str()) == popup_workspace => {
            Some((action, row.address.as_str()))
        }
        _ => None,
    }
}

/// Invoke `action`'s manifest action for `address`, whose workspace is
/// `workspace_id`, and wait for herdr to finish running it. The
/// invocation goes over the API socket: `herdr plugin action invoke`
/// fills the context from whichever workspace has focus, and the
/// address must not depend on that. `Ok(true)` when the run succeeded;
/// `Ok(false)` when it failed, its output relayed to stderr. An
/// invocation herdr refuses is an error here, before anything changed.
pub fn hand_off(action: RowAction, address: &str, workspace_id: &str) -> Result<bool, EstateError> {
    let plugin_id = non_empty_env("HERDR_PLUGIN_ID").unwrap_or_else(|| PLUGIN_ID.to_string());
    let action_id = action.action_id();
    progress::step(&format!(
        "{address}: this popup is in the workspace it may close, handing off to herdr action {action_id}"
    ));
    let invoked = herdrcli::call_api(
        "plugin.action.invoke",
        serde_json::json!({
            "plugin_id": plugin_id,
            "action_id": action_id,
            "context": {"workspace_id": workspace_id, "workspace_label": address},
        }),
    )?;
    let log_id = invoked
        .pointer("/log/log_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            EstateError::Protocol(format!("{action_id}: herdr's reply named no log entry"))
        })?;
    wait_for_run(&plugin_id, log_id)
}

/// Poll `plugin log list` until `log_id` is no longer running. A failed
/// run's stderr, its progress lines and its error, is written to this
/// process's stderr so the popup shows why.
fn wait_for_run(plugin_id: &str, log_id: &str) -> Result<bool, EstateError> {
    loop {
        let listed = herdrcli::call_herdr(
            &[
                "plugin".to_string(),
                "log".to_string(),
                "list".to_string(),
                "--plugin".to_string(),
                plugin_id.to_string(),
            ],
            true,
        )?;
        let entry = listed
            .data
            .get("logs")
            .and_then(|l| l.as_array())
            .and_then(|logs| logs.iter().find(|e| e["log_id"] == log_id))
            .cloned()
            .ok_or_else(|| EstateError::Protocol(format!("plugin log list: no entry {log_id}")))?;
        if entry["status"] == "running" {
            std::thread::sleep(POLL);
            continue;
        }
        let ok = entry["status"] == "succeeded" && entry["exit_code"] == 0;
        if !ok {
            use std::io::Write;
            let stderr = entry["stderr"].as_str().unwrap_or("");
            let _ = write!(std::io::stderr(), "{stderr}");
        }
        return Ok(ok);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::core::Row;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};
    use std::io::{BufRead, BufReader, Write};

    fn live(workspace_id: &str) -> Row {
        Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: workspace_id.to_string(),
                agent_status: "idle".to_string(),
            },
            brief: String::new(),
        }
    }

    #[test]
    fn each_row_outcome_on_the_popups_own_workspace_is_handed_off_as_its_action() {
        for (outcome, want) in [
            (Outcome::Selected(live("w2")), RowAction::Open),
            (Outcome::Fresh(live("w2")), RowAction::Fresh),
            (Outcome::FreshRunning(live("w2")), RowAction::Stop),
        ] {
            assert_eq!(
                self_hosted(&outcome, Some("w2")),
                Some((want, "home/garden")),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn a_row_in_another_workspace_stays_in_the_popup() {
        let outcome = Outcome::FreshRunning(live("w9"));
        assert_eq!(self_hosted(&outcome, Some("w2")), None);
    }

    #[test]
    fn a_shell_run_with_no_popup_workspace_stays_in_process() {
        let outcome = Outcome::FreshRunning(live("w2"));
        assert_eq!(self_hosted(&outcome, None), None);
    }

    #[test]
    fn a_dormant_or_new_row_has_no_workspace_to_close_under_the_popup() {
        for state in [
            RowState::Dormant {
                last: "2026-09-10".to_string(),
            },
            RowState::New,
        ] {
            let row = Row {
                address: "home/garden".to_string(),
                state,
                brief: String::new(),
            };
            assert_eq!(self_hosted(&Outcome::Fresh(row), Some("w2")), None);
        }
    }

    #[test]
    fn cancel_and_create_never_hand_off() {
        assert_eq!(self_hosted(&Outcome::Cancelled, Some("w2")), None);
        let create = Outcome::Create {
            address: "home/garden".to_string(),
            brief: "b".to_string(),
        };
        assert_eq!(self_hosted(&create, Some("w2")), None);
    }

    #[test]
    fn every_action_round_trips_through_its_argument() {
        for action in [RowAction::Open, RowAction::Fresh, RowAction::Stop] {
            assert_eq!(RowAction::from_arg(action.as_arg()), Some(action));
        }
        assert_eq!(RowAction::from_arg("close"), None);
    }

    #[test]
    fn the_popup_workspace_comes_from_herdrs_plugin_context() {
        let _g = EnvGuard::set(&[(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"workspace_id":"w2","workspace_label":"home/garden","focused_pane_id":"w2:p1"}"#,
        )]);
        assert_eq!(popup_workspace_id().as_deref(), Some("w2"));
    }

    #[test]
    fn a_missing_malformed_or_empty_context_names_no_workspace() {
        {
            let _g = EnvGuard::unset(&["HERDR_PLUGIN_CONTEXT_JSON"]);
            assert_eq!(popup_workspace_id(), None);
        }
        for json in [
            "not json",
            r#"{"tab_id":"w2:t1"}"#,
            r#"{"workspace_id":""}"#,
        ] {
            let _g = EnvGuard::set(&[("HERDR_PLUGIN_CONTEXT_JSON", json)]);
            assert_eq!(popup_workspace_id(), None, "{json}");
        }
    }

    #[test]
    fn the_address_a_dispatch_action_acts_on_is_its_contexts_workspace_label() {
        {
            let _g = EnvGuard::set(&[(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"workspace_id":"w2","workspace_label":"home/garden"}"#,
            )]);
            assert_eq!(context_address().unwrap(), "home/garden");
        }
        let _g = EnvGuard::unset(&["HERDR_PLUGIN_CONTEXT_JSON"]);
        assert!(matches!(context_address(), Err(EstateError::Usage(_))));
    }

    #[test]
    fn every_action_has_its_manifest_action_running_pick_dispatch() {
        let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"));
        let manifest: toml::Table = toml::from_str(text).unwrap();
        let actions = manifest["actions"].as_array().unwrap();
        for action in [RowAction::Open, RowAction::Fresh, RowAction::Stop] {
            let entry = actions
                .iter()
                .find(|a| a["id"].as_str() == Some(action.action_id().as_str()))
                .unwrap_or_else(|| panic!("no manifest action {}", action.action_id()));
            let command: Vec<&str> = entry["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(
                command,
                vec![
                    "target/release/herdr-estate",
                    "pick",
                    "--dispatch",
                    action.as_arg()
                ]
            );
            assert!(entry["contexts"].as_array().unwrap().is_empty());
        }
    }

    /// A one-shot stand-in for herdr's API socket: accepts one
    /// connection, answers `reply`, and hands back the request line.
    fn fake_api(reply: &'static str) -> (std::path::PathBuf, std::thread::JoinHandle<String>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        // Short: a socket path must fit sockaddr_un's 104 bytes on macOS.
        let path = std::env::temp_dir().join(format!(
            "he-api-{}-{}.sock",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            writeln!(&stream, "{reply}").unwrap();
            line
        });
        (path, served)
    }

    const INVOKED: &str = r#"{"id":"x","result":{"type":"plugin_action_invoked","log":{"log_id":"plugin-log-7","status":"running"}}}"#;

    fn log_list(status: &str, exit_code: &str, stderr: &str) -> String {
        format!(
            r#"{{"result": {{"logs": [
                {{"log_id": "plugin-log-6", "status": "failed", "exit_code": 1}},
                {{"log_id": "plugin-log-7", "status": "{status}", "exit_code": {exit_code}, "stderr": "{stderr}"}}
            ]}}}}"#
        )
    }

    #[test]
    fn a_hand_off_invokes_its_action_with_the_address_pinned_and_waits_for_the_run() {
        let d = TempDir::new();
        let fake = FakeHerdr::new();
        let (socket, served) = fake_api(INVOKED);
        let _g = EnvGuard::new(
            &[
                ("HERDR_SOCKET_PATH", socket.to_str().unwrap()),
                ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
                ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
            ],
            &["HERDR_PLUGIN_ID"],
        );
        fake.set_response(0, &log_list("running", "null", ""), 0);
        fake.set_response(1, &log_list("succeeded", "0", ""), 0);

        assert!(hand_off(RowAction::Stop, "home/garden", "w2").unwrap());

        let request: serde_json::Value = serde_json::from_str(&served.join().unwrap()).unwrap();
        let _ = std::fs::remove_file(&socket);
        assert_eq!(request["method"], "plugin.action.invoke");
        assert_eq!(
            request["params"],
            serde_json::json!({
                "plugin_id": "herdr-estate",
                "action_id": "dispatch-stop",
                "context": {"workspace_id": "w2", "workspace_label": "home/garden"},
            })
        );
        assert_eq!(fake.call_count(), 2);
        assert_eq!(
            fake.argv(1),
            vec!["plugin", "log", "list", "--plugin", "herdr-estate"]
        );
    }

    #[test]
    fn a_failed_run_is_reported_as_false() {
        let d = TempDir::new();
        let fake = FakeHerdr::new();
        let (socket, served) = fake_api(INVOKED);
        let _g = EnvGuard::new(
            &[
                ("HERDR_SOCKET_PATH", socket.to_str().unwrap()),
                ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
                ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
            ],
            &["HERDR_PLUGIN_ID"],
        );
        fake.set_response(
            0,
            &log_list("failed", "1", "herdr-estate: home/garden: refused"),
            0,
        );
        assert!(!hand_off(RowAction::Stop, "home/garden", "w2").unwrap());
        served.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn an_invocation_herdr_refuses_is_an_error_and_waits_on_nothing() {
        let d = TempDir::new();
        let fake = FakeHerdr::new();
        let (socket, served) = fake_api(
            r#"{"id":"x","error":{"code":"plugin_action_not_found","message":"plugin action not found"}}"#,
        );
        let _g = EnvGuard::new(
            &[
                ("HERDR_SOCKET_PATH", socket.to_str().unwrap()),
                ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
                ("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap()),
            ],
            &["HERDR_PLUGIN_ID"],
        );
        let err = hand_off(RowAction::Open, "home/garden", "w2").unwrap_err();
        served.join().unwrap();
        let _ = std::fs::remove_file(&socket);
        assert!(
            matches!(&err, EstateError::Protocol(m) if m.contains("plugin action not found")),
            "{err}"
        );
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn no_socket_is_an_error_before_anything_is_sent() {
        let _g = EnvGuard::unset(&["HERDR_SOCKET_PATH"]);
        let err = herdrcli::call_api("plugin.action.list", serde_json::json!({})).unwrap_err();
        assert!(matches!(err, EstateError::Protocol(m) if m.contains("HERDR_SOCKET_PATH")));
    }
}
