//! resume, fresh and list: the three actions, and the herdr command
//! shapes behind them.
//!
//! Every workspace is created with `--label <address>`, `--no-focus`, and
//! `--env HERDR_ESTATE_ADDRESS=<address>` plus the estate's own env table.
//! Both `resume` and `fresh` close and recreate a live workspace under the
//! same address when it is ours (`ownership`) and not alive
//! (`doubleopen::workspace_alive`) — the label match herdr's ids being
//! reused across a server restart could otherwise misidentify is never
//! enough on its own: a workspace not in the ownership record is left
//! alone, and both verbs refuse instead of creating a second one under the
//! same label. Either verb refuses outright when the workspace is alive.
//! `--dry-run` runs every one of these read-only checks (they decide
//! whether the printed plan would create alone or close-then-create) and
//! only skips the herdr calls that actually create, close or start
//! something.

use crate::adapters;
use crate::campaign::{self, AgentKind, Campaign};
use crate::continuity;
use crate::doubleopen;
use crate::errors::EstateError;
use crate::herdrcli::{self, HerdrResult, Workspace, WorkspaceCreateResult, WorkspaceGetResult};
use crate::ownership;
use crate::prompt;
use crate::verb::Verb;

pub const AGENT_START_TIMEOUT_MS: &str = "30000";
pub const PANE_PLACEHOLDER: &str = "<PANE_ID>";

/// Emptied on every workspace create (`--env KEY=`), before the estate's
/// own env pairs so a user who deliberately sets one of these in their
/// estate table wins. The agent inherits the environment of the
/// workspace's root shell, which herdr's server spawns, not this
/// process's, so there is no `env -u` to reach for; these three names are
/// the ones that describe *this* process (an API key it authenticated
/// with, tmux markers for a pane the workspace did not create) rather
/// than the workspace about to exist.
pub const WORKSPACE_ENV_SCRUB: &[&str] = &["ANTHROPIC_API_KEY", "TMUX", "TMUX_PANE"];

fn is_agent_name_byte(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
}

pub fn agent_name(address: &str) -> String {
    let sanitized: String = address
        .to_lowercase()
        .chars()
        .map(|c| if is_agent_name_byte(c) { c } else { '-' })
        .collect();
    let trimmed = sanitized.trim_start_matches('-');
    let base = if trimmed.is_empty() {
        "estate".to_string()
    } else {
        trimmed.to_string()
    };
    let named = if base.chars().next().is_some_and(|c| c.is_alphabetic()) {
        base
    } else {
        format!("a{base}")
    };
    named.chars().take(32).collect()
}

/// `redact: true` is for display only (`--dry-run`): every estate-table
/// env value is masked to `KEY=<from estates.toml>` so a printed plan
/// never leaks a secret. `HERDR_ESTATE_ADDRESS` is derived, not from the
/// table, and always stays in the clear.
pub fn create_args(campaign: &Campaign, redact: bool) -> Vec<String> {
    let mut args = vec![
        "workspace".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        campaign.workdir.display().to_string(),
        "--label".to_string(),
        campaign.address.clone(),
        "--no-focus".to_string(),
        "--env".to_string(),
        format!("HERDR_ESTATE_ADDRESS={}", campaign.address),
    ];
    for k in WORKSPACE_ENV_SCRUB {
        args.push("--env".to_string());
        args.push(format!("{k}="));
    }
    for (k, v) in &campaign.env {
        let shown = if redact {
            "<from estates.toml>".to_string()
        } else {
            v.clone()
        };
        args.push("--env".to_string());
        args.push(format!("{k}={shown}"));
    }
    args
}

pub fn start_args(campaign: &Campaign, pane_id: &str, agent_argv: &[String]) -> Vec<String> {
    let mut args = vec![
        "agent".to_string(),
        "start".to_string(),
        agent_name(&campaign.address),
        "--kind".to_string(),
        campaign.kind.as_str().to_string(),
        "--pane".to_string(),
        pane_id.to_string(),
        "--timeout".to_string(),
        AGENT_START_TIMEOUT_MS.to_string(),
        "--".to_string(),
    ];
    args.extend(agent_argv.iter().cloned());
    args
}

