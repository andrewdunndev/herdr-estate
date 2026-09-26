//! Integration tests for the built `herdr-estate` binary: subcommands,
//! `--dry-run`, exit codes, `create`, and address derivation from
//! `HERDR_WORKSPACE_ID`.
//!
//! Each test spawns the compiled binary (`env!("CARGO_BIN_EXE_herdr-estate")`)
//! as a child process with its OWN explicit environment built on
//! `env_clear()` — never the ambient process environment — so tests need
//! no shared env-mutation guard and run safely in parallel. `HOME`,
//! `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR` all point at a
//! fresh temp directory per test, and `HERDR_BIN_PATH` points at a fake
//! `herdr` that records its exact argv per call and replays canned JSON.

use std::process::Command;

mod common;
use common::*;

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
fn version_prints_the_package_version_and_writes_no_events_log() {
    let (f, _root) = fixture();
    for flag in ["--version", "-V"] {
        let out = run(&f, &[flag], &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(
            stdout(&out).trim(),
            format!("herdr-estate {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(f.fake.call_count(), 0);
        assert!(!f.state_dir.join("events.log").exists());
    }
}

#[test]
fn help_prints_every_verb_and_writes_no_events_log() {
    let (f, _root) = fixture();
    for argv in [
        &["--help"][..],
        &["-h"],
        &["resume", "--help"],
        &["pick", "-h"],
    ] {
        let out = run(&f, argv, &[]);
        assert!(out.status.success(), "{argv:?}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.starts_with("usage: herdr-estate <verb>"), "{text}");
        for verb in [
            "resume", "fresh", "list", "create", "pick", "--width", "--height",
        ] {
            assert!(text.contains(verb), "{verb} missing from {text}");
        }
        assert_eq!(f.fake.call_count(), 0);
        assert!(!f.state_dir.join("events.log").exists());
    }
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
            "[estate.home]\nroot = {:?}\nprepare = [\"/bin/true\", \"--check\"]\n[estate.home.env]\nSECRET_TOKEN = \"sk-do-not-print-me\"\n",
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
    assert!(text.contains("would prepare:"));
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
fn a_missing_path_is_in_the_dry_run_plan_and_never_in_the_log() {
    let (f, root) = fixture();
    let absent = root.join("no-such-dir");
    make_campaign(
        &root,
        "garden",
        &format!("---\npaths: [{}]\n---\n", absent.display()),
    );
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    let out = run(&f, &["fresh", "home/garden", "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!(
            "would skip, not there:\n  add-dir {}\n",
            absent.display()
        )),
        "{}",
        stdout(&out)
    );
    assert!(!stderr(&out).contains("add-dir"), "{}", stderr(&out));
    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    assert_eq!(events.lines().count(), 1, "only the run line: {events}");
}

#[test]
fn a_missing_path_is_reported_once_the_run_passes_its_guards() {
    let (f, root) = fixture();
    let absent = root.join("no-such-dir");
    make_campaign(
        &root,
        "garden",
        &format!("---\npaths: [{}]\n---\n", absent.display()),
    );
    // Refused on a running agent: nothing about the path is said.
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
        ]}}"#,
        0,
    );
    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stderr(&out).contains("add-dir"), "{}", stderr(&out));

    // Past the guards, the skip is one progress line before the create.
    f.fake
        .set_response(1, r#"{"result": {"workspaces": []}}"#, 0);
    f.fake.set_response(
        2,
        r#"{"result": {"workspace": {"workspace_id": "w9"}, "root_pane": {"pane_id": "w9:p1"}}}"#,
        0,
    );
    f.fake.set_response(3, r#"{"result": {}}"#, 0);
    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    let skip = err
        .find(&format!(
            "add-dir {} does not exist, skipping",
            absent.display()
        ))
        .unwrap_or_else(|| panic!("{err}"));
    assert!(skip < err.find("creating a workspace").unwrap(), "{err}");
}

/// A git repo with one commit at `path`, for a campaign's `worktree:`.
fn init_repo(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        let ok = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("PATH", "/usr/bin:/bin")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
}

/// A fixture whose `home/garden` names a worktree on a fresh repo, and
/// the directory that worktree lands in once created.
fn worktree_fixture() -> (Fixture, std::path::PathBuf) {
    let (f, root) = fixture();
    let repo = root.parent().unwrap().join("repo");
    init_repo(&repo);
    make_campaign(
        &root,
        "garden",
        &format!(
            "---\nworktree:\n  repo: {}\n  branch: wt/garden\n---\n",
            repo.display()
        ),
    );
    let worktree = f.state_dir.join("worktrees/repo/wt-garden");
    (f, worktree)
}

#[test]
fn a_refused_launch_leaves_no_worktree_behind() {
    let (f, worktree) = worktree_fixture();
    f.fake.set_response(
        0,
        r#"{"result": {"workspaces": [
            {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
        ]}}"#,
        0,
    );
    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("already open"), "{}", stderr(&out));
    assert!(
        !worktree.exists(),
        "refused, yet {} exists",
        worktree.display()
    );
}

