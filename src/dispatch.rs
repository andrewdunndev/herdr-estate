//! Dispatch of a picker row: what Enter (or Ctrl-f) does with the row the
//! picker's core selected, or with a campaign it asked to create. Lives
//! beside `verbs`, not in `picker::shell` — it reuses
//! `verbs::resume_with_ids`/`fresh_with_ids` exactly, the same
//! functions `herdr-estate resume`/`fresh` call, so every guard those
//! verbs enforce (ownership, alive, duplicate labels, unconfirmed close,
//! validation before close) applies unchanged. The picker adds no way
//! around them: a Live row's workspace is re-read at dispatch time
//! (`doubleopen::live_workspace`) and only focused directly when it is
//! still genuinely alive; a row that turned out dormant, gone or
//! duplicated by the time Enter fires is handed to `resume_with_ids`
//! just like a `Dormant` row, so the create-or-refuse decision is always
//! made inside the verb, never on the stale row the list build saw.

use crate::doubleopen;
use crate::errors::EstateError;
use crate::herdrcli;
use crate::picker::core::{Row, RowState};
use crate::progress;
use crate::verbs;

/// Act on the selected row, then focus whatever workspace ends up
/// carrying its address. Errors (a guard refusal, a herdr failure)
/// propagate without focusing anything.
pub fn open_row(estates: &toml::Table, row: &Row) -> Result<(), EstateError> {
    let workspace_id = match &row.state {
        // A Live row only names an address that carried a workspace when
        // the list was built; the picker can sit open for a while, so
        // that workspace's liveness (or existence) is re-read here
        // rather than trusted from the stale row. Only a workspace that
        // is genuinely alive right now is focus-only — anything else
        // (the agent exited, the workspace closed, a duplicate label)
        // goes through `resume_with_ids`, which owns the alive check,
        // the close-and-recreate decision and the duplicate-label
        // refusal, exactly as it does for a `Dormant` row.
        RowState::Live { .. } => match doubleopen::live_workspace(&row.address)? {
            Some(ws) if doubleopen::workspace_alive(&ws) => {
                progress::step(&format!(
                    "{}: already running in {}",
                    row.address, ws.workspace_id
                ));
                ws.workspace_id
            }
            _ => {
                let result = verbs::resume_with_ids(estates, &row.address, false)?;
                result.workspace_id.ok_or_else(|| {
                    EstateError::Protocol(format!(
                        "{}: resume did not report a workspace id",
                        row.address
                    ))
                })?
            }
        },
        RowState::Dormant { .. } => {
            let result = verbs::resume_with_ids(estates, &row.address, false)?;
            result.workspace_id.ok_or_else(|| {
                EstateError::Protocol(format!(
                    "{}: resume did not report a workspace id",
                    row.address
                ))
            })?
        }
        RowState::New => return fresh_row(estates, row),
    };
    focus(&row.address, workspace_id)
}

/// Ctrl-f on a row, and Enter on a New one: start a fresh conversation
/// through `verbs::fresh_with_ids`, whatever the row's state, then focus
/// the workspace it opens. The verb owns every refusal, a running agent
/// included, and the retired conversation id stays in `sessions.log`.
pub fn fresh_row(estates: &toml::Table, row: &Row) -> Result<(), EstateError> {
    let result = verbs::fresh_with_ids(estates, &row.address, false)?;
    let workspace_id = result.workspace_id.ok_or_else(|| {
        EstateError::Protocol(format!(
            "{}: fresh did not report a workspace id",
            row.address
        ))
    })?;
    focus(&row.address, workspace_id)
}

/// Enter on the picker's `+ create` line, with a brief typed: create the
/// campaign (`verbs::create_campaign`), then open it as the New row it
/// now is, through `open_row`. It reads `workspace list` first, and
/// while any workspace carries the address (even one herdr-estate did
/// not make, which the picker never shows) it writes and logs nothing.
/// If the open then fails, nothing is deleted: the campaign stays, a new
/// row, and a step says so.
pub fn create_and_open(
    estates: &toml::Table,
    address: &str,
    brief: &str,
) -> Result<(), EstateError> {
    if let Some(ws) = doubleopen::live_workspace(address)? {
        return Err(EstateError::Guard(format!(
            "{address}: workspace {} already carries this label, refusing to create a campaign under it",
            ws.workspace_id
        )));
    }
    verbs::create_campaign(estates, address, brief, false)?;
    let row = Row {
        address: address.to_string(),
        state: RowState::New,
        brief: String::new(),
    };
    open_row(estates, &row).inspect_err(|_| {
        progress::step(&format!(
            "{address}: campaign created, but starting it failed. It stays as a new row"
        ));
    })
}