pub fn derive_address() -> Result<String, EstateError> {
    let workspace_id = crate::paths::non_empty_env("HERDR_WORKSPACE_ID").ok_or_else(|| {
        EstateError::Usage(
            "no address given and $HERDR_WORKSPACE_ID is unset — pass one explicitly".to_string(),
        )
    })?;
    let result = herdrcli::call_herdr(
        &[
            "workspace".to_string(),
            "get".to_string(),
            workspace_id.clone(),
        ],
        true,
    )?;
    let parsed: WorkspaceGetResult = serde_json::from_value(result.data).unwrap_or_default();
    let label = parsed
        .workspace
        .and_then(|w| w.label)
        .filter(|s| !s.is_empty());
    label.ok_or_else(|| {
        EstateError::Usage(format!(
            "workspace {workspace_id} carries no label to derive an address from"
        ))
    })
}

/// The proven primitive: re-check the label immediately before closing,
/// and skip (never force) when it no longer matches.
pub fn close_if_ours(workspace_id: &str, address: &str) -> Result<(), EstateError> {
    let result = herdrcli::call_herdr(
        &[
            "workspace".to_string(),
            "get".to_string(),
            workspace_id.to_string(),
        ],
        false,
    )?;
    let label = if result.ok {
        serde_json::from_value::<WorkspaceGetResult>(result.data)
            .ok()
            .and_then(|r| r.workspace)
            .and_then(|w| w.label)
    } else {
        None
    };
    if label.as_deref() != Some(address) {
        return Ok(());
    }
    herdrcli::call_herdr(
        &[
            "workspace".to_string(),
            "close".to_string(),
            workspace_id.to_string(),
        ],
        true,
    )?;
    Ok(())
}

fn build_agent_argv(campaign: &Campaign, verb: Verb, conversation_id: &str) -> Vec<String> {
    let prompt_file = prompt::prompt_path(&campaign.estate, &campaign.name, &campaign.prompt_files);
    let argv = adapters::build_argv(campaign, verb, conversation_id, prompt_file.as_deref());
    // argv[0] is always the kind name, never part of the argv herdr forwards.
    argv.into_iter().skip(1).collect()
}

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || "@%+=:,./-".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

fn shlex_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shlex_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_plan(create_cmd: &[String], agent_cmd: &[String], close_cmd: Option<&[String]>) {
    if let Some(close) = close_cmd {
        println!("would close:");
        println!("  herdr {}", shlex_join(close));
    }
    println!("would create:");
    println!("  herdr {}", shlex_join(create_cmd));
    println!("would start:");
    println!("  herdr {}", shlex_join(agent_cmd));
}

/// Validate `workspace create`'s response before using it: a missing
/// workspace or pane id is a `ProtocolError` naming the field, never a
/// raw panic two calls later.
fn created_ids(created: &HerdrResult) -> Result<(String, String), EstateError> {
    let parsed: WorkspaceCreateResult =
        serde_json::from_value(created.data.clone()).unwrap_or_default();
    let workspace_id = parsed
        .workspace
        .and_then(|w| w.workspace_id)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            EstateError::Protocol(
                "workspace create: response missing workspace.workspace_id".to_string(),
            )
        })?;
    let pane_id = parsed
        .root_pane
        .and_then(|p| p.pane_id)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            EstateError::Protocol(
                "workspace create: response missing root_pane.pane_id".to_string(),
            )
        })?;
    Ok((workspace_id, pane_id))
}

/// The one read both `resume` and `fresh` need before deciding anything:
/// is a workspace already live under `address`, and if so is it safe to
/// replace. Returns the workspace row when it exists, is not alive
/// (`doubleopen::workspace_alive`) and is ours (`ownership::is_ours`) —
/// the case both verbs close and recreate over. Returns `None` when
/// nothing is live. Errors otherwise: the workspace is alive (refuse,
/// regardless of ownership), or it is not alive but is not ours (refuse;
/// nothing is closed, nothing is created). Runs the same way under
/// `--dry-run`, so a dry run's plan and refusals both match the real run;
/// only the mutating herdr calls further down are skipped.
fn check_live_workspace(address: &str) -> Result<Option<Workspace>, EstateError> {
    let ws = match doubleopen::live_workspace(address)? {
        Some(w) => w,
        None => return Ok(None),
    };
    if doubleopen::workspace_alive(&ws) {
        return Err(EstateError::Guard(format!(
            "{address}: already open in workspace {} with a running agent",
            ws.workspace_id
        )));
    }
    if !ownership::is_ours(&ws.workspace_id) {
        return Err(EstateError::Guard(format!(
            "{address}: a workspace with this label exists that herdr-estate did not create — close it yourself"
        )));
    }
    Ok(Some(ws))
}

