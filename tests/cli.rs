//! Integration tests for the built `herdr-estate` binary: subcommands,
//! `--dry-run`, exit codes, and address derivation from
//! `HERDR_WORKSPACE_ID`.
//!
//! Each test spawns the compiled binary (`env!("CARGO_BIN_EXE_herdr-estate")`)
//! as a child process with its OWN explicit environment built on
//! `env_clear()` — never the ambient process environment — so tests need
//! no shared env-mutation guard and run safely in parallel. `HOME`,
//! `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR` all point at a
//! fresh temp directory per test, and `HERDR_BIN_PATH` points at a fake
//! `herdr` that records its exact argv per call and replays canned JSON.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "herdr-estate-cli-test-{}-{n}-{nanos}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create test temp dir");
    path
}

/// A fake `herdr` that records its argv per call (0-based, in call
/// order) to a calls directory, and replays whatever canned stdout/exit
/// code was queued for that call index before the binary under test ran.
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

fn run(f: &Fixture, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-estate"));
    cmd.args(args)
        .env_clear()
        .env("HOME", &f.home)
        .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
        .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
        .env("HERDR_BIN_PATH", f.fake.bin_path())
        // `git` (worktrees), `hostname` (continuity) and `kill`
        // (doubleopen's liveness probe) all live under these two.
        .env("PATH", "/usr/bin:/bin");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run the built herdr-estate binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn list_with_no_campaigns_prints_the_empty_message() {
    let (f, _root) = fixture();
    let out = run(&f, &["list"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "no campaigns found");
    assert_eq!(f.fake.call_count(), 0);
}

#[test]
fn list_with_campaigns_prints_a_row_per_campaign() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    let out = run(&f, &["list"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("home/garden"));
    assert!(text.contains("new"));
}

#[test]
fn missing_subcommand_exits_2() {
    let (f, _root) = fixture();
    let out = run(&f, &[], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unknown_subcommand_exits_2() {
    let (f, _root) = fixture();
    let out = run(&f, &["launch"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn list_rejects_a_dry_run_flag_it_does_not_take() {
    let (f, _root) = fixture();
    let out = run(&f, &["list", "--dry-run"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn dry_run_without_an_address_refuses_without_calling_herdr() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    let out = run(&f, &["resume", "--dry-run"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("herdr-estate:"));
    assert_eq!(f.fake.call_count(), 0);
}

#[test]
fn resume_dry_run_prints_the_plan_and_masks_estate_env() {
    let base = unique_dir("dryrun");
    let root = base.join("home");
    make_campaign(&root, "garden", "---\n---\n");
    let config_dir = base.join("config");
    write_estate_toml(
        &config_dir,
        &format!(
            "[estate.home]\nroot = {:?}\n[estate.home.env]\nSECRET_TOKEN = \"sk-do-not-print-me\"\n",
            root.to_string_lossy()
        ),
    );
    let home = base.join("userhome");
    std::fs::create_dir_all(&home).unwrap();
    let fake = FakeHerdr::new();
    fake.set_response(0, r#"{"result": {"workspaces": []}}"#, 0);

    let out = Command::new(env!("CARGO_BIN_EXE_herdr-estate"))
        .args(["resume", "home/garden", "--dry-run"])
        .env_clear()
        .env("HOME", &home)
        .env("HERDR_PLUGIN_CONFIG_DIR", &config_dir)
        .env("HERDR_PLUGIN_STATE_DIR", base.join("state"))
        .env("HERDR_BIN_PATH", fake.bin_path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the built binary");

    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains("sk-do-not-print-me"));
    assert!(text.contains("SECRET_TOKEN=<from estates.toml>"));
    assert!(text.contains("would create:"));
    assert!(text.contains("would start:"));
    assert_eq!(fake.call_count(), 1);
    assert_eq!(fake.argv(0), vec!["workspace", "list"]);

    // A dry run never mutates the continuity log.
    assert!(!root.join("campaigns/garden/sessions.log").exists());
}

#[test]
fn fresh_happy_path_creates_starts_and_logs() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    f.fake.set_response(
        1,
        r#"{"result": {
            "workspace": {"workspace_id": "w9", "label": "home/garden"},
            "root_pane": {"pane_id": "p1"}
        }}"#,
        0,
    );
    f.fake.set_response(2, r#"{"result": {}}"#, 0);

    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("live"));
    assert_eq!(f.fake.call_count(), 3);
    assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);

    let log = std::fs::read_to_string(root.join("campaigns/garden/sessions.log")).unwrap();
    assert_eq!(log.lines().count(), 1);

    // The progress lines go to stderr, never stdout, which scripts read.
    let err = stderr(&out);
    assert!(err.contains("herdr-estate: home/garden: creating a workspace"));
    assert!(err.contains("herdr-estate: home/garden: starting claude in w9"));
    assert!(err.contains("herdr-estate: home/garden: claude is ready in w9"));
    assert!(!stdout(&out).contains("creating a workspace"));

    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    let first = events.lines().next().unwrap();
    assert!(first.ends_with(" run: herdr-estate fresh home/garden (shell)"));
    assert!(events
        .lines()
        .last()
        .unwrap()
        .ends_with("claude is ready in w9"));
}

#[test]
fn a_failed_create_keeps_estate_env_values_out_of_stderr_and_the_event_log() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    write_estate_toml(
        &f.config_dir,
        &format!(
            "[estate.home]\nroot = {:?}\n[estate.home.env]\nSECRET_TOKEN = \"sk-do-not-print-me\"\n",
            root.to_string_lossy()
        ),
    );
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    f.fake.set_response(1, "", 1); // workspace create fails

    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
    // The real value did go to herdr; only what is shown is masked.
    assert!(f
        .fake
        .argv(1)
        .contains(&"SECRET_TOKEN=sk-do-not-print-me".to_string()));
    let err = stderr(&out);
    assert!(err.contains("SECRET_TOKEN=<from estates.toml>"), "{err}");
    assert!(!err.contains("sk-do-not-print-me"));
    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    assert!(events.contains("error: herdr workspace create"));
    assert!(!events.contains("sk-do-not-print-me"));
}

#[test]
fn fresh_alive_workspace_refuses_and_exits_1() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
        ]}}"#,
        0,
    );
    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("herdr-estate:"));
    assert_eq!(f.fake.call_count(), 1); // only the read, nothing closed or created
}

