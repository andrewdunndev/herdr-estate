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

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use herdr_estate::picker::core::{self, AppState, Indicators, Row, RowState};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "herdr-estate-picker-test-{}-{n}-{nanos}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create test temp dir");
    path
}

struct FakeHerdr {
    dir: PathBuf,
}

impl FakeHerdr {
    fn new() -> Self {
        let dir = unique_dir("fake-herdr");
        std::fs::create_dir_all(dir.join("calls")).unwrap();
        let script = dir.join("herdr");
        let body = "#!/bin/sh\n\
            DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)/calls\"\n\
            IDXFILE=\"$DIR/count\"\n\
            if [ -f \"$IDXFILE\" ]; then IDX=$(cat \"$IDXFILE\"); else IDX=0; fi\n\
            echo $((IDX + 1)) > \"$IDXFILE\"\n\
            for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$DIR/$IDX.argv\"\n\
            if [ -f \"$DIR/$IDX.stdout\" ]; then cat \"$DIR/$IDX.stdout\"; fi\n\
            if [ -f \"$DIR/$IDX.exit\" ]; then exit \"$(cat \"$DIR/$IDX.exit\")\"; fi\n\
            exit 0\n";
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        FakeHerdr { dir }
    }

    fn bin_path(&self) -> PathBuf {
        self.dir.join("herdr")
    }

    fn set_response(&self, index: usize, stdout: &str, exit_code: i32) {
        std::fs::write(
            self.dir.join("calls").join(format!("{index}.stdout")),
            stdout,
        )
        .unwrap();
        std::fs::write(
            self.dir.join("calls").join(format!("{index}.exit")),
            exit_code.to_string(),
        )
        .unwrap();
    }

    fn call_count(&self) -> usize {
        std::fs::read_to_string(self.dir.join("calls").join("count"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    fn argv(&self, index: usize) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("calls").join(format!("{index}.argv")))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

struct Fixture {
    home: PathBuf,
    config_dir: PathBuf,
    state_dir: PathBuf,
    fake: FakeHerdr,
}

fn write_estate_toml(config_dir: &Path, contents: &str) {
    std::fs::create_dir_all(config_dir).unwrap();
    std::fs::write(config_dir.join("estates.toml"), contents).unwrap();
}

fn make_campaign(root: &Path, name: &str, frontmatter: &str) -> PathBuf {
    let cdir = root.join("campaigns").join(name);
    std::fs::create_dir_all(&cdir).unwrap();
    std::fs::write(cdir.join("campaign.md"), frontmatter).unwrap();
    cdir
}

fn append_session(cdir: &Path, when: &str, conversation_id: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cdir.join("sessions.log"))
        .unwrap();
    writeln!(f, "{when} host {conversation_id}").unwrap();
}

/// A fixture with one estate, `home`, rooted at the returned directory.
fn fixture() -> (Fixture, PathBuf) {
    let base = unique_dir("fixture");
    let root = base.join("home");
    let home = base.join("userhome");
    std::fs::create_dir_all(&home).unwrap();
    let config_dir = base.join("config");
    write_estate_toml(
        &config_dir,
        &format!("[estate.home]\nroot = {:?}\n", root.to_string_lossy()),
    );
    let f = Fixture {
        home,
        config_dir,
        state_dir: base.join("state"),
        fake: FakeHerdr::new(),
    };
    (f, root)
}

fn run(f: &Fixture, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-estate"));
    cmd.args(args)
        .env_clear()
        .env("HOME", &f.home)
        .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
        .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
        .env("HERDR_BIN_PATH", f.fake.bin_path())
        .env("PATH", "/usr/bin:/bin");
    cmd.output().expect("run the built herdr-estate binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn render_once_matches_the_pure_core_for_the_same_rows() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n"); // will be live
    let kitchen = make_campaign(&root, "kitchen", "---\n---\n"); // dormant
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
        },
        Row {
            address: "home/kitchen".to_string(),
            state: RowState::Dormant {
                last: "2026-09-10".to_string(),
            },
        },
        Row {
            address: "home/attic".to_string(),
            state: RowState::New,
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
    // The live row carries its dot: the glyphs came through.
    assert!(expected.contains("> ● home/garden"), "{expected}");
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
    let out = run(&f, &["pick", "--render-once"]);
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
    let out = run(&f, &["pick", "--render-once"]);
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
    let out = run(&f, &["pick", "--render-once"]);
    assert!(!out.status.success());
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).contains("herdr-estate:"));
}

#[test]
fn pick_rejects_an_unknown_flag() {
    let (f, _root) = fixture();
    let out = run(&f, &["pick", "--nope"]);
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
