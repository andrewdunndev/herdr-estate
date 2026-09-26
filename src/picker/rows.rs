use std::collections::HashMap;

use crate::errors::EstateError;
use crate::herdrcli::{self, Workspace};
use crate::picker::core::{Row, RowState};
use crate::verbs;

pub fn build_rows(estates: &toml::Table) -> Result<Vec<Row>, EstateError> {
    let live = index_by_label(herdrcli::workspace_list()?);
    let rows = verbs::list_campaigns(estates)
        .into_iter()
        .map(|campaign_row| {
            let state = match live.get(campaign_row.address.as_str()) {
                Some(ws) => RowState::Live {
                    workspace_id: ws.workspace_id.clone(),
                    agent_status: ws.agent_status.clone(),
                },
                None if !campaign_row.last.is_empty() => RowState::Dormant {
                    last: campaign_row.last,
                },
                None => RowState::New,
            };
            Row {
                address: campaign_row.address,
                state,
                brief: campaign_row.brief,
            }
        })
        .collect();
    Ok(rows)
}

fn index_by_label(workspaces: Vec<Workspace>) -> HashMap<String, Workspace> {
    let mut map = HashMap::new();
    for ws in workspaces {
        map.entry(ws.label.clone()).or_insert(ws);
    }
    map
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
        let env = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);

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
        fn campaign(&self, name: &str) {
            let dir = self.root.join("campaigns").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("campaign.md"), format!("---\n---\nThe {name}.\n")).unwrap();
        }

        fn campaign_with_log(&self, name: &str, sessions_log: &str) {
            self.campaign(name);
            std::fs::write(
                self.root.join("campaigns").join(name).join("sessions.log"),
                sessions_log,
            )
            .unwrap();
        }
    }

    #[test]
    fn new_campaign_with_no_log_and_no_workspace() {
        let f = make_fixture();
        f.campaign("attic");
        f.fake
            .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
        let rows = build_rows(&f.estates).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, "home/attic");
        assert_eq!(rows[0].state, RowState::New);
        assert_eq!(rows[0].brief, "The attic.");
    }

    #[test]
    fn dormant_campaign_has_a_log_but_no_live_workspace() {
        let f = make_fixture();
        f.campaign_with_log("garden", "2026-09-10T00:00:00.000000+00:00 host conv-1\n");
        f.fake
            .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
        let rows = build_rows(&f.estates).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].state,
            RowState::Dormant {
                last: "2026-09-10".to_string()
            }
        );
    }

    #[test]
    fn live_campaign_reports_its_agent_status_and_workspace_id() {
        let f = make_fixture();
        f.campaign("garden");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w9", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        let rows = build_rows(&f.estates).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].state,
            RowState::Live {
                workspace_id: "w9".to_string(),
                agent_status: Some(AgentStatus::Working),
            }
        );
    }

    #[test]
    fn a_missing_agent_status_is_not_the_wire_value_unknown() {
        let f = make_fixture();
        f.campaign("garden");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w9", "label": "home/garden"}
            ]}}"#,
            0,
        );
        let rows = build_rows(&f.estates).unwrap();
        assert_eq!(rows.len(), 1);
        let RowState::Live { agent_status, .. } = &rows[0].state else {
            panic!("expected a Live row, got {:?}", rows[0].state);
        };
        assert_eq!(agent_status, &None);
    }

    #[test]
    fn recorded_and_live_at_once_classifies_as_live() {
        let f = make_fixture();
        f.campaign_with_log("garden", "2026-09-10T00:00:00.000000+00:00 host conv-1\n");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w9", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let rows = build_rows(&f.estates).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].state, RowState::Live { .. }));
    }

    #[test]
    fn a_live_workspace_under_no_known_campaign_is_not_a_row() {
        let f = make_fixture();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w9", "label": "other/place", "agent_status": "working"}
            ]}}"#,
            0,
        );
        let rows = build_rows(&f.estates).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn a_failed_workspace_list_is_an_error_not_an_empty_list() {
        let f = make_fixture();
        f.campaign("garden");
        f.fake.set_response(0, "", 1);
        let err = build_rows(&f.estates).unwrap_err();
        assert!(matches!(err, EstateError::Protocol(_)));
    }
}