#[test]
fn a_dry_run_names_the_worktree_and_creates_nothing() {
    let (f, worktree) = worktree_fixture();
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    let out = run(&f, &["fresh", "home/garden", "--dry-run"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!(
            "home/garden: would ensure worktree: {}",
            worktree.display()
        )),
        "{}",
        stdout(&out)
    );
    assert!(!worktree.exists());
}

#[test]
fn a_launch_past_its_guards_creates_the_worktree_and_starts_there() {
    let (f, worktree) = worktree_fixture();
    f.fake
        .set_response(0, r#"{"result": {"workspaces": []}}"#, 0);
    f.fake.set_response(
        1,
        r#"{"result": {"workspace": {"workspace_id": "w9"}, "root_pane": {"pane_id": "w9:p1"}}}"#,
        0,
    );
    f.fake.set_response(2, r#"{"result": {}}"#, 0);
    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(worktree.is_dir());
    let create = f.fake.argv(1);
    let cwd = &create[create.iter().position(|a| a == "--cwd").unwrap() + 1];
    assert_eq!(
        std::fs::canonicalize(cwd).unwrap(),
        std::fs::canonicalize(&worktree).unwrap()
    );
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

#[test]
fn create_writes_the_campaign_and_never_calls_herdr() {
    let (f, root) = fixture();
    std::fs::create_dir_all(root.join("campaigns")).unwrap();
    // Any herdr call would fail the run: create makes none.
    f.fake.set_response(0, "", 1);
    let out = run(&f, &["create", "home/quern", "--brief", "Grind it."], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let dir = root.join("campaigns/quern");
    assert_eq!(
        stdout(&out),
        format!("created  home/quern  {}\n", dir.display())
    );
    assert!(stderr(&out).contains(&format!(
        "herdr-estate: home/quern: creating campaign.md and log.md in {} (untracked)",
        dir.display()
    )));
    assert!(dir.join("campaign.md").is_file());
    assert!(dir.join("log.md").is_file());
    assert_eq!(f.fake.call_count(), 0);
}

#[test]
fn create_dry_run_lists_both_files_and_writes_and_logs_nothing() {
    let (f, root) = fixture();
    std::fs::create_dir_all(root.join("campaigns")).unwrap();
    f.fake.set_response(0, "", 1);
    let out = run(
        &f,
        &["create", "home/quern", "--brief", "Grind it.", "--dry-run"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let dir = root.join("campaigns/quern");
    assert_eq!(
        stdout(&out),
        format!(
            "would create:\n  {}\n  {}\n",
            dir.join("campaign.md").display(),
            dir.join("log.md").display()
        )
    );
    assert!(!dir.exists());
    assert_eq!(f.fake.call_count(), 0);
    // The run line, and no progress line after it.
    let events = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    assert_eq!(events.lines().count(), 1, "{events}");
}

#[test]
fn create_without_its_address_or_its_brief_exits_2() {
    let (f, _root) = fixture();
    for args in [&["create", "home/quern"][..], &["create", "--brief", "b"]] {
        assert_eq!(run(&f, args, &[]).status.code(), Some(2), "{args:?}");
    }
}
