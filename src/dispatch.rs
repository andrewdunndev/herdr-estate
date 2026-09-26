use crate::detach::RowAction;
use crate::doubleopen;
use crate::errors::EstateError;
use crate::herdrcli;
use crate::picker::core::{Row, RowState};
use crate::progress;
use crate::verbs;

pub fn open_row(estates: &toml::Table, row: &Row) -> Result<(), EstateError> {
    let workspace_id = match &row.state {
        RowState::Live { .. } => open_live(estates, &row.address)?,
        RowState::Dormant { .. } => {
            let result = verbs::resume_with_ids(estates, &row.address, false)?;
            result.workspace_id.ok_or_else(|| {
                EstateError::Protocol(format!(
                    "{}: resume did not report a workspace id",
                    row.address
                ))
            })?
        }
        RowState::New => return fresh_row(estates, &row.address),
    };
    focus(&row.address, workspace_id)
}

fn open_live(estates: &toml::Table, address: &str) -> Result<String, EstateError> {
    match doubleopen::live_workspace(address)? {
        Some(ws) if doubleopen::workspace_alive(&ws) => {
            progress::step(&format!(
                "{address}: already running in {}",
                ws.workspace_id
            ));
            Ok(ws.workspace_id)
        }
        _ => {
            let result = verbs::resume_with_ids(estates, address, false)?;
            result.workspace_id.ok_or_else(|| {
                EstateError::Protocol(format!("{address}: resume did not report a workspace id"))
            })
        }
    }
}

pub fn run_action(
    estates: &toml::Table,
    action: RowAction,
    address: &str,
) -> Result<(), EstateError> {
    match action {
        RowAction::Open => {
            let workspace_id = open_live(estates, address)?;
            focus(address, workspace_id)
        }
        RowAction::Fresh => fresh_row(estates, address),
        RowAction::Stop => fresh_row_running(estates, address),
    }
}

pub fn fresh_row(estates: &toml::Table, address: &str) -> Result<(), EstateError> {
    let result = verbs::fresh_with_ids(estates, address, false)?;
    let workspace_id = result.workspace_id.ok_or_else(|| {
        EstateError::Protocol(format!("{address}: fresh did not report a workspace id"))
    })?;
    focus(address, workspace_id)
}

