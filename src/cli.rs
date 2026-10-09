use crate::config;
use crate::detach::{self, RowAction};
use crate::dispatch;
use crate::errors::EstateError;
use crate::picker;
use crate::progress;
use crate::sort;
use crate::verbs;

pub const PICK_DEFAULT_WIDTH: usize = 80;
pub const PICK_DEFAULT_HEIGHT: usize = 20;

pub const USAGE: &str = "\
usage: herdr-estate <verb> [options]

  resume [<address>] [--dry-run]    continue the latest conversation, or
                                    start fresh if none is on record
  fresh [<address>] [--dry-run]     retire the latest conversation and
                                    start a new one
  list                              every campaign in the estate table
  create <address> --brief <text> [--dry-run]
                                    write a new campaign's campaign.md
                                    and log.md, uncommitted
  pick                              the campaign picker; inside herdr it
                                    opens as a popup
  pick --render-once [--width <n>] [--height <n>]
                                    print one frame, 80x20 by default
  sort                              order herdr's workspaces by label,
                                    ~ first; herdr runs it on every
                                    new workspace

With no address, the current workspace's label is used; --dry-run
prints the plan, changes nothing, and needs an address.

  -h, --help       print this and exit
  -V, --version    print the version and exit
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Resume {
        address: Option<String>,
        dry_run: bool,
    },
    Fresh {
        address: Option<String>,
        dry_run: bool,
    },
    List,
    Create {
        address: String,
        brief: String,
        dry_run: bool,
    },
    Pick {
        render_once: bool,
        width: usize,
        height: usize,
    },
    PickDispatch {
        action: RowAction,
    },
    Sort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

pub fn parse_args(argv: &[String]) -> Result<Command, ParseError> {
    let verb = argv.first().ok_or_else(|| ParseError {
        message: "the following arguments are required: verb".to_string(),
    })?;
    let rest = &argv[1..];
    match verb.as_str() {
        "resume" | "fresh" => {
            let mut address: Option<String> = None;
            let mut dry_run = false;
            for a in rest {
                if a == "--dry-run" {
                    dry_run = true;
                } else if a.starts_with('-') || address.is_some() {
                    return Err(ParseError {
                        message: format!("unrecognized arguments: {a}"),
                    });
                } else {
                    address = Some(a.clone());
                }
            }
            if verb == "resume" {
                Ok(Command::Resume { address, dry_run })
            } else {
                Ok(Command::Fresh { address, dry_run })
            }
        }
        "list" | "sort" => {
            if !rest.is_empty() {
                return Err(ParseError {
                    message: format!("unrecognized arguments: {}", rest.join(" ")),
                });
            }
            Ok(if verb == "list" {
                Command::List
            } else {
                Command::Sort
            })
        }
        "create" => parse_create(rest),
        "pick" => parse_pick(rest),
        other => Err(ParseError {
            message: format!(
                "argument verb: invalid choice: {other:?} (choose from 'resume', 'fresh', 'list', 'create', 'pick', 'sort')"
            ),
        }),
    }
}

fn parse_create(rest: &[String]) -> Result<Command, ParseError> {
    let mut address = None;
    let mut brief = None;
    let mut dry_run = false;
    let mut args = rest.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--brief" => {
                let text = args.next().filter(|b| !b.starts_with('-'));
                brief = Some(text.cloned().ok_or_else(|| ParseError {
                    message: "argument --brief: expected one argument".to_string(),
                })?);
            }
            _ if a.starts_with('-') || address.is_some() => {
                return Err(ParseError {
                    message: format!("unrecognized arguments: {a}"),
                })
            }
            _ => address = Some(a.clone()),
        }
    }
    match (address, brief) {
        (Some(address), Some(brief)) => Ok(Command::Create {
            address,
            brief,
            dry_run,
        }),
        _ => Err(ParseError {
            message: "the following arguments are required: address, --brief".to_string(),
        }),
    }
}

