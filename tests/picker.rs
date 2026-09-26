//! Integration tests for `herdr-estate pick`: the `--render-once` golden
//! frame, the "no half-true list" error path, and the `pick` action
//! opening the popup rather than drawing. The interactive TUI
//! itself needs a real terminal, so it is tried by hand in a herdr popup;
//! this file covers everything reachable without a TTY.
//!
//! Same fixture shape as `tests/cli.rs`: each test spawns the compiled
//! binary with its own `env_clear()`'d environment, `HERDR_BIN_PATH`
//! pointed at a fake `herdr` that records exact argv and replays canned
//! JSON.

use std::path::Path;
use std::process::Command;

use herdr_estate::picker::core::{self, AppState, Indicators, Row, RowState};

mod common;
use common::*;

fn append_session(cdir: &Path, when: &str, conversation_id: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cdir.join("sessions.log"))
        .unwrap();
    writeln!(f, "{when} host {conversation_id}").unwrap();
}

#[test]
fn render_once_matches_the_pure_core_for_the_same_rows() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n"); // will be live
    let kitchen = make_campaign(&root, "kitchen", "---\n---\n# kitchen\nStock it.\n"); // dormant
    append_session(&kitchen, "2026-09-10T00:00:00.000000+00:00", "conv-1");
    make_campaign(&root, "attic", "---\n---\n"); // new

    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w9", "label": "home/garden", "agent_status": "working"}
        ]}}"#,
        0,
    );

    let out = run(
        &f,
        &["pick", "--render-once", "--width", "40", "--height", "8"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));

    // The golden frame, computed from the same pure core the binary
    // itself calls, over the rows this fixture seeded on disk — an
    // independent hand-transcription of the padding/truncation math
    // would be one more place for an off-by-one to hide. `env_clear`
    // and the fixture's own HOME mean there is no herdr config, so the
    // binary draws herdr's default glyphs, dots.
    let expected_rows = vec![
        Row {
            address: "home/garden".to_string(),
            state: RowState::Live {
                workspace_id: "w9".to_string(),
                agent_status: "working".to_string(),
            },
            brief: String::new(),
        },
        Row {
            address: "home/kitchen".to_string(),
            state: RowState::Dormant {
                last: "2026-09-10".to_string(),
            },
            brief: "Stock it.".to_string(),
        },
        Row {
            address: "home/attic".to_string(),
            state: RowState::New,
            brief: String::new(),
        },
    ];
    let expected_lines: Vec<String> =
        core::view(&AppState::new(expected_rows), 40, 8, Indicators::Dots)
            .lines
            .iter()
            .map(core::ViewLine::plain)
            .collect();
    let expected = expected_lines.join("\n") + "\n";

    assert_eq!(stdout(&out), expected);
    assert!(expected.lines().count() <= 8);
    assert!(expected.lines().all(|l| l.chars().count() <= 40));
    // The live row carries its dot and the dormant row its brief, cut
    // to the 5 cells 40 columns leave: the glyphs and the brief came
    // through.
    assert!(expected.contains("> ● home/garden"), "{expected}");
    assert!(expected.contains("home/kitchen  Stoc… last"), "{expected}");
}

#[test]
fn render_once_draws_the_status_glyphs_herdrs_config_names() {
    // The operator's setting, in the file herdr itself reads: with no
    // HERDR_CONFIG_PATH or XDG_CONFIG_HOME (`env_clear`), that is
    // $HOME/.config/herdr/config.toml. The same live row that draws a
    // dot with no config draws herdr's symbol for working here.
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    let herdr_config = f.home.join(".config").join("herdr");
    std::fs::create_dir_all(&herdr_config).unwrap();
    std::fs::write(
        herdr_config.join("config.toml"),
        "[ui]\nstatus_indicators = \"symbols\"\n",
    )
    .unwrap();
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w9", "label": "home/garden", "agent_status": "working"}
        ]}}"#,
        0,
    );
    let out = run(
        &f,
        &["pick", "--render-once", "--width", "40", "--height", "8"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("> ◐ home/garden"), "{text}");
    assert!(!text.contains('●'), "{text}");
}

#[test]
fn render_once_with_no_campaigns_shows_the_filter_line_and_no_match() {
    let (f, _root) = fixture();
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    let out = run(&f, &["pick", "--render-once"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("/ filter campaigns"), "{text}");
    assert!(text.contains(" no campaigns"), "{text}");
}

#[test]
fn render_once_defaults_to_an_80_by_20_frame() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    let out = run(&f, &["pick", "--render-once"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.lines().count() <= 20);
    assert!(text.lines().all(|l| l.chars().count() <= 80));
}

#[test]
fn a_failed_workspace_list_refuses_and_never_opens_a_half_true_picker() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake.set_response(0, "", 1); // herdr itself fails
    let out = run(&f, &["pick", "--render-once"], &[]);
    assert!(!out.status.success());
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).contains("herdr-estate:"));
}