pub fn fresh_row_running(estates: &toml::Table, address: &str) -> Result<(), EstateError> {
    let result = verbs::fresh_with_ids_stop_running(estates, address)?;
    let workspace_id = result.workspace_id.ok_or_else(|| {
        EstateError::Protocol(format!("{address}: fresh did not report a workspace id"))
    })?;
    focus(address, workspace_id)
}

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
    use crate::herdrcli::AgentStatus;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};

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
                agent_status: Some(AgentStatus::Working),
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
    fn live_row_genuinely_alive_never_runs_prepare() {
        let mut f = make_fixture();
        f.campaign("garden");
        if let Some(toml::Value::Table(row)) = f.estates.get_mut("home") {
            row.insert(
                "prepare".to_string(),
                toml::Value::Array(vec![toml::Value::String(
                    "/no/such/prepare-binary".to_string(),
                )]),
            );
        }
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
                agent_status: Some(AgentStatus::Working),
            },
            brief: String::new(),
        };
        open_row(&f.estates, &row).unwrap();
        assert_eq!(f.fake.call_count(), 2);
        assert!(!crate::progress::logged_messages()
            .iter()
            .any(|l| l.contains("prepare")));
    }

    #[test]
    fn dormant_row_resumes_then_focuses_the_new_workspace() {
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        let proj = f.user_home().join(".claude").join("projects").join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();

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

        fresh_row(&f.estates, "home/garden").unwrap();

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
        assert_eq!(f.fake.call_count(), 1);

        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn fresh_row_running_stops_an_idle_agent_then_freshens_and_focuses() {
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        crate::ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "idle", "pane_count": 1}}}"#,
            0,
        );
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        fresh_row_running(&f.estates, "home/garden").unwrap();

        assert_eq!(f.fake.call_count(), 6);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
        assert_eq!(f.fake.argv(3)[..2], ["workspace", "create"]);
        assert_eq!(f.fake.argv(5), vec!["workspace", "focus", "w-new"]);
        let lines = crate::continuity::read(&cdir);
        assert_eq!(lines.len(), 2, "the retired id stays, the new one appends");
        assert!(lines[0].ends_with(" old-sid"), "{lines:?}");
    }

    #[test]
    fn run_action_stop_is_the_whole_stop_close_create_start_and_focus() {
        let f = make_fixture();
        let cdir = f.campaign("garden");
        crate::continuity::append(&cdir, "old-sid").unwrap();
        crate::ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "idle", "pane_count": 1}}}"#,
            0,
        );
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        run_action(&f.estates, RowAction::Stop, "home/garden").unwrap();

        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
        assert_eq!(f.fake.argv(5), vec!["workspace", "focus", "w-new"]);
        assert_eq!(crate::continuity::read(&cdir).len(), 2);
        assert!(!crate::ownership::is_ours("w1"));
        assert!(crate::ownership::is_ours("w-new"));
    }

    #[test]
    fn run_action_open_takes_the_live_rows_path_and_only_focuses_an_alive_one() {
        let f = make_fixture();
        f.campaign("garden");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        run_action(&f.estates, RowAction::Open, "home/garden").unwrap();
        assert_eq!(f.fake.call_count(), 2);
        assert_eq!(f.fake.argv(1), vec!["workspace", "focus", "w2"]);
    }

    #[test]
    fn run_action_fresh_refuses_a_running_agent_the_way_ctrl_f_does() {
        let f = make_fixture();
        f.campaign("garden");
        crate::ownership::record("home/garden", "w2").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        let err = run_action(&f.estates, RowAction::Fresh, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("running agent")));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn fresh_row_running_refuses_a_working_agent_and_never_closes_or_focuses() {
        let f = make_fixture();
        f.campaign("garden");
        crate::ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );

        let err = fresh_row_running(&f.estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn live_row_whose_workspace_went_not_alive_resumes_instead_of_focusing_the_dead_shell() {
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

        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w1".to_string(),
                agent_status: Some(AgentStatus::Working),
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
        let f = make_fixture();
        f.campaign("attic");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, empty_list(), 0);
        f.fake.set_response(2, CREATE_OK, 0);
        f.fake.set_response(3, r#"{"result": {}}"#, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::Live {
                workspace_id: "gone-w1".to_string(),
                agent_status: Some(AgentStatus::Working),
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
                agent_status: Some(AgentStatus::Unknown),
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

    fn quern(f: &Fixture) -> std::path::PathBuf {
        let campaigns = f.root.join("campaigns");
        std::fs::create_dir_all(&campaigns).unwrap();
        campaigns.join("quern")
    }

    #[test]
    fn create_refuses_while_any_workspace_carries_the_label_and_writes_nothing() {
        let f = make_fixture();
        let dir = quern(&f);
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
        let f = make_fixture();
        let dir = f.campaign("garden");
        f.fake.set_response(0, empty_list(), 0);
        let err = create_and_open(&f.estates, "home/garden", "Grind.").unwrap_err();
        assert!(
            matches!(&err, EstateError::Usage(m) if m.ends_with("already a campaign")),
            "{err}"
        );
        assert_eq!(f.fake.call_count(), 1);
        assert!(crate::continuity::read(&dir).is_empty());
        assert!(crate::progress::logged_messages().is_empty());
    }

    #[test]
    fn create_writes_the_campaign_then_opens_it_as_a_new_row() {
        let f = make_fixture();
        let dir = quern(&f);
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, empty_list(), 0);
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
        f.fake.set_response(2, "", 1);

        assert!(create_and_open(&f.estates, "home/quern", "Grind.").is_err());
        assert!(dir.join("campaign.md").is_file());
        assert!(dir.join("log.md").is_file());
        assert_eq!(
            crate::progress::logged_messages().last().unwrap(),
            "home/quern: campaign created, but starting it failed. It stays as a new row"
        );
    }
}
