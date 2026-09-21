//! Dispatch of a picker row: what Enter does with the row the picker's
//! core selected. Lives beside `verbs`, not in `picker::shell` — it
//! reuses `verbs::resume_with_ids`/`fresh_with_ids` exactly, the same
//! functions `herdr-estate resume`/`fresh` call, so every guard those
//! verbs enforce (ownership, alive, duplicate labels, unconfirmed close,
//! validation before close) applies unchanged. The picker adds no way
//! around them: a live row only ever focuses an already-open workspace,
//! and a dormant or new row's create-or-refuse decision is made entirely
//! inside the verb it calls.

use crate::errors::EstateError;
use crate::herdrcli;
use crate::picker::core::{Row, RowState};
use crate::verbs;

/// Act on the selected row, then focus whatever workspace ends up
/// carrying its address. Errors (a guard refusal, a herdr failure)
/// propagate without focusing anything.
pub fn open_row(estates: &toml::Table, row: &Row) -> Result<(), EstateError> {
    let workspace_id = match &row.state {
        RowState::Live { workspace_id, .. } => workspace_id.clone(),
        RowState::Dormant { .. } => {
            let result = verbs::resume_with_ids(estates, &row.address, false)?;
            result.workspace_id.ok_or_else(|| {
                EstateError::Protocol(format!(
                    "{}: resume did not report a workspace id",
                    row.address
                ))
            })?
        }
        RowState::New => {
            let result = verbs::fresh_with_ids(estates, &row.address, false)?;
            result.workspace_id.ok_or_else(|| {
                EstateError::Protocol(format!(
                    "{}: fresh did not report a workspace id",
                    row.address
                ))
            })?
        }
    };
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
        let row = Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w9".to_string(),
                agent_status: "working".to_string(),
            },
        };
        open_row(&f.estates, &row).unwrap();
        assert_eq!(f.fake.call_count(), 1);
        assert_eq!(f.fake.argv(0), vec!["workspace", "focus", "w9"]);
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
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 4);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1)[..3], ["workspace", "create", "--cwd"]);
        assert_eq!(f.fake.argv(2)[..2], ["agent", "start"]);
        assert_eq!(f.fake.argv(3), vec!["workspace", "focus", "w-new"]);
    }

    #[test]
    fn new_row_freshens_then_focuses_the_new_workspace() {
        let f = make_fixture();
        f.campaign("attic");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        let row = Row {
            address: "home/attic".to_string(),
            state: RowState::New,
        };
        open_row(&f.estates, &row).unwrap();

        assert_eq!(f.fake.call_count(), 4);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        assert_eq!(f.fake.argv(1)[..2], ["workspace", "create"]);
        assert_eq!(f.fake.argv(2)[..2], ["agent", "start"]);
        assert_eq!(f.fake.argv(3), vec!["workspace", "focus", "w-new"]);
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
        };
        let err = open_row(&f.estates, &row).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1); // only the workspace list probe
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
        };
        let err = open_row(&f.estates, &row).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }
}