/// Record `workspace_id` as ours in `f`'s state dir, the same file
/// `ownership::record` writes (`{workspace_id: address}` at
/// `owned.json`), so a dry run's close-line check finds it.
fn record_owned(f: &Fixture, workspace_id: &str, address: &str) {
    std::fs::create_dir_all(&f.state_dir).unwrap();
    std::fs::write(
        f.state_dir.join("owned.json"),
        format!("{{\"{workspace_id}\": \"{address}\"}}"),
    )
    .unwrap();
}

#[test]
fn fresh_dry_run_close_line_names_the_owned_dormant_workspace() {
    let (f, root) = fixture();
    let cdir = make_campaign(&root, "garden", "---\n---\n");
    std::fs::write(
        cdir.join("sessions.log"),
        "2026-09-21T00:00:00Z testhost old-sid\n",
    )
    .unwrap();
    record_owned(&f, "w1", "home/garden");
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
        ]}}"#,
        0,
    );
    // The dry run re-confirms the label the same read-only way the real
    // close would, before it prints "would close:".
    f.fake.set_response(
        1,
        r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
        0,
    );
    let out = run(&f, &["fresh", "home/garden", "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("would close:"), "{text}");
    assert!(text.contains("w1"), "{text}");
    assert_eq!(f.fake.call_count(), 2);
}

#[test]
fn fresh_dry_run_close_line_shows_even_without_a_recorded_conversation() {
    // The close line tracks live workspace state only, independent of
    // whether a conversation happens to be recorded.
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n"); // no sessions.log written
    record_owned(&f, "w1", "home/garden");
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
        ]}}"#,
        0,
    );
    f.fake.set_response(
        1,
        r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
        0,
    );
    let out = run(&f, &["fresh", "home/garden", "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("would close:"));
}

#[test]
fn fresh_dry_run_no_close_line_without_a_live_workspace() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    let out = run(&f, &["fresh", "home/garden", "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("would close:"));
}

#[test]
fn address_derived_from_herdr_workspace_id_when_omitted() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    f.fake.set_response(
        0,
        r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
        0,
    );
    f.fake
        .set_response(1, r#"{"result": {"workspaces": []}}"#, 0);
    f.fake.set_response(
        2,
        r#"{"result": {
            "workspace": {"workspace_id": "w9", "label": "home/garden"},
            "root_pane": {"pane_id": "p1"}
        }}"#,
        0,
    );
    f.fake.set_response(3, r#"{"result": {}}"#, 0);

    let out = run(&f, &["fresh"], &[("HERDR_WORKSPACE_ID", "w1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(f.fake.argv(0), vec!["workspace", "get", "w1"]);
}

#[test]
fn missing_estate_table_is_a_handled_error_exit_1() {
    let base = unique_dir("noestates");
    let home = base.join("userhome");
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_herdr-estate"))
        .args(["list"])
        .env_clear()
        .env("HOME", &home)
        .env("HERDR_PLUGIN_CONFIG_DIR", base.join("config-not-written"))
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the built binary");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("herdr-estate:"));
}

#[test]
fn unknown_campaign_is_a_handled_error_not_a_crash() {
    let (f, _root) = fixture();
    let out = run(&f, &["resume", "home/nope"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("herdr-estate:"));
    assert_eq!(f.fake.call_count(), 0);
}