fn parse_pick(rest: &[String]) -> Result<Command, ParseError> {
    if rest.first().map(String::as_str) == Some("--dispatch") {
        return parse_pick_dispatch(&rest[1..]);
    }
    let mut render_once = false;
    let mut width = PICK_DEFAULT_WIDTH;
    let mut height = PICK_DEFAULT_HEIGHT;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--render-once" => render_once = true,
            "--width" => {
                i += 1;
                width = parse_dimension(rest.get(i), "--width")?;
            }
            "--height" => {
                i += 1;
                height = parse_dimension(rest.get(i), "--height")?;
            }
            other => {
                return Err(ParseError {
                    message: format!("unrecognized arguments: {other}"),
                })
            }
        }
        i += 1;
    }
    Ok(Command::Pick {
        render_once,
        width,
        height,
    })
}

fn parse_pick_dispatch(rest: &[String]) -> Result<Command, ParseError> {
    let [action] = rest else {
        return Err(ParseError {
            message: "argument --dispatch: expected one action".to_string(),
        });
    };
    let action = RowAction::from_arg(action).ok_or_else(|| ParseError {
        message: format!(
            "argument --dispatch: invalid choice: {action:?} (choose from 'open', 'fresh', 'stop')"
        ),
    })?;
    Ok(Command::PickDispatch { action })
}

fn parse_dimension(value: Option<&String>, flag: &str) -> Result<usize, ParseError> {
    let raw = value.ok_or_else(|| ParseError {
        message: format!("argument {flag}: expected one argument"),
    })?;
    raw.parse::<usize>().map_err(|_| ParseError {
        message: format!("argument {flag}: invalid value: {raw:?}"),
    })
}

fn resolve_address(address: Option<String>, dry_run: bool) -> Result<String, EstateError> {
    if let Some(a) = address {
        return Ok(a);
    }
    if dry_run {
        return Err(EstateError::Usage(
            "--dry-run needs an explicit <address>, deriving one from the current workspace would itself call herdr"
                .to_string(),
        ));
    }
    verbs::derive_address()
}

fn dispatch(cmd: Command) -> Result<i32, EstateError> {
    match cmd {
        Command::Resume { address, dry_run } => {
            let estates = config::load_estates(None)?;
            let addr = resolve_address(address, dry_run)?;
            if !dry_run {
                if let Some(ok) = detach::shell_hand_off(RowAction::Open, &addr)? {
                    return Ok(if ok { 0 } else { 1 });
                }
            }
            verbs::resume(&estates, &addr, dry_run)
        }
        Command::Fresh { address, dry_run } => {
            let estates = config::load_estates(None)?;
            let addr = resolve_address(address, dry_run)?;
            if !dry_run {
                if let Some(ok) = detach::shell_hand_off(RowAction::Fresh, &addr)? {
                    return Ok(if ok { 0 } else { 1 });
                }
            }
            verbs::fresh(&estates, &addr, dry_run)
        }
        Command::List => {
            let estates = config::load_estates(None)?;
            let rows = verbs::list_campaigns(&estates);
            if rows.is_empty() {
                println!("no campaigns found");
                return Ok(0);
            }
            for row in &rows {
                println!("{}", verbs::format_row(row));
            }
            Ok(0)
        }
        Command::Create {
            address,
            brief,
            dry_run,
        } => {
            let estates = config::load_estates(None)?;
            verbs::create_campaign(&estates, &address, &brief, dry_run).map(|()| 0)
        }
        Command::Pick {
            render_once,
            width,
            height,
        } => {
            if picker::shell::opens_popup_instead_of_drawing(render_once) {
                return picker::shell::open_popup();
            }
            let estates = config::load_estates(None)?;
            picker::shell::run(&estates, render_once, width, height)
        }
        Command::PickDispatch { action } => {
            let estates = config::load_estates(None)?;
            let addr = detach::context_address()?;
            dispatch::run_action(&estates, action, &addr).map(|()| 0)
        }
        Command::Sort => Ok(sort::run()),
    }
}