/// Everything that can fail without herdr (prompt files exist) is
/// validated before `ws` (an owned, non-alive workspace) is closed.
/// Creates the replacement, validates the response, records ownership of
/// the new workspace, and starts the agent.
fn open_workspace(
    campaign: &Campaign,
    address: &str,
    ws: Option<Workspace>,
    create_cmd: &[String],
    agent_argv: &[String],
) -> Result<(String, String), EstateError> {
    if !campaign.prompt_files.is_empty() {
        prompt::write_prompt(&campaign.estate, &campaign.name, &campaign.prompt_files)?;
    }
    if let Some(ws) = ws {
        close_if_ours(&ws.workspace_id, address)?;
        ownership::forget(&ws.workspace_id).map_err(|e| {
            EstateError::Config(format!("could not update the ownership record: {e}"))
        })?;
    }
    let created = herdrcli::call_herdr(create_cmd, true)?;
    let (workspace_id, pane_id) = created_ids(&created)?;
    ownership::record(address, &workspace_id)
        .map_err(|e| EstateError::Config(format!("could not update the ownership record: {e}")))?;
    herdrcli::call_herdr(&start_args(campaign, &pane_id, agent_argv), true)?;
    Ok((workspace_id, pane_id))
}

/// A `/dev/urandom`-backed v4 UUID — the crate's dependency budget has no
/// room for the `uuid` crate.
fn new_conversation_id() -> Result<String, EstateError> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| EstateError::Protocol(format!("could not open /dev/urandom: {e}")))?;
    f.read_exact(&mut bytes)
        .map_err(|e| EstateError::Protocol(format!("could not read /dev/urandom: {e}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn resume(estates: &toml::Table, address: &str, dry_run: bool) -> Result<i32, EstateError> {
    let campaign = campaign::resolve(estates, address, dry_run)?;
    let conversation_id = continuity::latest_conversation(&campaign.dir);
    let conversation_id = match conversation_id {
        Some(id) => id,
        None => {
            println!("{address}: no recorded conversation — resume behaves as fresh");
            return fresh(estates, address, dry_run);
        }
    };

    if campaign.kind == AgentKind::Claude {
        if !doubleopen::transcript_exists(&conversation_id) {
            return Err(EstateError::Guard(format!(
                "{address}: conversation {}... is recorded but never persisted a transcript",
                short(&conversation_id)
            )));
        }
        if let Some(holder) = doubleopen::registry_holder(&conversation_id) {
            return Err(EstateError::Guard(format!(
                "{address}: conversation is open as {holder} — exit that pane first"
            )));
        }
    }

    let agent_argv = build_agent_argv(&campaign, Verb::Resume, &conversation_id);
    let create_cmd = create_args(&campaign, false);
    let start_cmd = start_args(&campaign, PANE_PLACEHOLDER, &agent_argv);

    let ws = check_live_workspace(address)?;

    if dry_run {
        let close_cmd = ws.as_ref().map(|w| {
            vec![
                "workspace".to_string(),
                "close".to_string(),
                w.workspace_id.clone(),
            ]
        });
        print_plan(
            &create_args(&campaign, true),
            &start_cmd,
            close_cmd.as_deref(),
        );
        return Ok(0);
    }

    let (workspace_id, pane_id) = open_workspace(&campaign, address, ws, &create_cmd, &agent_argv)?;
    println!(
        "live  {address}  {workspace_id}  {pane_id}  [{}...]",
        short(&conversation_id)
    );
    Ok(0)
}

pub fn fresh(estates: &toml::Table, address: &str, dry_run: bool) -> Result<i32, EstateError> {
    let campaign = campaign::resolve(estates, address, dry_run)?;
    let old_id = continuity::latest_conversation(&campaign.dir);
    if let Some(old_id) = &old_id {
        if campaign.kind == AgentKind::Claude {
            if let Some(holder) = doubleopen::registry_holder(old_id) {
                return Err(EstateError::Guard(format!(
                    "{address}: conversation is open as {holder} — exit that pane first"
                )));
            }
        }
    }

    let new_id = new_conversation_id()?;
    let agent_argv = build_agent_argv(&campaign, Verb::Fresh, &new_id);
    let create_cmd = create_args(&campaign, false);
    let start_cmd = start_args(&campaign, PANE_PLACEHOLDER, &agent_argv);

    let ws = check_live_workspace(address)?;

    if dry_run {
        let close_cmd = ws.as_ref().map(|w| {
            vec![
                "workspace".to_string(),
                "close".to_string(),
                w.workspace_id.clone(),
            ]
        });
        print_plan(
            &create_args(&campaign, true),
            &start_cmd,
            close_cmd.as_deref(),
        );
        return Ok(0);
    }

    let (workspace_id, pane_id) = open_workspace(&campaign, address, ws, &create_cmd, &agent_argv)?;
    continuity::append(&campaign.dir, &new_id)
        .map_err(|e| EstateError::Config(format!("could not append to the continuity log: {e}")))?;
    println!(
        "live  {address}  {workspace_id}  {pane_id}  [{}...]  (fresh)",
        short(&new_id)
    );
    Ok(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignRow {
    pub address: String,
    pub last: String,
}

pub fn list_campaigns(estates: &toml::Table) -> Vec<CampaignRow> {
    let mut rows = Vec::new();
    for (estate_name, value) in estates.iter() {
        let row = match value.as_table() {
            Some(t) => t,
            None => continue,
        };
        let root_val = match row
            .get("root")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(r) => r,
            None => continue,
        };
        let root = crate::paths::expand(root_val);
        let campaigns_sub = row
            .get("campaigns")
            .and_then(|v| v.as_str())
            .unwrap_or(crate::config::DEFAULT_CAMPAIGNS_DIR);
        let campaigns_dir = root.join(campaigns_sub);
        if !campaigns_dir.is_dir() {
            continue;
        }
        let mut names: Vec<String> = match std::fs::read_dir(&campaigns_dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.path().join("campaign.md").is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        for name in names {
            let cdir = campaigns_dir.join(&name);
            let address = format!("{estate_name}/{name}");
            let log = continuity::read(&cdir);
            let last = log
                .last()
                .and_then(|l| l.split_whitespace().next())
                .map(|s| s.chars().take(10).collect::<String>())
                .unwrap_or_default();
            rows.push(CampaignRow { address, last });
        }
    }
    rows
}

pub fn format_row(row: &CampaignRow) -> String {
    let state = if row.last.is_empty() {
        "new".to_string()
    } else {
        format!("last {}", row.last)
    };
    format!("{:<26} {state}", row.address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaign::AgentKind;
    use crate::test_support::{EnvGuard, FakeHerdr, TempDir};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn campaign_at(dir: &Path, kind: AgentKind) -> Campaign {
        Campaign {
            address: "home/garden".to_string(),
            estate: "home".to_string(),
            name: "garden".to_string(),
            dir: dir.to_path_buf(),
            workdir: dir.to_path_buf(),
            kind,
            add_dirs: Vec::new(),
            prompt_files: Vec::new(),
            kind_flags: BTreeMap::new(),
            env: BTreeMap::from([("K".to_string(), "V".to_string())]),
        }
    }

    // --- new_conversation_id ---

    #[test]
    fn new_conversation_id_is_shaped_like_a_v4_uuid() {
        let id = new_conversation_id().unwrap();
        assert_eq!(id.len(), 36);
        let bytes = id.as_bytes();
        assert_eq!(bytes[8], b'-');
        assert_eq!(bytes[13], b'-');
        assert_eq!(bytes[18], b'-');
        assert_eq!(bytes[23], b'-');
        assert_eq!(bytes[14], b'4');
        assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn new_conversation_id_two_calls_differ() {
        assert_ne!(
            new_conversation_id().unwrap(),
            new_conversation_id().unwrap()
        );
    }

    // --- agent_name ---

    #[test]
    fn agent_name_sanitizes_and_lowercases() {
        assert_eq!(agent_name("Home/Garden"), "home-garden");
    }

    #[test]
    fn agent_name_truncates_to_32() {
        let name = agent_name(&format!("home/{}", "x".repeat(40)));
        assert!(name.chars().count() <= 32);
    }

    #[test]
    fn agent_name_leading_digit_gets_prefixed() {
        assert!(agent_name("9lives/x")
            .chars()
            .next()
            .unwrap()
            .is_alphabetic());
    }

    // --- command shapes ---

    #[test]
    fn create_args_has_label_and_env() {
        let d = TempDir::new();
        let c = campaign_at(d.path(), AgentKind::Claude);
        let args = create_args(&c, false);
        assert!(args.contains(&"--label".to_string()));
        let idx = args.iter().position(|a| a == "--label").unwrap();
        assert_eq!(args[idx + 1], "home/garden");
        assert!(args.contains(&"HERDR_ESTATE_ADDRESS=home/garden".to_string()));
        assert!(args.contains(&"K=V".to_string()));
    }

    #[test]
    fn create_args_has_no_focus() {
        let d = TempDir::new();
        let c = campaign_at(d.path(), AgentKind::Claude);
        assert!(create_args(&c, false).contains(&"--no-focus".to_string()));
    }

    #[test]
    fn create_args_scrubs_workspace_env_before_estate_env() {
        let d = TempDir::new();
        let c = campaign_at(d.path(), AgentKind::Claude);
        let args = create_args(&c, false);
        let k_idx = args.iter().position(|a| a == "K=V").unwrap();
        for k in WORKSPACE_ENV_SCRUB {
            let flag = format!("{k}=");
            let idx = args.iter().position(|a| a == &flag).unwrap();
            assert!(idx < k_idx);
        }
    }

    #[test]
    fn create_args_redact_masks_estate_env_not_address() {
        let d = TempDir::new();
        let c = campaign_at(d.path(), AgentKind::Claude);
        let args = create_args(&c, true);
        assert!(args.contains(&"K=<from estates.toml>".to_string()));
        assert!(!args.contains(&"K=V".to_string()));
        assert!(args.contains(&"HERDR_ESTATE_ADDRESS=home/garden".to_string()));
    }

    #[test]
    fn start_args_shape() {
        let d = TempDir::new();
        let c = campaign_at(d.path(), AgentKind::Claude);
        let args = start_args(&c, "p1", &["--resume".to_string(), "sid".to_string()]);
        assert_eq!(&args[..2], &["agent".to_string(), "start".to_string()]);
        let kind_idx = args.iter().position(|a| a == "--kind").unwrap();
        assert_eq!(args[kind_idx + 1], "claude");
        let pane_idx = args.iter().position(|a| a == "--pane").unwrap();
        assert_eq!(args[pane_idx + 1], "p1");
        assert_eq!(
            &args[args.len() - 2..],
            &["--resume".to_string(), "sid".to_string()]
        );
    }

    // --- derive_address ---

    #[test]
    fn derive_address_missing_workspace_id_errors() {
        let _g = EnvGuard::unset(&["HERDR_WORKSPACE_ID"]);
        assert!(matches!(derive_address(), Err(EstateError::Usage(_))));
    }

    #[test]
    fn derive_address_reads_label_from_workspace_get() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[
            ("HERDR_WORKSPACE_ID", "w1"),
            ("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap()),
        ]);
        assert_eq!(derive_address().unwrap(), "home/garden");
        assert_eq!(fake.argv(0), vec!["workspace", "get", "w1"]);
    }

    // --- close_if_ours ---

    #[test]
    fn close_if_ours_closes_on_label_match() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden"}}}"#,
            0,
        );
        fake.set_response(1, r#"{"result": {}}"#, 0);
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        close_if_ours("w1", "home/garden").unwrap();
        assert_eq!(fake.call_count(), 2);
        assert_eq!(fake.argv(1), vec!["workspace", "close", "w1"]);
    }

    #[test]
    fn close_if_ours_skips_on_label_mismatch() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "someone-elses"}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        close_if_ours("w1", "home/garden").unwrap();
        assert_eq!(fake.call_count(), 1); // only the get, never the close
    }

    // --- created_ids ---

    #[test]
    fn created_ids_missing_workspace_id_errors() {
        let bad = HerdrResult {
            ok: true,
            returncode: 0,
            data: serde_json::json!({"root_pane": {"pane_id": "p1"}}),
            stderr: String::new(),
        };
        assert!(matches!(created_ids(&bad), Err(EstateError::Protocol(_))));
    }

    #[test]
    fn created_ids_missing_pane_id_errors() {
        let bad = HerdrResult {
            ok: true,
            returncode: 0,
            data: serde_json::json!({"workspace": {"workspace_id": "w1"}}),
            stderr: String::new(),
        };
        assert!(matches!(created_ids(&bad), Err(EstateError::Protocol(_))));
    }

    #[test]
    fn created_ids_valid_response_returns_both() {
        let ok = HerdrResult {
            ok: true,
            returncode: 0,
            data: serde_json::json!({
                "workspace": {"workspace_id": "w9", "label": "home/garden"},
                "root_pane": {"pane_id": "p1"},
            }),
            stderr: String::new(),
        };
        assert_eq!(
            created_ids(&ok).unwrap(),
            ("w9".to_string(), "p1".to_string())
        );
    }

    // --- integration-shaped helpers: full resume/fresh over a FakeHerdr ---

    struct Fixture {
        _tmp: TempDir,
        _env: EnvGuard,
        estates: toml::Table,
        cdir: PathBuf,
        fake: FakeHerdr,
    }

    fn make_fixture(kind: &str) -> Fixture {
        let tmp = TempDir::new();
        let root = tmp.path().join("home");
        let cdir = root.join("campaigns").join("garden");
        std::fs::create_dir_all(&cdir).unwrap();
        let fm = if kind == "claude" {
            "---\n---\n".to_string()
        } else {
            format!("---\nagent: {kind}\n---\n")
        };
        std::fs::write(cdir.join("campaign.md"), fm).unwrap();

        let fake = FakeHerdr::new();
        let state_dir = tmp.path().join("state");
        // A fresh HOME per fixture: `doubleopen::registry_holder` reads
        // `$HOME/.claude/sessions`, and `std::process::id()` is the same
        // for every test in this binary, so a shared HOME would leak a
        // session file one test wrote into every later test's view.
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
            estates,
            cdir,
            fake,
        }
    }

    const CREATE_OK: &str = r#"{"result": {
        "workspace": {"workspace_id": "w9", "label": "home/garden"},
        "root_pane": {"pane_id": "p1"}
    }}"#;

    fn empty_list() -> &'static str {
        r#"{"result": {"workspaces": []}}"#
    }

    #[test]
    fn resume_happy_path_creates_and_starts_no_log_write() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let before = continuity::read(&f.cdir);

        // claude-kind guard: a transcript must exist.
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();

        f.fake.set_response(0, empty_list(), 0); // live_workspace
        f.fake.set_response(1, CREATE_OK, 0); // workspace create
        f.fake.set_response(2, r#"{"result": {}}"#, 0); // agent start

        let rc = resume(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 3);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
        let start_argv = f.fake.argv(2);
        assert!(start_argv.iter().any(|a| a == "old-sid"));
        assert!(start_argv.iter().any(|a| a == "--resume"));
        assert_eq!(continuity::read(&f.cdir), before);
        assert_eq!(continuity::read(&f.cdir).len(), 1);
    }

    #[test]
    fn resume_with_no_recorded_conversation_falls_back_to_fresh() {
        let f = make_fixture("claude");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        let rc = resume(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(continuity::read(&f.cdir).len(), 1); // fresh appended one
    }

    #[test]
    fn resume_missing_transcript_refuses_and_calls_nothing() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn resume_registry_holder_refuses() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();
        let sessions = crate::paths::home_dir().join(".claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{}.json", std::process::id())),
            serde_json::json!({"sessionId": "old-sid", "name": "andrew"}).to_string(),
        )
        .unwrap();
        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn resume_running_workspace_refuses_and_calls_nothing_further() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1); // only the list, no close/create/start
    }

    #[test]
    fn resume_not_ours_workspace_refuses_and_creates_nothing() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "unowned-1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("did not create")));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn fresh_closes_matching_workspace_and_appends_log() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
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
        ); // close_if_ours get
        f.fake.set_response(2, r#"{"result": {}}"#, 0); // close
        f.fake.set_response(3, CREATE_OK, 0); // create
        f.fake.set_response(4, r#"{"result": {}}"#, 0); // start

        let rc = fresh(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 5);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
        let start_argv = f.fake.argv(4);
        assert!(start_argv.iter().any(|a| a == "--session-id"));
        let lines = continuity::read(&f.cdir);
        assert_eq!(lines.len(), 2);
        assert_ne!(
            lines.last().unwrap().split_whitespace().last(),
            Some("old-sid")
        );
        assert!(!ownership::is_ours("w1"));
        assert!(ownership::is_ours("w9"));
    }

    #[test]
    fn fresh_refuses_on_a_missing_prompt_file_before_closing_anything() {
        let mut f = make_fixture("claude");
        if let Some(toml::Value::Table(row)) = f.estates.get_mut("home") {
            row.insert(
                "prompts".to_string(),
                toml::Value::Array(vec![toml::Value::String("prompts/missing.md".to_string())]),
            );
        }
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        assert!(matches!(
            fresh(&f.estates, "home/garden", false),
            Err(EstateError::Config(_))
        ));
        // Only the read that found the dormant workspace; nothing closed.
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn fresh_with_no_prior_workspace_skips_close() {
        let f = make_fixture("claude");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        let rc = fresh(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 3); // list + create + start only
    }

    #[test]
    fn fresh_old_conversation_open_elsewhere_refuses() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let sessions = crate::paths::home_dir().join(".claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{}.json", std::process::id())),
            serde_json::json!({"sessionId": "old-sid", "name": "andrew"}).to_string(),
        )
        .unwrap();
        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn fresh_running_workspace_refuses_and_does_not_close() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn fresh_not_ours_workspace_refuses() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "unowned-1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("did not create")));
        assert_eq!(f.fake.call_count(), 1);
    }

    // --- kind-gated guards: the two Claude-only checks run only when
    // campaign.kind == claude. A non-claude campaign with no ~/.claude
    // state at all would fail those checks if they ran, so a plain
    // success here is proof they were skipped.

    #[test]
    fn resume_skips_claude_checks_for_other_kind() {
        let f = make_fixture("codex");
        continuity::append(&f.cdir, "old-sid").unwrap();
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        let rc = resume(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn fresh_skips_claude_checks_for_other_kind() {
        let f = make_fixture("codex");
        continuity::append(&f.cdir, "old-sid").unwrap();
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        let rc = fresh(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
    }

    // --- dry run ---

    #[test]
    fn resume_dry_run_makes_only_the_read_only_list_call() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();
        f.fake.set_response(0, empty_list(), 0);
        let rc = resume(&f.estates, "home/garden", true).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 1);
        assert_eq!(f.fake.argv(0), vec!["workspace", "list"]);
    }

    // The dry-run "would close:" line is covered in tests/cli.rs, against
    // stdout of the built binary.

    #[test]
    fn dry_run_refuses_when_the_live_workspace_is_alive() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "working"}
            ]}}"#,
            0,
        );
        assert!(matches!(
            fresh(&f.estates, "home/garden", true),
            Err(EstateError::Guard(_))
        ));
    }

    #[test]
    fn dry_run_refuses_when_the_live_workspace_is_not_ours() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "unowned-1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        assert!(matches!(
            resume(&f.estates, "home/garden", true),
            Err(EstateError::Guard(_))
        ));
    }

    // --- list_campaigns / format_row ---

    #[test]
    fn list_campaigns_lists_new_and_dormant() {
        let d = TempDir::new();
        let root = d.path().join("home");
        let garden = root.join("campaigns").join("garden");
        let taxes = root.join("campaigns").join("taxes");
        std::fs::create_dir_all(&garden).unwrap();
        std::fs::create_dir_all(&taxes).unwrap();
        std::fs::write(garden.join("campaign.md"), "---\n---\n").unwrap();
        std::fs::write(taxes.join("campaign.md"), "---\n---\n").unwrap();
        continuity::append(&garden, "sid").unwrap();

        let mut row = toml::Table::new();
        row.insert(
            "root".to_string(),
            toml::Value::String(root.to_string_lossy().to_string()),
        );
        let mut estates = toml::Table::new();
        estates.insert("home".to_string(), toml::Value::Table(row));

        let rows = list_campaigns(&estates);
        let addresses: Vec<&str> = rows.iter().map(|r| r.address.as_str()).collect();
        assert!(addresses.contains(&"home/garden"));
        assert!(addresses.contains(&"home/taxes"));
        let garden_row = rows.iter().find(|r| r.address == "home/garden").unwrap();
        let taxes_row = rows.iter().find(|r| r.address == "home/taxes").unwrap();
        assert!(!garden_row.last.is_empty());
        assert!(taxes_row.last.is_empty());
    }

    #[test]
    fn format_row_shows_new_or_last() {
        let new_row = CampaignRow {
            address: "home/x".to_string(),
            last: String::new(),
        };
        assert!(format_row(&new_row).ends_with("new"));
        let dormant = CampaignRow {
            address: "home/y".to_string(),
            last: "2026-09-21".to_string(),
        };
        assert!(format_row(&dormant).contains("last 2026-09-21"));
    }
}
