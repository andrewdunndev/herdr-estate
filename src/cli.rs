//! bin/herdr-estate's argument parsing and dispatch.

use crate::config;
use crate::errors::EstateError;
use crate::verbs;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

/// A small hand-rolled parser matching the shape of the Python argparse
/// setup: one required subcommand (`resume`, `fresh` or `list`), an
/// optional positional `address` on the two verbs, and `--dry-run` on
/// those same two. Anything else is a usage error, exit code 2 (the
/// exit code Python's argparse itself uses on a parse failure).
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
        "list" => {
            if !rest.is_empty() {
                return Err(ParseError {
                    message: format!("unrecognized arguments: {}", rest.join(" ")),
                });
            }
            Ok(Command::List)
        }
        other => Err(ParseError {
            message: format!(
                "argument verb: invalid choice: {other:?} (choose from 'resume', 'fresh', 'list')"
            ),
        }),
    }
}

/// `address` when given; otherwise the label of the workspace
/// `$HERDR_WORKSPACE_ID` names (a herdr call), unless `dry_run` is set,
/// since a dry run must never itself call herdr just to find out what it
/// would have printed.
fn resolve_address(address: Option<String>, dry_run: bool) -> Result<String, EstateError> {
    if let Some(a) = address {
        return Ok(a);
    }
    if dry_run {
        return Err(EstateError::Usage(
            "--dry-run needs an explicit <address> — deriving one from the current workspace would itself call herdr"
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
            verbs::resume(&estates, &addr, dry_run)
        }
        Command::Fresh { address, dry_run } => {
            let estates = config::load_estates(None)?;
            let addr = resolve_address(address, dry_run)?;
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
    }
}

/// Parse `argv` (excluding the program name) and run the named verb,
/// returning the process exit code: 0 on success, 1 on a handled
/// `EstateError` (printed as `herdr-estate: <message>` to stderr), 2 on
/// a usage error in the arguments themselves.
pub fn run(argv: &[String]) -> i32 {
    let cmd = match parse_args(argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("herdr-estate: error: {}", e.message);
            return 2;
        }
    };
    match dispatch(cmd) {
        Ok(rc) => rc,
        Err(e) => {
            eprintln!("herdr-estate: {e}");
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
    fn parse_rejects_a_second_positional() {
        assert!(parse_args(&args(&["resume", "home/garden", "home/taxes"])).is_err());
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

    #[test]
    fn run_missing_verb_exits_2() {
        assert_eq!(run(&[]), 2);
    }

    #[test]
    fn run_unknown_verb_exits_2() {
        assert_eq!(run(&args(&["launch"])), 2);
    }
}