pub fn run(argv: &[String]) -> i32 {
    if argv.first().map(String::as_str) == Some("--version")
        || argv.first().map(String::as_str) == Some("-V")
    {
        println!("herdr-estate {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return 0;
    }
    progress::append(&format!(
        "run: herdr-estate {} ({})",
        argv.join(" "),
        progress::launched_via()
    ));
    let cmd = match parse_args(argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("herdr-estate: error: {}", e.message);
            progress::append(&format!("error: {}", e.message));
            return 2;
        }
    };
    match dispatch(cmd) {
        Ok(rc) => rc,
        Err(e) => {
            progress::report_error(&e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_requires_a_verb() {
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn parse_rejects_unknown_verb() {
        assert!(parse_args(&args(&["launch"])).is_err());
    }

    #[test]
    fn parse_resume_with_address() {
        assert_eq!(
            parse_args(&args(&["resume", "home/garden"])).unwrap(),
            Command::Resume {
                address: Some("home/garden".to_string()),
                dry_run: false
            }
        );
    }

    #[test]
    fn parse_resume_with_dry_run_and_no_address() {
        assert_eq!(
            parse_args(&args(&["resume", "--dry-run"])).unwrap(),
            Command::Resume {
                address: None,
                dry_run: true
            }
        );
    }

    #[test]
    fn parse_fresh_with_address_and_dry_run_either_order() {
        assert_eq!(
            parse_args(&args(&["fresh", "--dry-run", "home/garden"])).unwrap(),
            Command::Fresh {
                address: Some("home/garden".to_string()),
                dry_run: true
            }
        );
        assert_eq!(
            parse_args(&args(&["fresh", "home/garden", "--dry-run"])).unwrap(),
            Command::Fresh {
                address: Some("home/garden".to_string()),
                dry_run: true
            }
        );
    }

    #[test]
    fn parse_list_takes_no_arguments() {
        assert_eq!(parse_args(&args(&["list"])).unwrap(), Command::List);
        assert!(parse_args(&args(&["list", "--dry-run"])).is_err());
        assert!(parse_args(&args(&["list", "home/garden"])).is_err());
    }

    #[test]
    fn parse_sort_takes_no_arguments() {
        assert_eq!(parse_args(&args(&["sort"])).unwrap(), Command::Sort);
        assert!(parse_args(&args(&["sort", "--dry-run"])).is_err());
        assert!(parse_args(&args(&["sort", "home/garden"])).is_err());
    }

    #[test]
    fn parse_create_needs_an_address_and_a_brief() {
        assert_eq!(
            parse_args(&args(&[
                "create",
                "--brief",
                "b",
                "home/quern",
                "--dry-run"
            ]))
            .unwrap(),
            Command::Create {
                address: "home/quern".to_string(),
                brief: "b".to_string(),
                dry_run: true
            }
        );
        for bad in [
            &["create", "home/quern"][..],
            &["create", "--brief", "b"],
            &["create", "home/quern", "--brief"],
            &["create", "home/quern", "--brief", "--dry-run"],
            &["create", "home/a", "home/b", "--brief", "b"],
            &["create", "home/quern", "--nope", "--brief", "b"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parse_rejects_a_second_positional() {
        assert!(parse_args(&args(&["resume", "home/garden", "home/taxes"])).is_err());
    }

    #[test]
    fn parse_pick_with_no_flags_uses_the_popup_defaults() {
        assert_eq!(
            parse_args(&args(&["pick"])).unwrap(),
            Command::Pick {
                render_once: false,
                width: PICK_DEFAULT_WIDTH,
                height: PICK_DEFAULT_HEIGHT,
            }
        );
    }

    #[test]
    fn parse_pick_render_once_with_width_and_height() {
        assert_eq!(
            parse_args(&args(&[
                "pick",
                "--render-once",
                "--width",
                "40",
                "--height",
                "10"
            ]))
            .unwrap(),
            Command::Pick {
                render_once: true,
                width: 40,
                height: 10,
            }
        );
    }

    #[test]
    fn parse_pick_rejects_a_non_numeric_width() {
        assert!(parse_args(&args(&["pick", "--width", "wide"])).is_err());
    }

    #[test]
    fn parse_pick_rejects_a_dangling_width_flag() {
        assert!(parse_args(&args(&["pick", "--width"])).is_err());
    }

    #[test]
    fn parse_pick_rejects_an_unknown_flag() {
        assert!(parse_args(&args(&["pick", "--nope"])).is_err());
    }

    #[test]
    fn parse_pick_dispatch_takes_each_action_and_leaves_the_address_to_the_context() {
        for (arg, action) in [
            ("open", RowAction::Open),
            ("fresh", RowAction::Fresh),
            ("stop", RowAction::Stop),
        ] {
            assert_eq!(
                parse_args(&args(&["pick", "--dispatch", arg])).unwrap(),
                Command::PickDispatch { action }
            );
        }
    }

    #[test]
    fn parse_pick_dispatch_refuses_an_unknown_action_an_address_a_flag_or_extras() {
        for argv in [
            &["pick", "--dispatch", "close"][..],
            &["pick", "--dispatch"],
            &["pick", "--dispatch", "stop", "home/garden"],
            &["pick", "--dispatch", "stop", "--render-once"],
            &["pick", "--dispatch", "stop", "home/garden", "home/attic"],
            &["pick", "--render-once", "--dispatch", "stop", "home/garden"],
        ] {
            assert!(parse_args(&args(argv)).is_err(), "{argv:?}");
        }
    }

    #[test]
    fn parse_rejects_an_unknown_flag() {
        assert!(parse_args(&args(&["resume", "--nope"])).is_err());
    }

    #[test]
    fn resolve_address_uses_the_given_one() {
        assert_eq!(
            resolve_address(Some("home/garden".to_string()), false).unwrap(),
            "home/garden"
        );
    }

    #[test]
    fn resolve_address_dry_run_without_address_refuses_without_calling_herdr() {
        let err = resolve_address(None, true).unwrap_err();
        assert!(matches!(err, EstateError::Usage(_)));
    }

    fn scratch_state() -> (crate::test_support::TempDir, crate::test_support::EnvGuard) {
        let d = crate::test_support::TempDir::new();
        let g = crate::test_support::EnvGuard::new(
            &[("HERDR_PLUGIN_STATE_DIR", d.path().to_str().unwrap())],
            &["HERDR_PLUGIN_ACTION_ID", "HERDR_PLUGIN_ENTRYPOINT_ID"],
        );
        (d, g)
    }

    #[test]
    fn run_missing_verb_exits_2() {
        let _s = scratch_state();
        assert_eq!(run(&[]), 2);
    }

    #[test]
    fn run_unknown_verb_exits_2_and_logs_the_run_and_the_error() {
        let _s = scratch_state();
        assert_eq!(run(&args(&["launch"])), 2);
        let logged = progress::logged_messages();
        assert_eq!(logged.len(), 2);
        assert_eq!(logged[0], "run: herdr-estate launch (shell)");
        assert!(logged[1].starts_with("error: "));
    }

    #[test]
    fn herdr_plugin_toml_version_matches_cargo_pkg_version() {
        let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml"));
        let manifest: toml::Table = toml::from_str(text).unwrap();
        assert_eq!(
            manifest["version"].as_str().unwrap(),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn run_logs_a_handled_error_before_exiting_1() {
        let d = crate::test_support::TempDir::new();
        let cfg = d.path().join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("estates.toml"), "not = [valid").unwrap();
        let state = d.path().join("state");
        let _g = crate::test_support::EnvGuard::new(
            &[
                ("HERDR_PLUGIN_STATE_DIR", state.to_str().unwrap()),
                ("HERDR_PLUGIN_CONFIG_DIR", cfg.to_str().unwrap()),
            ],
            &["HERDR_PLUGIN_ACTION_ID", "HERDR_PLUGIN_ENTRYPOINT_ID"],
        );
        assert_eq!(run(&args(&["list"])), 1);
        let logged = progress::logged_messages();
        assert_eq!(logged.len(), 2);
        assert_eq!(logged[0], "run: herdr-estate list (shell)");
        assert!(logged[1].starts_with("error: "));
    }
}
