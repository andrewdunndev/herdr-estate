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
            "resume", "fresh", "list", "create", "pick", "sort", "--width", "--height",
        ] {
            assert!(text.contains(verb), "{verb} missing from {text}");
        }
        assert_eq!(f.fake.call_count(), 0);
        assert!(!f.state_dir.join("events.log").exists());
    }
}

#[test]
fn sort_outside_herdr_fails_naming_the_socket_and_needs_no_estate_table() {
    let (f, _root) = fixture();
    std::fs::remove_file(f.config_dir.join("estates.toml")).unwrap();
    let out = run(&f, &["sort"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("sort: workspace.list: $HERDR_SOCKET_PATH is unset"),
        "{}",
        stderr(&out)
    );
    assert_eq!(f.fake.call_count(), 0);
}

#[test]
fn sort_as_a_created_hook_that_cannot_reach_herdr_exits_0_and_logs_it() {
    let (f, _root) = fixture();
    let out = run(
        &f,
        &["sort"],
        &[("HERDR_PLUGIN_EVENT", "workspace.created")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let log = std::fs::read_to_string(f.state_dir.join("events.log")).unwrap();
    assert!(
        log.contains("run: herdr-estate sort (herdr event workspace.created)"),
        "{log}"
    );
    assert!(log.contains("error: sort: workspace.list:"), "{log}");
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
    f.fake.set_response(1, "", 1);

    let out = run(&f, &["fresh", "home/garden"], &[]);
    assert_eq!(out.status.code(), Some(1));
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
    assert_eq!(f.fake.call_count(), 1);
}

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
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
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
fn a_launch_its_guards_refuse_leaves_no_worktree_behind() {
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
fn two_concurrent_launches_of_one_address_create_one_workspace() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    write_estate_toml(
        &f.config_dir,
        &format!(
            "[estate.home]\nroot = {:?}\nprepare = [\"/bin/sleep\", \"1\"]\n",
            root.to_string_lossy()
        ),
    );
    let fake_dir = root.parent().unwrap().join("race-herdr");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let creates = fake_dir.join("creates");
    let script = fake_dir.join("herdr");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n\
             \"workspace list\") echo '{{\"result\":{{\"workspaces\":[]}}}}' ;;\n\
             \"workspace create\") echo x >> {creates:?}; \
             echo '{{\"result\":{{\"workspace\":{{\"workspace_id\":\"w9\"}},\"root_pane\":{{\"pane_id\":\"w9:p1\"}}}}}}' ;;\n\
             *) echo '{{\"result\":{{}}}}' ;;\nesac\n"
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let launch = || {
        Command::new(env!("CARGO_BIN_EXE_herdr-estate"))
            .args(["fresh", "home/garden"])
            .env_clear()
            .env("HOME", &f.home)
            .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
            .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
            .env("HERDR_BIN_PATH", &script)
            .env("PATH", "/usr/bin:/bin")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let (first, second) = (launch(), launch());
    let outs = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    let created = std::fs::read_to_string(&creates).unwrap_or_default();
    assert_eq!(created.lines().count(), 1, "{outs:?}");
    let refused: Vec<_> = outs.iter().filter(|o| !o.status.success()).collect();
    assert_eq!(refused.len(), 1, "{outs:?}");
    assert!(
        stderr(refused[0]).contains("another herdr-estate run"),
        "{}",
        stderr(refused[0])
    );
}

const STATEFUL_HERDR: &str = r#"#!/bin/sh
D="$(cd "$(dirname "$0")" && pwd)"
case "$1 $2" in
"workspace list")
  printf '{"result":{"workspaces":['
  sep=""
  if [ -f "$D/ws" ]; then
    while read -r id label; do
      printf '%s{"workspace_id":"%s","label":"%s","agent_status":"idle","pane_count":1}' "$sep" "$id" "$label"
      sep=","
    done < "$D/ws"
  fi
  printf ']}}\n' ;;
"workspace create")
  prev=""; label=""
  for a in "$@"; do [ "$prev" = "--label" ] && label="$a"; prev="$a"; done
  id="w$$"
  echo "$id $label" >> "$D/ws"
  printf '{"result":{"workspace":{"workspace_id":"%s"},"root_pane":{"pane_id":"%s:p1"}}}\n' "$id" "$id" ;;
*) echo '{"result":{}}' ;;
esac
"#;

#[test]
fn concurrent_launches_of_different_addresses_keep_every_ownership_record() {
    let names = ["garden", "attic", "cellar", "kitchen", "shed", "loft"];
    let (f, root) = fixture();
    for name in names {
        make_campaign(&root, name, "---\n---\n");
    }
    write_estate_toml(
        &f.config_dir,
        &format!(
            "[estate.home]\nroot = {:?}\nprepare = [\"/bin/sleep\", \"1\"]\n",
            root.to_string_lossy()
        ),
    );
    let fake_dir = root.parent().unwrap().join("stateful-herdr");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let script = fake_dir.join("herdr");
    std::fs::write(&script, STATEFUL_HERDR).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let children: Vec<_> = names
        .iter()
        .map(|name| {
            Command::new(env!("CARGO_BIN_EXE_herdr-estate"))
                .args(["fresh", &format!("home/{name}")])
                .env_clear()
                .env("HOME", &f.home)
                .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
                .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
                .env("HERDR_BIN_PATH", &script)
                .env("PATH", "/usr/bin:/bin")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
    }
    let owned: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(f.state_dir.join("owned.json")).unwrap())
            .unwrap();
    let mut addresses: Vec<&str> = owned.values().map(String::as_str).collect();
    addresses.sort_unstable();
    let mut want: Vec<String> = names.iter().map(|n| format!("home/{n}")).collect();
    want.sort_unstable();
    assert_eq!(addresses, want, "{owned:?}");
}

#[test]
fn a_launch_killed_while_holding_the_lock_frees_it_at_once() {
    let (f, root) = fixture();
    make_campaign(&root, "garden", "---\n---\n");
    let base = root.parent().unwrap().to_path_buf();
    let hold = base.join("hold");
    let started = base.join("started");
    let sleeper = base.join("sleeper.pid");
    std::fs::write(&hold, "").unwrap();
    let prepare = format!(
        "if [ -f {hold:?} ]; then echo $$ > {sleeper:?}; touch {started:?}; exec sleep 30; fi"
    );
    write_estate_toml(
        &f.config_dir,
        &format!(
            "[estate.home]\nroot = {:?}\nprepare = [\"/bin/sh\", \"-c\", {prepare:?}]\n",
            root.to_string_lossy()
        ),
    );
    let fake_dir = base.join("stateful-herdr");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let script = fake_dir.join("herdr");
    std::fs::write(&script, STATEFUL_HERDR).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let launch = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-estate"));
        command
            .args(["fresh", "home/garden"])
            .env_clear()
            .env("HOME", &f.home)
            .env("HERDR_PLUGIN_CONFIG_DIR", &f.config_dir)
            .env("HERDR_PLUGIN_STATE_DIR", &f.state_dir)
            .env("HERDR_BIN_PATH", &script)
            .env("PATH", "/usr/bin:/bin")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command
    };
    let mut holder = launch().spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !started.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(started.exists(), "the first launch never reached prepare");
    let refused = launch().output().unwrap();
    assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("another herdr-estate run (pid"),
        "{}",
        stderr(&refused)
    );

    holder.kill().unwrap();
    holder.wait().unwrap();
    std::fs::remove_file(&hold).unwrap();
    let at = std::time::Instant::now();
    let out = launch().output().unwrap();
    let elapsed = at.elapsed();
    let sleeper_pid = std::fs::read_to_string(&sleeper).unwrap_or_default();
    let orphan_alive = Command::new("kill")
        .args(["-0", sleeper_pid.trim()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = Command::new("kill").arg(sleeper_pid.trim()).status();
    assert!(orphan_alive, "the orphaned prepare should still be running");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
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
    for i in [1, 2] {
        f.fake
            .set_response(i, r#"{"result": {"workspaces": []}}"#, 0);
    }
    f.fake.set_response(
        3,
        r#"{"result": {
            "workspace": {"workspace_id": "w9", "label": "home/garden"},
            "root_pane": {"pane_id": "p1"}
        }}"#,
        0,
    );
    f.fake.set_response(4, r#"{"result": {}}"#, 0);

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