#[test]
fn inside_herdr_the_popup_is_painted_herdrs_panel_before_the_list_is_read() {
    // A failed `workspace list` ends the run before the picker opens, so
    // stdout holds only the paint: vesper's panel, from the file herdr
    // reads, set and cleared to, then SGR 0 on the way out. A run with
    // no HERDR_PLUGIN_ID, from a shell, leaves its terminal alone.
    let painted = "\x1b[48;2;26;26;26m\x1b[38;2;255;255;255m\x1b[2J\x1b[1;1H\x1b[0m";
    for (plugin_id, want) in [(Some("herdr-estate"), painted), (None, "")] {
        let (f, root) = fixture();
        make_campaign(&root, "garden", "---\n---\n");
        let herdr_config = f.home.join(".config").join("herdr");
        std::fs::create_dir_all(&herdr_config).unwrap();
        std::fs::write(
            herdr_config.join("config.toml"),
            "[theme]\nname = \"vesper\"\n",
        )
        .unwrap();
        f.fake.set_response(0, "", 1);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-estate"));
        cmd.arg("pick")
            .env_clear()
            .env("HOME", &f.home)
            .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
            .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
            .env("HERDR_BIN_PATH", f.fake.bin_path())
            .env("PATH", "/usr/bin:/bin");
        if let Some(id) = plugin_id {
            cmd.env("HERDR_PLUGIN_ID", id);
        }
        let out = cmd.output().expect("run the built herdr-estate binary");
        assert!(!out.status.success(), "{plugin_id:?}");
        assert_eq!(stdout(&out), want, "{plugin_id:?}");
        assert!(stderr(&out).contains("herdr-estate:"), "{plugin_id:?}");
    }
}

#[test]
fn pick_rejects_an_unknown_flag() {
    let (f, _root) = fixture();
    let out = run(&f, &["pick", "--nope"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_pick_action_opens_the_popup_instead_of_drawing() {
    // How herdr runs the `pick` action from a key: no terminal, with
    // HERDR_PLUGIN_ACTION_ID set. It must ask herdr for the picker pane,
    // not try to draw (which failed with "could not start the terminal").
    let (f, _root) = fixture();
    f.fake.set_response(0, r#"{"result": {}}"#, 0);
    let out = Command::new(env!("CARGO_BIN_EXE_herdr-estate"))
        .arg("pick")
        .env_clear()
        .env("HOME", &f.home)
        .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
        .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
        .env("HERDR_BIN_PATH", f.fake.bin_path())
        .env("HERDR_PLUGIN_ID", "herdr-estate")
        .env("HERDR_PLUGIN_ACTION_ID", "pick")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the built herdr-estate binary");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.fake.call_count(), 1);
    assert_eq!(
        f.fake.argv(0),
        vec![
            "plugin",
            "pane",
            "open",
            "--plugin",
            "herdr-estate",
            "--entrypoint",
            "picker"
        ]
    );
    assert!(stderr(&out).contains("herdr-estate: opening the picker popup"));
    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    assert!(events
        .lines()
        .next()
        .unwrap()
        .ends_with(" run: herdr-estate pick (herdr action pick)"));
}

#[test]
fn the_dispatch_stop_action_replaces_the_workspace_its_context_names() {
    // How herdr runs `dispatch-stop` for a popup opened in w1: the
    // manifest's argv, no address, the address in the invocation
    // context. Everything after the close (create, ownership, agent
    // start, sessions.log, the focus) runs in this one process.
    let (f, root) = fixture();
    let cdir = make_campaign(&root, "garden", "---\n---\n");
    append_session(&cdir, "2026-09-10T00:00:00.000000+00:00", "old-sid");
    std::fs::create_dir_all(&f.state_dir).unwrap();
    std::fs::write(f.state_dir.join("owned.json"), r#"{"w1":"home/garden"}"#).unwrap();
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
    f.fake.set_response(2, r#"{"result": {}}"#, 0); // close
    f.fake.set_response(
        3,
        r#"{"result": {"workspace": {"workspace_id": "w9"}, "root_pane": {"pane_id": "w9:p1"}}}"#,
        0,
    ); // create
    f.fake.set_response(4, r#"{"result": {}}"#, 0); // agent start
    f.fake.set_response(5, r#"{"result": {}}"#, 0); // focus

    let context = r#"{"workspace_id":"w1","workspace_label":"home/garden"}"#;
    let out = run(
        &f,
        &["pick", "--dispatch", "stop"],
        &[
            ("HERDR_PLUGIN_ACTION_ID", "dispatch-stop"),
            ("HERDR_PLUGIN_CONTEXT_JSON", context),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.fake.call_count(), 6);
    assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
    assert_eq!(f.fake.argv(5), vec!["workspace", "focus", "w9"]);
    let sessions = std::fs::read_to_string(cdir.join("sessions.log")).unwrap();
    assert_eq!(sessions.lines().count(), 2, "{sessions}");
    let owned = std::fs::read_to_string(f.state_dir.join("owned.json")).unwrap();
    assert_eq!(owned, r#"{"w9":"home/garden"}"#);
}

#[test]
fn a_handed_off_refusal_exits_1_and_leaves_its_error_in_the_log() {
    // Refused before anything changes, so the popup is still there to
    // hold the error the handed-off process printed and logged.
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "working", "pane_count": 1}
        ]}}"#,
        0,
    );
    let out = run(&f, &["pick", "--dispatch", "stop", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.fake.call_count(), 1);
    assert!(
        stderr(&out).contains("herdr-estate: home/garden:"),
        "{}",
        stderr(&out)
    );
    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    let lines: Vec<&str> = events.lines().collect();
    assert!(lines[0].ends_with(" run: herdr-estate pick --dispatch stop home/garden (shell)"));
    assert!(
        lines.last().unwrap().contains(" error: home/garden:"),
        "{events}"
    );
}