fn focus(address: &str, workspace_id: String) -> Result<(), EstateError> {
    progress::step(&format!("{address}: focusing {workspace_id}"));
    herdrcli::call_herdr(
        &["workspace".to_string(), "focus".to_string(), workspace_id],
        true,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};

    /// Same shape as `verbs`'s own `make_fixture`: one estate, `home`,
    /// rooted under a fresh temp dir, `$HOME` and the ownership state
    /// dir both pointed at throwaway dirs, `$HERDR_BIN_PATH` at the
    /// fake. `campaign` creates `campaigns/<name>/campaign.md` on
    /// demand — a dispatch test only ever touches one.
    struct Fixture {
        _tmp: TempDir,
        _env: EnvGuard,
        root: std::path::PathBuf,
        estates: toml::Table,
        fake: FakeHerdr,
    }

    fn make_fixture() -> Fixture {
        let tmp = TempDir::new();
        let root = tmp.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        let fake = FakeHerdr::new();
        let state_dir = tmp.path().join("state");
        let user_home = tmp.path().join("userhome");
        std::fs::create_dir_all(&user_home).unwrap();
        let env = EnvGuard::set(&[
            ("HERDR_PLUGIN_STATE_DIR", state_dir.to_str().unwrap()),
            ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
            ("HOME", user_home.to_str().unwrap()),
        ]);

        let mut row = toml::Table::new();
        row.insert(
            "root".to_string(),
            toml::Value::String(root.to_string_lossy().to_string()),
        );
        let mut estates = toml::Table::new();
        estates.insert("home".to_string(), toml::Value::Table(row));

        Fixture {
            _tmp: tmp,
            _env: env,
            root,
            estates,
            fake,
        }
    }

    impl Fixture {
        fn campaign(&self, name: &str) -> std::path::PathBuf {
            let dir = self.root.join("campaigns").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("campaign.md"), "---\n---\n").unwrap();
            dir
        }

        fn user_home(&self) -> std::path::PathBuf {
            self._tmp.path().join("userhome")
        }
    }

    const CREATE_OK: &str = r#"{"result": {
        "workspace": {"workspace_id": "w-new"},
        "root_pane": {"pane_id": "p1"}
    }}"#;

    fn empty_list() -> &'static str {
        r#"{"result": {"workspaces": []}}"#
    }

    #[test]
    fn live_row_only_focuses_the_known_workspace_id() {
        let f = make_fixture();
        // The dispatch-time re-read agrees with the row: still alive,
        // same workspace id.
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w9", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w9".to_string(),
                agent_status: "working".to_string(),
            },
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();
        assert_eq!(f.fake.call_count(), 2);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1), vec!["workspace", "focus", "w9"]);
        assert_eq!(
            crate::progress::logged_messages(),
            vec![
                "home/garden: already running in w9",
                "home/garden: focusing w9",
            ]
        );
    }

    #[test]
    fn dormant_row_resumes_then_focuses_the_new_workspace() {
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        let proj = f.user_home().join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();

        // resume_with_ids's own sequence: workspace list (none live),
        // create, agent start — then dispatch's own focus call.
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Dormant {
                last: "2026-09-10".to_string(),
            },
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 4);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1)[..3], ["workspace", "create", "--cwd"]);
        assert_eq!(f.fake.argv(2)[..2], ["agent", "start"]);
        // The verb this branch must call, pinned to its distinguishing
        // flag: `fresh_with_ids` in its place would produce the same
        // argv shape (`agent start ...`) but never `--resume`, and would
        // append a new conversation id to the log instead of leaving it
        // alone.
        let start_argv = f.fake.argv(2);
        assert!(start_argv.iter().any(|a| a == "--resume"), "{start_argv:?}");
        assert!(start_argv.iter().any(|a| a == "old-sid"), "{start_argv:?}");
        assert_eq!(f.fake.argv(3), vec!["workspace", "focus", "w-new"]);
        let lines = crate::continuity::read(&cdir);
        assert_eq!(
            lines.last().unwrap().split_whitespace().last(),
            Some("old-sid"),
            "resume must not append a new conversation id"
        );
    }

    #[test]
    fn new_row_freshens_then_focuses_the_new_workspace() {
        let f = make_fixture();
        let cdir = f.campaign("attic");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::New,
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 4);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1)[..2], ["workspace", "create"]);
        assert_eq!(f.fake.argv(2)[..2], ["agent", "start"]);
        // Mirrors the dormant case: a New row's verb must be
        // `fresh_with_ids`, never `--resume` (there is nothing recorded
        // to resume), and it must append the new conversation id.
        let start_argv = f.fake.argv(2);
        assert!(
            !start_argv.iter().any(|a| a == "--resume"),
            "{start_argv:?}"
        );
        assert_eq!(f.fake.argv(3), vec!["workspace", "focus", "w-new"]);
        let lines = crate::continuity::read(&cdir);
        assert_eq!(lines.len(), 1, "fresh must append exactly one new line");
    }

    #[test]
    fn fresh_on_a_dormant_row_starts_a_new_conversation_and_keeps_the_old_id() {
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Dormant {
                last: "2026-09-10".to_string(),
            },
            brief: String::new(),
        };
        fresh_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 4);
        assert_eq!(f.fake.argv(1)[..2], ["workspace", "create"]);
        let start_argv = f.fake.argv(2);
        assert!(
            !start_argv.iter().any(|a| a == "--resume" || a == "old-sid"),
            "{start_argv:?}"
        );
        assert_eq!(f.fake.argv(3), vec!["workspace", "focus", "w-new"]);
        let lines = crate::continuity::read(&cdir);
        assert_eq!(lines.len(), 2, "fresh appends one new id after the old");
        assert!(lines[0].ends_with(" old-sid"), "{lines:?}");
    }

    #[test]
    fn a_guard_refusal_creates_nothing_starts_nothing_and_never_focuses() {
        let f = make_fixture();
        f.campaign("attic");
        // Already alive: `agent_status` is anything but "unknown".
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/attic", "agent_status": "working"}
            ]}}"#,
            0,
        );

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::New,
            brief: String::new(),
        };
        let err = open_row(&f.estates, &row).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1); // only the workspace list probe

        // Refused before any step: nothing claims a start or a focus.
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn live_row_whose_workspace_went_not_alive_resumes_instead_of_focusing_the_dead_shell() {
        // The row was built while an agent was still attached (Live,
        // agent_status "working"), but by dispatch time the agent has
        // exited: a fresh `workspace list` shows the same workspace id
        // with agent_status "unknown" (not alive). Enter must not focus
        // that dead shell — it must resume the recorded conversation,
        // which owns closing and recreating the workspace.
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        let proj = f.user_home().join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();
        crate::ownership::record("home/garden", "w1").unwrap();

        let not_alive_list = r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
        ]}}"#;
        // 0: open_row's own re-read. 1: check_live_workspace inside
        // resume_with_ids. 2: close_if_ours's pre-close label get. 3: the
        // close itself. 4: create. 5: agent start. 6: dispatch's focus.
        f.fake.set_response(0, not_alive_list, 0);
        f.fake.set_response(1, not_alive_list, 0);
        f.fake.set_response(
            2,
            r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
            0,
        );
        f.fake.set_response(3, r#"{"result": {}}"#, 0);
        f.fake.set_response(4, CREATE_OK, 0);
        f.fake.set_response(5, r#"{"result": {}}"#, 0);
        f.fake.set_response(6, r#"{"result": {}}"#, 0);

        // The row as the picker drew it: still Live, still carrying the
        // stale workspace id from the listing that built the row.
        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "working".to_string(),
            },
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 7);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(2), vec!["workspace", "get", "w1"]);
        assert_eq!(f.fake.argv(3), vec!["workspace", "close", "w1"]);
        assert_eq!(f.fake.argv(4)[..3], ["workspace", "create", "--cwd"]);
        let start_argv = f.fake.argv(5);
        assert!(start_argv.iter().any(|a| a == "--resume"));
        assert!(start_argv.iter().any(|a| a == "old-sid"));
        assert_eq!(f.fake.argv(6), vec!["workspace", "focus", "w-new"]);

        let lines = crate::continuity::read(&cdir);
        assert_eq!(
            lines.last().unwrap().split_whitespace().last(),
            Some("old-sid")
        );
    }

    #[test]
    fn live_row_whose_workspace_closed_meanwhile_falls_through_to_fresh() {
        // Symmetric case: the workspace the row named is gone entirely by
        // dispatch time. With no recorded conversation either, this must
        // route through resume (which itself falls back to fresh) rather
        // than fail with a focus error on a workspace id that no longer
        // exists.
        let f = make_fixture();
        f.campaign("attic");
        f.fake.set_response(0, empty_list(), 0); // open_row's re-read
        f.fake.set_response(1, empty_list(), 0); // check_live_workspace
        f.fake.set_response(2, CREATE_OK, 0);
        f.fake.set_response(3, r#"{"result": {}}"#, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::Live {
                workspace_id: "gone-w1".to_string(),
                agent_status: "working".to_string(),
            },
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 5);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(2)[..2], ["workspace", "create"]);
        assert_eq!(f.fake.argv(3)[..2], ["agent", "start"]);
        assert_eq!(f.fake.argv(4), vec!["workspace", "focus", "w-new"]);
    }

    #[test]
    fn duplicate_labels_on_a_live_row_refuse_before_focusing_either() {
        // The row was built as an ordinary Live row from whichever
        // workspace `workspace list` returned first; the dispatch-time
        // re-read must still see both and refuse rather than focus one
        // of the two arbitrarily.
        let f = make_fixture();
        f.campaign("garden");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );

        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: "unknown".to_string(),
            },
            brief: String::new(),
        };
        let err = open_row(&f.estates, &row).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn duplicate_labels_refuse_and_never_focus() {
        let f = make_fixture();
        f.campaign("attic");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/attic", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/attic", "agent_status": "unknown"}
            ]}}"#,
            0,
        );

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::New,
            brief: String::new(),
        };
        let err = open_row(&f.estates, &row).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }

    // --- create_and_open ---

    /// `home/quern`'s directory, under a campaigns directory that exists.
    fn quern(f: &Fixture) -> std::path::PathBuf {
        let campaigns = f.root.join("campaigns");
        std::fs::create_dir_all(&campaigns).unwrap();
        campaigns.join("quern")
    }

    #[test]
    fn create_refuses_while_any_workspace_carries_the_label_and_writes_nothing() {
        let f = make_fixture();
        let dir = quern(&f);
        // Not alive and not ours: a workspace build_rows never shows.
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/quern", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let err = create_and_open(&f.estates, "home/quern", "Grind.").unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
        assert!(!dir.exists());
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn a_create_the_verb_refuses_never_starts_the_campaign() {
        // The label is free, but the campaign appeared after the picker
        // built its rows: the verb refuses, and nothing opens it.
        let f = make_fixture();
        let dir = f.campaign("garden");
        f.fake.set_response(0, empty_list(), 0);
        let err = create_and_open(&f.estates, "home/garden", "Grind.").unwrap_err();
        assert!(
            matches!(&err, EstateError::Usage(m) if m.ends_with("already a campaign")),
            "{err}"
        );
        assert_eq!(f.fake.call_count(), 1); // only the label check
        assert!(crate::continuity::read(&dir).is_empty());
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn create_writes_the_campaign_then_opens_it_as_a_new_row() {
        let f = make_fixture();
        let dir = quern(&f);
        f.fake.set_response(0, empty_list(), 0); // the label check
        f.fake.set_response(1, empty_list(), 0); // fresh's own check
        f.fake.set_response(2, CREATE_OK, 0);
        f.fake.set_response(3, r#"{"result": {}}"#, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        create_and_open(&f.estates, "home/quern", "Grind.").unwrap();

        assert_eq!(f.fake.call_count(), 5);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(2)[..2], ["workspace", "create"]);
        assert_eq!(f.fake.argv(3)[..2], ["agent", "start"]);
        assert_eq!(f.fake.argv(4), vec!["workspace", "focus", "w-new"]);
        assert!(dir.join("campaign.md").is_file());
        assert_eq!(crate::continuity::read(&dir).len(), 1);
        assert!(crate::progress::logged_messages()[0]
            .starts_with("home/quern: creating campaign.md and log.md in "));
    }

    #[test]
    fn a_failed_start_deletes_nothing_and_says_the_campaign_stays_a_new_row() {
        let f = make_fixture();
        let dir = quern(&f);
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, empty_list(), 0);
        f.fake.set_response(2, "", 1); // workspace create fails

        assert!(create_and_open(&f.estates, "home/quern", "Grind.").is_err());
        assert!(dir.join("campaign.md").is_file());
        assert!(dir.join("log.md").is_file());
        assert_eq!(
            crate::progress::logged_messages().last().unwrap(),
            "home/quern: campaign created, but starting it failed. It stays as a new row"
        );
    }
}
