//! resume, fresh, list and create: the verbs, and the herdr command
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
//! same label. Either verb refuses outright when the workspace is alive,
//! or when more than one workspace already carries the address
//! (`doubleopen::live_workspace`). The close itself is re-confirmed
//! immediately before it runs, and again treated as unsafe to act past
//! when that confirmation fails or herdr reports the close itself
//! failed: the verb stops right there rather than creating a replacement
//! over a workspace that might still be there.
//! `--dry-run` runs every one of these read-only checks (they decide
//! whether the printed plan would create alone or close-then-create) and
//! only skips the herdr calls that actually create, close or start
//! something.

use std::path::Path;

use crate::adapters;
use crate::campaign::{self, AgentKind, Campaign};
use crate::config::parse_address;
use crate::continuity;
use crate::doubleopen;
use crate::errors::EstateError;
use crate::herdrcli::{
    self, AgentStatus, HerdrResult, Workspace, WorkspaceCreateResult, WorkspaceGetResult,
    WorkspaceRef,
};
use crate::lock;
use crate::name::{self, agent_name};
use crate::ownership;
use crate::progress;
use crate::prompt;
use crate::verb::Verb;

pub const AGENT_START_TIMEOUT_MS: &str = "30000";

/// How long an estate's `prepare` may run before the launch refuses:
/// twice the agent start's allowance, since a prepare may fetch or
/// build before the agent ever starts.
pub const PREPARE_TIMEOUT_SECS: u64 = 60;

/// How long, once `prepare` has exited, its output is waited on: a
/// process it left running in the background can hold the pipes open
/// for as long as it runs.
const PREPARE_PIPE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
pub const PANE_PLACEHOLDER: &str = "<PANE_ID>";

/// Why `open_workspace` closes the workspace it replaces, which sets
/// both its progress line and what the close re-checks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseMode {
    /// `resume`/`fresh`'s ordinary path: the workspace is dormant, its
    /// agent already gone.
    Exited,
    /// The picker's Ctrl-f second press: the agent is still attached,
    /// idle or done, and this close is what stops it.
    Stopping,
}

impl CloseMode {
    /// The closing progress line's reason.
    fn reason(self) -> &'static str {
        match self {
            CloseMode::Exited => "which herdr-estate opened and whose agent has exited",
            CloseMode::Stopping => "stopping its agent to start a fresh conversation",
        }
    }

    /// Only a stop requires the agent to still be idle or done at the
    /// close; the ordinary resume/fresh close never required that to
    /// begin with.
    fn requires_idle_or_done(self) -> bool {
        self == CloseMode::Stopping
    }
}

/// Emptied on every workspace create (`--env KEY=`), before the estate's
/// own env pairs so a user who deliberately sets one of these in their
/// estate table wins. The agent inherits the environment of the
/// workspace's root shell, which herdr's server spawns, not this
/// process's, so there is no `env -u` to reach for; these three names are
/// the ones that describe *this* process (an API key it authenticated
/// with, tmux markers for a pane the workspace did not create) rather
/// than the workspace about to exist.
pub const WORKSPACE_ENV_SCRUB: &[&str] = &["ANTHROPIC_API_KEY", "TMUX", "TMUX_PANE"];

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
            herdrcli::ENV_VALUE_MASK.to_string()
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
            "no address given and $HERDR_WORKSPACE_ID is unset. Pass one explicitly".to_string(),
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

/// Whether `close_if_ours` actually closed the workspace, or could not
/// confirm it was safe to. `NotConfirmed` covers every cause the caller
/// treats the same way: the pre-close `workspace get` itself failed, the
/// label had changed since the `workspace list` scan that found this row,
/// or herdr reported the close call itself failed. None of these justify
/// forcing the close, and none of them justify creating a replacement
/// over a workspace that might still be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    NotConfirmed,
}

/// Whether `workspace_id`'s label is still `address`, via a fresh
/// `workspace get` -- and, when it is, the same fresh read's
/// `agent_status` alongside it, for `close_if_ours` to re-check too.
/// Read-only (`check: false`), so it is safe to run under `--dry-run`
/// too. A failed get (herdr reported an error, or the response carried
/// no label) counts as "no longer matches" — there is nothing here to
/// trust either way.
fn confirm_label_matches(
    workspace_id: &str,
    address: &str,
) -> Result<Option<WorkspaceRef>, EstateError> {
    let result = herdrcli::call_herdr(
        &[
            "workspace".to_string(),
            "get".to_string(),
            workspace_id.to_string(),
        ],
        false,
    )?;
    let ws = if result.ok {
        serde_json::from_value::<WorkspaceGetResult>(result.data)
            .ok()
            .and_then(|r| r.workspace)
    } else {
        None
    };
    Ok(ws.filter(|w| w.label.as_deref() == Some(address)))
}

/// The proven primitive: re-check the label immediately before closing
/// (herdr's ids are reused across a server restart, so the match a
/// `workspace list` scan saw a moment ago is not proof enough here), then
/// close. When `require_idle_or_done` is set (the picker's Ctrl-f second
/// press only), the same fresh read's `agent_status` must still be idle
/// or done too, and its `pane_count` must still be exactly one --
/// narrowing, as far as a single extra read can, the window between
/// `check_replaceable_running_workspace`'s read and the close call
/// itself. Never forces anything: a label that no longer matches, a
/// status that flipped away from idle or done, a pane count that is no
/// longer exactly one (absent included), or a close herdr itself
/// reports as failed, all come back `NotConfirmed` for the caller to
/// refuse on rather than a raw close.
pub fn close_if_ours(
    workspace_id: &str,
    address: &str,
    require_idle_or_done: bool,
) -> Result<CloseOutcome, EstateError> {
    let Some(current) = confirm_label_matches(workspace_id, address)? else {
        return Ok(CloseOutcome::NotConfirmed);
    };
    if require_idle_or_done {
        if !matches!(
            current.agent_status,
            Some(AgentStatus::Idle) | Some(AgentStatus::Done)
        ) {
            return Ok(CloseOutcome::NotConfirmed);
        }
        if current.pane_count != Some(1) {
            return Ok(CloseOutcome::NotConfirmed);
        }
    }
    let closed = herdrcli::call_herdr(
        &[
            "workspace".to_string(),
            "close".to_string(),
            workspace_id.to_string(),
        ],
        false,
    )?;
    Ok(if closed.ok {
        CloseOutcome::Closed
    } else {
        CloseOutcome::NotConfirmed
    })
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

fn print_plan(
    missing_add_dirs: &[std::path::PathBuf],
    prepare_cmd: Option<&[String]>,
    create_cmd: &[String],
    agent_cmd: &[String],
    close_cmd: Option<&[String]>,
) {
    if !missing_add_dirs.is_empty() {
        println!("would skip, not there:");
        for dir in missing_add_dirs {
            println!("  add-dir {}", dir.display());
        }
    }
    if let Some(prepare) = prepare_cmd {
        println!("would prepare:");
        println!("  {}", shlex_join(prepare));
    }
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
/// workspace or pane id is `EstateError::Protocol` naming the field,
/// never a raw panic two calls later.
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
        return Err(refuse_not_ours(address));
    }
    Ok(Some(ws))
}

/// The refusal both guards give a live workspace under `address` that
/// the ownership record doesn't name.
fn refuse_not_ours(address: &str) -> EstateError {
    EstateError::Guard(format!(
        "{address}: a workspace with this label exists that herdr-estate did not create. Close it yourself"
    ))
}

/// The picker's Ctrl-f second press only: a live, owned workspace whose
/// freshly re-read `agent_status` is exactly idle or done, and that
/// carries exactly one pane, can be replaced too, closed and recreated
/// the same way a dormant one is. Everything else refuses: no live
/// workspace at all, one herdr-estate did not create (same message and
/// guard `check_live_workspace` uses), an agent that is working,
/// blocked, or any other status that isn't idle or done, or a pane
/// count that isn't exactly one -- `agent_status` is a single aggregate
/// over every pane in the workspace, so a second, working pane can
/// still be idle-or-done on that aggregate (herdr ranks an idle,
/// unseen pane above a working one), and a close tears down every
/// pane, not just the one this row names. A listing that omits
/// `pane_count` is not a known single pane either, so it refuses the
/// same as a count above one. Never called from the CLI path, which
/// keeps refusing a running agent through `check_live_workspace`
/// unchanged.
fn check_replaceable_running_workspace(address: &str) -> Result<Workspace, EstateError> {
    let ws = doubleopen::live_workspace(address)?
        .ok_or_else(|| EstateError::Guard(format!("{address}: no running agent to stop")))?;
    if !ownership::is_ours(&ws.workspace_id) {
        return Err(refuse_not_ours(address));
    }
    match ws.pane_count {
        Some(1) => {}
        Some(count) => {
            return Err(EstateError::Guard(format!(
                "{address}: workspace {} carries {count} panes, not just this one; refusing to stop it",
                ws.workspace_id
            )));
        }
        None => {
            return Err(EstateError::Guard(format!(
                "{address}: workspace {} carries no pane count to confirm it is just this one; refusing to stop it",
                ws.workspace_id
            )));
        }
    }
    match &ws.agent_status {
        Some(AgentStatus::Idle) | Some(AgentStatus::Done) => Ok(ws),
        _ => Err(EstateError::Guard(format!(
            "{address}: workspace {} is not idle or done, refusing to stop it",
            ws.workspace_id
        ))),
    }
}

/// The estate's `prepare` command, argv form, run with the campaign's
/// workdir as cwd, stdin closed, stdout and stderr captured. A spawn
/// error (a missing binary, say), a non-zero exit, or a run past
/// `PREPARE_TIMEOUT_SECS` (killed, then refused) all refuse the same
/// way -- a guard error naming `address`, so the caller's `?` stops
/// right here and never reaches the close or create that would
/// otherwise follow. Success logs one progress step; the command's own
/// stdout and stderr are otherwise discarded.
fn run_prepare(campaign: &Campaign, address: &str, argv: &[String]) -> Result<(), EstateError> {
    run_prepare_within(
        campaign,
        address,
        argv,
        std::time::Duration::from_secs(PREPARE_TIMEOUT_SECS),
    )
}

/// `run_prepare` with its deadline as a parameter, for the tests. Both
/// pipes are drained on their own threads while this waits, so a
/// prepare that writes more than a pipe holds never stalls on it. Only
/// the command itself is killed at the deadline; a process it started
/// and left running is its own. Such a process can hold the pipes open
/// after the command exits, so their output is waited on for at most
/// `PREPARE_PIPE_GRACE` (never past the deadline) and reads as empty
/// after that, which the progress line or refusal says.
fn run_prepare_within(
    campaign: &Campaign,
    address: &str,
    argv: &[String],
    timeout: std::time::Duration,
) -> Result<(), EstateError> {
    use std::io::Read;
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&campaign.workdir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            EstateError::Guard(format!(
                "{address}: prepare failed to run {} in {}: {e}",
                argv[0],
                campaign.workdir.display()
            ))
        })?;
    let (sent, drained) = std::sync::mpsc::channel::<(usize, Vec<u8>)>();
    let drain = |which: usize, pipe: Option<Box<dyn Read + Send>>| {
        let sent = sent.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = sent.send((which, bytes));
        });
    };
    drain(
        0,
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    drain(
        1,
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = std::time::Instant::now();
    let status = loop {
        let waited = child.try_wait().map_err(|e| {
            EstateError::Guard(format!("{address}: prepare could not be waited on: {e}"))
        })?;
        if let Some(status) = waited {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EstateError::Guard(format!(
                "{address}: prepare timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let grace = PREPARE_PIPE_GRACE.min(timeout.saturating_sub(started.elapsed()));
    let grace_ends = std::time::Instant::now() + grace;
    let mut pipes: [Option<Vec<u8>>; 2] = [None, None];
    while pipes.iter().any(Option::is_none) {
        let left = grace_ends.saturating_duration_since(std::time::Instant::now());
        match drained.recv_timeout(left) {
            Ok((which, bytes)) => pipes[which] = Some(bytes),
            Err(_) => break,
        }
    }
    let held_open = pipes.iter().any(Option::is_none);
    let [stdout, stderr] = pipes;
    let output = std::process::Output {
        status,
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    };
    let held_note = if held_open {
        " (its output held open by a process it left running)"
    } else {
        ""
    };
    if !output.status.success() {
        return Err(EstateError::Guard(format!(
            "{address}: prepare failed: {}{held_note}",
            prepare_failure_detail(&output)
        )));
    }
    progress::step(&format!("{address}: prepare ok{held_note}"));
    Ok(())
}

/// The first 3 non-empty lines of `output`'s stderr, or its stdout when
/// stderr carried none, or the exit status when neither did -- whichever
/// says the most about why `prepare` failed.
fn prepare_failure_detail(output: &std::process::Output) -> String {
    let lines_of = |bytes: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(3)
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let stderr_lines = lines_of(&output.stderr);
    if !stderr_lines.is_empty() {
        return stderr_lines.join(" | ");
    }
    let stdout_lines = lines_of(&output.stdout);
    if !stdout_lines.is_empty() {
        return stdout_lines.join(" | ");
    }
    output.status.to_string()
}

/// Runs once every guard has passed: creates the campaign's worktree,
/// if it names one (`campaign::materialise`), and builds the create and
/// agent commands from the result, so a refused launch never leaves a
/// worktree behind. Everything else that can fail without herdr
/// (prompt files exist, the estate's `prepare` command, run in that
/// workdir) is validated before `ws` (an owned
/// workspace, either non-alive or the idle/done one
/// `check_replaceable_running_workspace` cleared) is closed with
/// `close_mode`'s reason in the progress line. Creates the
/// replacement, validates the response, records ownership of the new
/// workspace, and starts the agent. When `ws` is present but its close
/// could not be confirmed, stops here with a guard error naming the
/// workspace: nothing is created, no agent is started. A failed
/// `prepare` stops even earlier, before that close: neither it nor the
/// create that would follow it ever runs.
fn open_workspace(
    resolved: &Campaign,
    address: &str,
    ws: Option<Workspace>,
    close_mode: CloseMode,
    verb: Verb,
    conversation_id: &str,
) -> Result<(String, String), EstateError> {
    let campaign = &campaign::materialise(resolved)?;
    let create_cmd = &create_args(campaign, false);
    let agent_argv = &build_agent_argv(campaign, verb, conversation_id);
    if !campaign.prompt_files.is_empty() {
        prompt::write_prompt(&campaign.estate, &campaign.name, &campaign.prompt_files)?;
    }
    if let Some(argv) = &campaign.prepare {
        run_prepare(campaign, address, argv)?;
    }
    for dir in &campaign.missing_add_dirs {
        progress::step(&format!(
            "{address}: add-dir {} does not exist, skipping",
            dir.display()
        ));
    }
    if let Some(ws) = ws {
        progress::step(&format!(
            "{address}: closing {}, {}",
            ws.workspace_id,
            close_mode.reason()
        ));
        match close_if_ours(
            &ws.workspace_id,
            address,
            close_mode.requires_idle_or_done(),
        )? {
            CloseOutcome::Closed => {
                ownership::forget(&ws.workspace_id).map_err(|e| {
                    EstateError::Config(format!("could not update the ownership record: {e}"))
                })?;
            }
            CloseOutcome::NotConfirmed => {
                return Err(EstateError::Guard(format!(
                    "{address}: workspace {} could not be confirmed closed, refusing to create a replacement",
                    ws.workspace_id
                )));
            }
        }
    }
    progress::step(&format!("{address}: creating a workspace"));
    let created = herdrcli::call_herdr(create_cmd, true)?;
    let (workspace_id, pane_id) = created_ids(&created)?;
    ownership::record(address, &workspace_id)
        .map_err(|e| EstateError::Config(format!("could not update the ownership record: {e}")))?;
    // `agent start` returns only once herdr sees the agent ready for
    // input: several seconds for claude, the whole wait the picker popup
    // otherwise sat blank through.
    let kind = campaign.kind.as_str();
    progress::step(&format!(
        "{address}: starting {kind} in {workspace_id}, waiting up to {}s for it to be ready",
        agent_start_timeout_secs()
    ));
    herdrcli::call_herdr(&start_args(campaign, &pane_id, agent_argv), true)?;
    progress::step(&format!("{address}: {kind} is ready in {workspace_id}"));
    Ok((workspace_id, pane_id))
}

/// `AGENT_START_TIMEOUT_MS` in whole seconds, for the progress line that
/// says how long the start may take.
pub(crate) fn agent_start_timeout_secs() -> u64 {
    AGENT_START_TIMEOUT_MS.parse::<u64>().unwrap_or(0) / 1000
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

/// The `--dry-run` plan: prints what the real run would do, without
/// making the calls that create, close or start anything. When a live
/// workspace was found, its close is confirmed the same read-only way the
/// real run would confirm it — dry run can know this much in advance,
/// and refuses the same way the real run would. It cannot know whether a
/// future close call itself would fail, since running one is the one
/// thing dry run never does; for that case alone the "would close" line
/// stands as printed, with nothing extra said about it.
fn dry_run_plan(
    campaign: &Campaign,
    address: &str,
    ws: Option<&Workspace>,
    prepare_cmd: Option<&[String]>,
    create_cmd_redacted: &[String],
    start_cmd: &[String],
) -> Result<i32, EstateError> {
    let close_cmd = match ws {
        None => None,
        Some(w) => {
            if confirm_label_matches(&w.workspace_id, address)?.is_some() {
                Some(vec![
                    "workspace".to_string(),
                    "close".to_string(),
                    w.workspace_id.clone(),
                ])
            } else {
                return Err(EstateError::Guard(format!(
                    "{address}: workspace {} could not be confirmed closed, refusing to create a replacement",
                    w.workspace_id
                )));
            }
        }
    };
    if campaign.worktree.is_some() {
        println!(
            "{address}: would ensure worktree: {}",
            campaign.workdir.display()
        );
    }
    print_plan(
        &campaign.missing_add_dirs,
        prepare_cmd,
        create_cmd_redacted,
        start_cmd,
        close_cmd.as_deref(),
    );
    Ok(0)
}

/// What a non-dry-run `resume`/`fresh` produced: the exit code the CLI
/// prints and returns, plus the workspace id it opened (`None` on a dry
/// run, which opens nothing). `pub(crate)`: the picker's `dispatch`
/// reads `workspace_id` to focus the workspace these verbs just opened,
/// through the exact same guarded call — never a second, parallel path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerbResult {
    pub exit_code: i32,
    pub workspace_id: Option<String>,
}

pub fn resume(estates: &toml::Table, address: &str, dry_run: bool) -> Result<i32, EstateError> {
    resume_with_ids(estates, address, dry_run).map(|r| r.exit_code)
}

pub(crate) fn resume_with_ids(
    estates: &toml::Table,
    address: &str,
    dry_run: bool,
) -> Result<VerbResult, EstateError> {
    let campaign = campaign::resolve(estates, address)?;
    let _lock = (!dry_run).then(|| lock::acquire(address)).transpose()?;
    let conversation_id = continuity::latest_conversation(&campaign.dir);
    let conversation_id = match conversation_id {
        Some(id) => id,
        None => {
            println!("{address}: no recorded conversation: resume behaves as fresh");
            return fresh_resolved(&campaign, address, dry_run);
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
                "{address}: conversation is open as {holder}. Exit that pane first"
            )));
        }
    }

    let ws = check_live_workspace(address)?;

    if dry_run {
        let agent_argv = build_agent_argv(&campaign, Verb::Resume, &conversation_id);
        let start_cmd = start_args(&campaign, PANE_PLACEHOLDER, &agent_argv);
        let exit_code = dry_run_plan(
            &campaign,
            address,
            ws.as_ref(),
            campaign.prepare.as_deref(),
            &create_args(&campaign, true),
            &start_cmd,
        )?;
        return Ok(VerbResult {
            exit_code,
            workspace_id: None,
        });
    }

    progress::step(&format!(
        "{address}: resuming conversation {}...",
        short(&conversation_id)
    ));
    let (workspace_id, pane_id) = open_workspace(
        &campaign,
        address,
        ws,
        CloseMode::Exited,
        Verb::Resume,
        &conversation_id,
    )?;
    println!(
        "live  {address}  {workspace_id}  {pane_id}  [{}...]",
        short(&conversation_id)
    );
    Ok(VerbResult {
        exit_code: 0,
        workspace_id: Some(workspace_id),
    })
}

pub fn fresh(estates: &toml::Table, address: &str, dry_run: bool) -> Result<i32, EstateError> {
    fresh_with_ids(estates, address, dry_run).map(|r| r.exit_code)
}

pub(crate) fn fresh_with_ids(
    estates: &toml::Table,
    address: &str,
    dry_run: bool,
) -> Result<VerbResult, EstateError> {
    let campaign = campaign::resolve(estates, address)?;
    let _lock = (!dry_run).then(|| lock::acquire(address)).transpose()?;
    fresh_resolved(&campaign, address, dry_run)
}

/// `fresh` from an already resolved campaign: `fresh_with_ids`, and
/// `resume` falling back to fresh with the campaign it resolved.
fn fresh_resolved(
    campaign: &Campaign,
    address: &str,
    dry_run: bool,
) -> Result<VerbResult, EstateError> {
    let old_id = fresh_registry_guard(campaign, address)?;
    let ws = check_live_workspace(address)?;
    fresh_open(campaign, address, old_id, ws, CloseMode::Exited, dry_run)
}

/// The picker's Ctrl-f second press: an idle or done agent, still
/// attached, is stopped and a fresh conversation started in its place.
/// `check_replaceable_running_workspace` owns the guard that keeps this
/// refusing everything else (no live workspace, not ours, working,
/// blocked, any other status, more than one pane); nothing here repeats
/// or loosens it. `stop_registry_guard` restores `fresh_registry_guard`'s
/// intent without its bug -- its own comment says how. The CLI's `fresh`
/// never calls this, only `fresh_with_ids` above, which keeps refusing a
/// running agent exactly as before.
pub(crate) fn fresh_with_ids_stop_running(
    estates: &toml::Table,
    address: &str,
) -> Result<VerbResult, EstateError> {
    let campaign = campaign::resolve(estates, address)?;
    let _lock = lock::acquire(address)?;
    let old_id = continuity::latest_conversation(&campaign.dir);
    stop_registry_guard(&campaign, address, &old_id)?;
    let ws = check_replaceable_running_workspace(address)?;
    fresh_open(
        &campaign,
        address,
        old_id,
        Some(ws),
        CloseMode::Stopping,
        false,
    )
}

/// The claude-kind double-open guard `fresh_with_ids` runs first, before
/// it reads a live workspace: the conversation about to be retired must
/// not already be open in some other pane. `fresh_with_ids_stop_running`
/// does not call this: the conversation it retires is, by definition,
/// attached and live right now (that is what idle or done means), so
/// its own pid is always a registry holder of itself, and this guard
/// would always refuse. It calls `stop_registry_guard` instead, which
/// asks the same question the right way for that path -- is the
/// conversation open in some OTHER pane too, not just this one. Returns
/// the conversation id to retire, if any, for the progress line and the
/// close-reason wording further down to use.
fn fresh_registry_guard(campaign: &Campaign, address: &str) -> Result<Option<String>, EstateError> {
    let old_id = continuity::latest_conversation(&campaign.dir);
    if let Some(old_id) = &old_id {
        if campaign.kind == AgentKind::Claude {
            if let Some(holder) = doubleopen::registry_holder(old_id) {
                return Err(EstateError::Guard(format!(
                    "{address}: conversation is open as {holder}. Exit that pane first"
                )));
            }
        }
    }
    Ok(old_id)
}

/// The stop path's own double-open question: not whether the
/// conversation being retired is open anywhere (it always is, in the
/// very pane calling this), but whether it is open in more than one
/// live pane. No new herdr read -- the same registry
/// `doubleopen::registry_holder` reads, just counted rather than
/// stopped at the first match. More than one live holder refuses before
/// `check_replaceable_running_workspace` ever runs, let alone a close;
/// exactly one (itself) or zero proceeds.
fn stop_registry_guard(
    campaign: &Campaign,
    address: &str,
    old_id: &Option<String>,
) -> Result<(), EstateError> {
    if campaign.kind != AgentKind::Claude {
        return Ok(());
    }
    let Some(old_id) = old_id else {
        return Ok(());
    };
    let holders = doubleopen::live_registry_holders(old_id);
    if holders.len() > 1 {
        return Err(EstateError::Guard(format!(
            "{address}: conversation is open as {}. Exit those panes first",
            holders.join(", ")
        )));
    }
    Ok(())
}

/// The second half both `fresh_with_ids` and `fresh_with_ids_stop_running`
/// share once they have decided `ws`: build the new conversation, print
/// or run the plan, close `ws` (with `close_mode`'s reason in the progress
/// line) and recreate. `dry_run` is always `false` from the picker path,
/// which has no dry-run concept of its own.
fn fresh_open(
    campaign: &Campaign,
    address: &str,
    old_id: Option<String>,
    ws: Option<Workspace>,
    close_mode: CloseMode,
    dry_run: bool,
) -> Result<VerbResult, EstateError> {
    let new_id = new_conversation_id()?;

    if dry_run {
        let agent_argv = build_agent_argv(campaign, Verb::Fresh, &new_id);
        let start_cmd = start_args(campaign, PANE_PLACEHOLDER, &agent_argv);
        let exit_code = dry_run_plan(
            campaign,
            address,
            ws.as_ref(),
            campaign.prepare.as_deref(),
            &create_args(campaign, true),
            &start_cmd,
        )?;
        return Ok(VerbResult {
            exit_code,
            workspace_id: None,
        });
    }

    // Name what fresh retires, so the log says which conversation to
    // go back to by hand: sessions.log keeps it, but nothing else does.
    let retiring = old_id
        .as_deref()
        .map(|old| format!(", retiring {}...", short(old)))
        .unwrap_or_default();
    progress::step(&format!(
        "{address}: starting fresh conversation {}...{retiring}",
        short(&new_id)
    ));
    let (workspace_id, pane_id) =
        open_workspace(campaign, address, ws, close_mode, Verb::Fresh, &new_id)?;
    continuity::append(&campaign.dir, &new_id)
        .map_err(|e| EstateError::Config(format!("could not append to the continuity log: {e}")))?;
    println!(
        "live  {address}  {workspace_id}  {pane_id}  [{}...]  (fresh)",
        short(&new_id)
    );
    Ok(VerbResult {
        exit_code: 0,
        workspace_id: Some(workspace_id),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignRow {
    pub address: String,
    pub last: String,
    /// `frontmatter::brief` of its campaign.md, for the picker only
    /// (`format_row` leaves it out); empty when that file can't be read.
    pub brief: String,
}

pub fn list_campaigns(estates: &toml::Table) -> Vec<CampaignRow> {
    let mut rows = Vec::new();
    for estate_name in estates.keys() {
        let Some(campaigns_dir) = campaign::campaigns_dir(estates, estate_name)
            .ok()
            .filter(|d| d.is_dir())
        else {
            continue;
        };
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
            let brief = std::fs::read_to_string(cdir.join("campaign.md"))
                .map(|text| crate::frontmatter::brief(&text))
                .unwrap_or_default();
            rows.push(CampaignRow {
                address,
                last,
                brief,
            });
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

/// Fills a file `create` has just made at the path. A seam, so a test
/// can fail either of its writes part-way, or write beside them.
type Fill = fn(&Path, &mut std::fs::File, &str) -> std::io::Result<()>;

/// Writes one new file, never over anything that exists (`create_new`),
/// and removes it again if filling it fails.
fn write_new(path: &Path, text: &str, fill: Fill) -> std::io::Result<()> {
    let mut f = std::fs::File::create_new(path)?;
    fill(path, &mut f, text).inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

/// `herdr-estate create`: a new campaign directory holding `campaign.md`,
/// the brief under its heading, and `log.md`, whose first entry hands
/// both to the campaign's first agent to commit. Files only: no herdr
/// call, no git. Refuses what `name::refusal` or `name::brief_refusal`
/// refuses, a missing campaigns directory (never created here), and
/// anything at all at the path, even an empty directory: a renamed
/// campaign leaves its gitignored sessions.log behind, and adopting it
/// would inherit its conversations. `--dry-run` runs every check and
/// writes nothing.
pub fn create_campaign(
    estates: &toml::Table,
    address: &str,
    brief: &str,
    dry_run: bool,
) -> Result<(), EstateError> {
    create_with(estates, address, brief, dry_run, |_, f, text| {
        std::io::Write::write_all(f, text.as_bytes())
    })
}

fn create_with(
    estates: &toml::Table,
    address: &str,
    brief: &str,
    dry_run: bool,
    fill: Fill,
) -> Result<(), EstateError> {
    let (estate, name) = parse_address(address)?;
    let campaigns = campaign::campaigns_dir(estates, &estate)?;
    if !campaigns.is_dir() {
        return Err(EstateError::Config(format!(
            "{address}: no campaigns directory at {}, refusing to create one",
            campaigns.display()
        )));
    }
    let listed = list_campaigns(estates);
    let known: Vec<&str> = listed.iter().map(|r| r.address.as_str()).collect();
    let brief = brief.trim();
    let refused = name::refusal(&estate, &name, &known)
        .or_else(|| name::brief_refusal(brief).map(str::to_string));
    if let Some(reason) = refused {
        return Err(EstateError::Usage(format!("{address}: {reason}")));
    }
    let dir = campaigns.join(&name);
    let adopt = || {
        EstateError::Guard(format!(
            "{} already exists, refusing to adopt it",
            dir.display()
        ))
    };
    let paths = [dir.join("campaign.md"), dir.join("log.md")];
    if dry_run {
        if std::fs::symlink_metadata(&dir).is_ok() {
            return Err(adopt());
        }
        println!("would create:");
        for path in &paths {
            println!("  {}", path.display());
        }
        return Ok(());
    }
    // create_dir, never create_dir_all, is the check itself: it fails
    // on anything at the path, a dangling symlink or an empty directory
    // too.
    std::fs::create_dir(&dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => adopt(),
        _ => EstateError::Config(format!("could not create {}: {e}", dir.display())),
    })?;
    progress::step(&format!(
        "{address}: creating campaign.md and log.md in {} (untracked)",
        dir.display()
    ));
    let date = today(std::process::Command::new("date").arg("+%F"));
    let texts = [
        format!("---\n---\n\n# {name}\n\n## What this is\n\n{brief}\n"),
        log_md(&name, address, brief, &date),
    ];
    for (i, (path, text)) in paths.iter().zip(&texts).enumerate() {
        if let Err(e) = write_new(path, text, fill) {
            // Only what this call made: write_new has taken back its own
            // file, so the files before it, then the directory, which
            // remove_dir takes only once empty.
            for made in &paths[..i] {
                let _ = std::fs::remove_file(made);
            }
            let _ = std::fs::remove_dir(&dir);
            return Err(EstateError::Config(format!(
                "could not write {}: {e}",
                path.display()
            )));
        }
    }
    println!("created  {address}  {}", dir.display());
    Ok(())
}

/// A new campaign's `log.md`, in the estates' log convention: dated `##`
/// entries, the tail of the latest being the re-entry pointer.
fn log_md(name: &str, address: &str, brief: &str, date: &str) -> String {
    format!(
        "# {name}: log

## {date}: campaign created

Created as `{address}` with the brief: {brief}

Next: agree the scope and write it into campaign.md, then commit
campaign.md and log.md.
"
    )
}

/// The operator's local date, `YYYY-MM-DD`, from `date` (`date +%F`,
/// run as `continuity` runs `hostname`); the UTC date if it fails or
/// prints anything but a date.
fn today(date: &mut std::process::Command) -> String {
    date.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|d| is_date(d))
        .unwrap_or_else(|| continuity::now_utc_iso()[..10].to_string())
}

/// `YYYY-MM-DD` in shape.
fn is_date(s: &str) -> bool {
    s.split('-').map(str::len).eq([4, 2, 2]) && s.bytes().all(|b| b == b'-' || b.is_ascii_digit())
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
            missing_add_dirs: Vec::new(),
            prompt_files: Vec::new(),
            kind_flags: BTreeMap::new(),
            env: BTreeMap::from([("K".to_string(), "V".to_string())]),
            prepare: None,
            worktree: None,
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

    // --- shlex quoting ---

    #[test]
    fn shlex_quote_wraps_a_value_containing_a_space() {
        // A space is not in the safe charset; an unescaped one in a
        // `--dry-run` plan would let one argv element read as two.
        let quoted = shlex_quote("two words");
        assert_eq!(quoted, "'two words'");
    }

    #[test]
    fn shlex_quote_escapes_an_embedded_single_quote() {
        let quoted = shlex_quote("it's");
        assert_eq!(quoted, r#"'it'"'"'s'"#);
    }

    #[test]
    fn shlex_quote_leaves_a_safe_value_unquoted() {
        let quoted = shlex_quote("home/garden-1_2.3:4=5@6%7+8,9");
        assert_eq!(quoted, "home/garden-1_2.3:4=5@6%7+8,9");
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
        assert_eq!(
            close_if_ours("w1", "home/garden", false).unwrap(),
            CloseOutcome::Closed
        );
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
        assert_eq!(
            close_if_ours("w1", "home/garden", false).unwrap(),
            CloseOutcome::NotConfirmed
        );
        assert_eq!(fake.call_count(), 1); // only the get, never the close
    }

    #[test]
    fn close_if_ours_skips_when_required_idle_or_done_but_the_fresh_read_says_working() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "working"}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        assert_eq!(
            close_if_ours("w1", "home/garden", true).unwrap(),
            CloseOutcome::NotConfirmed
        );
        assert_eq!(fake.call_count(), 1); // only the get, never the close
    }

    #[test]
    fn close_if_ours_closes_when_required_idle_or_done_and_the_fresh_read_still_agrees() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "done", "pane_count": 1}}}"#,
            0,
        );
        fake.set_response(1, r#"{"result": {}}"#, 0);
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        assert_eq!(
            close_if_ours("w1", "home/garden", true).unwrap(),
            CloseOutcome::Closed
        );
        assert_eq!(fake.argv(1), vec!["workspace", "close", "w1"]);
    }

    #[test]
    fn close_if_ours_skips_when_required_idle_or_done_but_the_fresh_read_reports_a_second_pane() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "done", "pane_count": 2}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        assert_eq!(
            close_if_ours("w1", "home/garden", true).unwrap(),
            CloseOutcome::NotConfirmed
        );
        assert_eq!(fake.call_count(), 1); // only the get, never the close
    }

    #[test]
    fn close_if_ours_skips_when_required_idle_or_done_but_the_fresh_read_omits_pane_count() {
        let fake = FakeHerdr::new();
        fake.set_response(
            0,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "done"}}}"#,
            0,
        );
        let _g = EnvGuard::set(&[("HERDR_BIN_PATH", fake.bin_path().to_str().unwrap())]);
        assert_eq!(
            close_if_ours("w1", "home/garden", true).unwrap(),
            CloseOutcome::NotConfirmed
        );
        assert_eq!(fake.call_count(), 1); // only the get, never the close
    }

    // --- created_ids ---

    #[test]
    fn created_ids_missing_workspace_id_errors() {
        let bad = HerdrResult {
            ok: true,
            data: serde_json::json!({"root_pane": {"pane_id": "p1"}}),
        };
        assert!(matches!(created_ids(&bad), Err(EstateError::Protocol(_))));
    }

    #[test]
    fn created_ids_missing_pane_id_errors() {
        let bad = HerdrResult {
            ok: true,
            data: serde_json::json!({"workspace": {"workspace_id": "w1"}}),
        };
        assert!(matches!(created_ids(&bad), Err(EstateError::Protocol(_))));
    }

    #[test]
    fn created_ids_valid_response_returns_both() {
        let ok = HerdrResult {
            ok: true,
            data: serde_json::json!({
                "workspace": {"workspace_id": "w9", "label": "home/garden"},
                "root_pane": {"pane_id": "p1"},
            }),
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
    fn resume_prepare_success_runs_in_the_workdir_before_the_create_call() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();

        let marker = f._tmp.path().join("marker");
        let script = write_pwd_marker_script(f._tmp.path(), &marker);
        set_prepare(&mut f, &[script.to_str().unwrap()]);

        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        let rc = resume(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 3);
        // The script's own `pwd` resolves any symlink in the OS temp
        // dir (`/var` -> `/private/var` on macOS); canonicalize the
        // expected side the same way before comparing.
        let workdir = std::fs::canonicalize(f._tmp.path().join("home")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            workdir.to_str().unwrap()
        );
        assert!(progress::logged_messages().contains(&"home/garden: prepare ok".to_string()));
    }

    #[test]
    fn resume_prepare_failure_refuses_before_any_close_or_create() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        write_transcript("old-sid");
        let script = write_failing_script(f._tmp.path(), &["boom"]);
        set_prepare(&mut f, &[script.to_str().unwrap()]);
        f.fake.set_response(0, empty_list(), 0);

        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("prepare failed") && msg.contains("boom")),
            "{err}"
        );
        // Only the read that checked for a live workspace; no close, no
        // create, no start.
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn resume_prepare_spawn_failure_refuses_the_same_way() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        write_transcript("old-sid");
        let missing = f._tmp.path().join("no-such-binary");
        set_prepare(&mut f, &[missing.to_str().unwrap()]);
        f.fake.set_response(0, empty_list(), 0);

        let err = resume(&f.estates, "home/garden", false).unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("prepare failed")),
            "{err}"
        );
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn resume_dry_run_never_runs_prepare() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        write_transcript("old-sid");
        let marker = f._tmp.path().join("marker");
        let script = write_pwd_marker_script(f._tmp.path(), &marker);
        set_prepare(&mut f, &[script.to_str().unwrap(), "--flag"]);
        f.fake.set_response(0, empty_list(), 0);

        let rc = resume(&f.estates, "home/garden", true).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 1); // the live-workspace probe only
        assert!(!marker.exists(), "dry run must never execute prepare");
    }

    /// A persisted transcript for `sid`, which the claude-kind resume
    /// guard requires before it will start anything.
    fn write_transcript(sid: &str) {
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(format!("{sid}.jsonl")), "{}\n").unwrap();
    }

    // --- prepare ---

    fn prepare_campaign(workdir: &Path) -> Campaign {
        Campaign {
            address: "home/garden".to_string(),
            estate: "home".to_string(),
            name: "garden".to_string(),
            dir: workdir.to_path_buf(),
            workdir: workdir.to_path_buf(),
            kind: AgentKind::Claude,
            add_dirs: Vec::new(),
            missing_add_dirs: Vec::new(),
            prompt_files: Vec::new(),
            kind_flags: Default::default(),
            env: Default::default(),
            prepare: None,
            worktree: None,
        }
    }

    fn argv_of(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_prepare_past_its_deadline_is_killed_and_refused() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let campaign = prepare_campaign(d.path());
        let started = std::time::Instant::now();
        let err = run_prepare_within(
            &campaign,
            "home/garden",
            &argv_of(&["sleep", "30"]),
            std::time::Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(
            matches!(&err, EstateError::Guard(m) if m == "home/garden: prepare timed out after 0s"),
            "{err}"
        );
    }

    #[test]
    fn a_prepare_that_writes_more_than_a_pipe_holds_still_finishes() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let campaign = prepare_campaign(d.path());
        run_prepare_within(
            &campaign,
            "home/garden",
            &argv_of(&[
                "sh",
                "-c",
                "head -c 1000000 /dev/zero; head -c 1000000 /dev/zero >&2",
            ]),
            std::time::Duration::from_secs(20),
        )
        .unwrap();
    }

    #[test]
    fn a_prepare_that_leaves_a_process_holding_its_pipes_still_finishes() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let campaign = prepare_campaign(d.path());
        let started = std::time::Instant::now();
        run_prepare_within(
            &campaign,
            "home/garden",
            &argv_of(&["sh", "-c", "sleep 30 & exit 0"]),
            std::time::Duration::from_secs(20),
        )
        .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            progress::logged_messages(),
            vec!["home/garden: prepare ok (its output held open by a process it left running)"]
        );
    }

    #[test]
    fn a_failed_prepare_whose_pipes_stay_open_names_its_exit_and_why_it_says_no_more() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let campaign = prepare_campaign(d.path());
        let err = run_prepare_within(
            &campaign,
            "home/garden",
            &argv_of(&["sh", "-c", "echo boom >&2; sleep 30 & exit 3"]),
            std::time::Duration::from_secs(20),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EstateError::Guard(m) if m.contains("exit status: 3") && m.ends_with("held open by a process it left running)")),
            "{err}"
        );
    }

    #[test]
    fn the_prepare_deadline_is_twice_the_agent_starts() {
        assert_eq!(PREPARE_TIMEOUT_SECS, 2 * agent_start_timeout_secs());
    }

    /// Sets the fixture's `home` estate row's `prepare` key to `argv`.
    fn set_prepare(f: &mut Fixture, argv: &[&str]) {
        if let Some(toml::Value::Table(row)) = f.estates.get_mut("home") {
            row.insert(
                "prepare".to_string(),
                toml::Value::Array(
                    argv.iter()
                        .map(|s| toml::Value::String(s.to_string()))
                        .collect(),
                ),
            );
        }
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    }

    /// A `prepare` script that writes its own cwd to `marker` and exits
    /// 0 -- what the "runs with the workdir as cwd" tests read back.
    fn write_pwd_marker_script(dir: &Path, marker: &Path) -> PathBuf {
        let script = dir.join("prepare-pwd.sh");
        std::fs::write(&script, format!("#!/bin/sh\npwd > {marker:?}\n")).unwrap();
        make_executable(&script);
        script
    }

    /// A `prepare` script that prints each of `stderr_lines` to stderr,
    /// then exits non-zero.
    fn write_failing_script(dir: &Path, stderr_lines: &[&str]) -> PathBuf {
        let script = dir.join("prepare-fail.sh");
        let mut body = "#!/bin/sh\n".to_string();
        for line in stderr_lines {
            body.push_str(&format!("echo {line:?} >&2\n"));
        }
        body.push_str("exit 3\n");
        std::fs::write(&script, body).unwrap();
        make_executable(&script);
        script
    }

    #[test]
    fn resume_says_each_step_in_order() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid-0000").unwrap();
        write_transcript("old-sid-0000");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

        resume(&f.estates, "home/garden", false).unwrap();
        assert_eq!(
            progress::logged_messages(),
            vec![
                "home/garden: resuming conversation old-sid-...",
                "home/garden: creating a workspace",
                "home/garden: starting claude in w9, waiting up to 30s for it to be ready",
                "home/garden: claude is ready in w9",
            ]
        );
    }

    #[test]
    fn fresh_over_an_owned_dead_workspace_says_it_is_closing_it_first() {
        let f = make_fixture("claude");
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
        );
        f.fake.set_response(2, r#"{"result": {}}"#, 0); // close
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        fresh(&f.estates, "home/garden", false).unwrap();
        let logged = progress::logged_messages();
        assert_eq!(logged.len(), 5);
        assert!(logged[0].starts_with("home/garden: starting fresh conversation "));
        assert_eq!(
            logged[1],
            "home/garden: closing w1, which herdr-estate opened and whose agent has exited"
        );
        assert_eq!(logged[2], "home/garden: creating a workspace");
        assert_eq!(logged[4], "home/garden: claude is ready in w9");
    }

    #[test]
    fn a_failed_agent_start_never_says_the_agent_is_ready() {
        let f = make_fixture("claude");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake
            .set_response(2, r#"{"error": {"code": "timeout"}}"#, 1); // agent start

        assert!(fresh(&f.estates, "home/garden", false).is_err());
        let logged = progress::logged_messages();
        assert!(logged
            .last()
            .unwrap()
            .starts_with("home/garden: starting claude in w9"));
        assert!(!logged.iter().any(|m| m.contains("is ready")));
    }

    #[test]
    fn a_dry_run_and_a_guard_refusal_both_log_nothing() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        write_transcript("old-sid");
        f.fake.set_response(0, empty_list(), 0);
        resume(&f.estates, "home/garden", true).unwrap();
        assert!(progress::logged_messages().is_empty());

        // A running agent under the label: refused before any step.
        f.fake.set_response(
            1,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        assert!(resume(&f.estates, "home/garden", false).is_err());
        assert!(progress::logged_messages().is_empty());
    }

    #[test]
    fn only_a_stop_requires_the_agent_idle_or_done_at_the_close() {
        assert!(CloseMode::Stopping.requires_idle_or_done());
        assert!(!CloseMode::Exited.requires_idle_or_done());
        assert_ne!(CloseMode::Stopping.reason(), CloseMode::Exited.reason());
    }

    #[test]
    fn agent_start_timeout_secs_is_the_millisecond_constant_in_seconds() {
        assert_eq!(
            agent_start_timeout_secs(),
            AGENT_START_TIMEOUT_MS.parse::<u64>().unwrap() / 1000
        );
        assert_eq!(agent_start_timeout_secs(), 30);
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
            serde_json::json!({"sessionId": "old-sid", "name": "garden"}).to_string(),
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
        // The step names the conversation fresh retired.
        assert!(progress::logged_messages()[0].ends_with("..., retiring old-sid..."));
    }

    #[test]
    fn fresh_prepare_success_runs_before_the_close_and_the_create() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let marker = f._tmp.path().join("marker");
        let script = write_pwd_marker_script(f._tmp.path(), &marker);
        set_prepare(&mut f, &[script.to_str().unwrap()]);
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
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let rc = fresh(&f.estates, "home/garden", false).unwrap();
        assert_eq!(rc, 0);
        assert_eq!(f.fake.call_count(), 5);
        // The script's own `pwd` resolves any symlink in the OS temp
        // dir (`/var` -> `/private/var` on macOS); canonicalize the
        // expected side the same way before comparing.
        let workdir = std::fs::canonicalize(f._tmp.path().join("home")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            workdir.to_str().unwrap()
        );
        let logged = progress::logged_messages();
        let prepare_at = logged
            .iter()
            .position(|l| l == "home/garden: prepare ok")
            .unwrap();
        let closing_at = logged
            .iter()
            .position(|l| l.contains("closing w1"))
            .unwrap();
        assert!(prepare_at < closing_at, "{logged:?}");
    }

    #[test]
    fn fresh_prepare_failure_refuses_before_any_close_or_create() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let script = write_failing_script(
            f._tmp.path(),
            &["boom", "line two", "line three", "dropped"],
        );
        set_prepare(&mut f, &[script.to_str().unwrap()]);
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg == "home/garden: prepare failed: boom | line two | line three"),
            "{err}"
        );
        // Only the read that found the dormant workspace; no close-get,
        // no close, no create, no start.
        assert_eq!(f.fake.call_count(), 1);
        assert!(
            ownership::is_ours("w1"),
            "the workspace must not be forgotten"
        );
    }

    #[test]
    fn fresh_with_no_recorded_conversation_names_nothing_retired() {
        let f = make_fixture("claude");
        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        fresh(&f.estates, "home/garden", false).unwrap();
        let first = &progress::logged_messages()[0];
        assert!(first.starts_with("home/garden: starting fresh conversation "));
        assert!(!first.contains("retiring"));
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
            serde_json::json!({"sessionId": "old-sid", "name": "garden"}).to_string(),
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

    // --- fresh_with_ids_stop_running: the picker's Ctrl-f second press,
    // never reachable from the CLI.

    #[test]
    fn stop_running_closes_an_idle_agent_and_starts_fresh_with_a_log_retire() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        ); // live_workspace, inside check_replaceable_running_workspace
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "idle", "pane_count": 1}}}"#,
            0,
        ); // close_if_ours get
        f.fake.set_response(2, r#"{"result": {}}"#, 0); // close
        f.fake.set_response(3, CREATE_OK, 0); // create
        f.fake.set_response(4, r#"{"result": {}}"#, 0); // start

        let result = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.workspace_id.as_deref(), Some("w9"));
        assert_eq!(f.fake.call_count(), 5);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
        assert_eq!(
            progress::logged_messages()[1],
            "home/garden: closing w1, stopping its agent to start a fresh conversation"
        );
        // Retired exactly the way an ordinary fresh does: appended to
        // sessions.log, the old id kept, not overwritten.
        let lines = continuity::read(&f.cdir);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" old-sid"), "{lines:?}");
        assert!(!ownership::is_ours("w1"));
        assert!(ownership::is_ours("w9"));
    }

    #[test]
    fn a_start_that_fails_after_the_close_leaves_every_record_matching_herdr() {
        // The order a run cut short anywhere past the close must leave
        // true: the closed workspace is forgotten as soon as it is gone,
        // the new one is owned as soon as it exists, and sessions.log
        // names the new conversation only once its agent has started.
        // Here the start fails, so the old conversation stays the latest
        // (resumable), and the new workspace, which herdr does hold, is
        // ours to close on the next run.
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
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
        f.fake.set_response(3, CREATE_OK, 0); // create
        f.fake
            .set_response(4, r#"{"error": {"code": "timeout"}}"#, 1); // start

        assert!(fresh_with_ids_stop_running(&f.estates, "home/garden").is_err());
        assert_eq!(
            continuity::latest_conversation(&f.cdir).as_deref(),
            Some("old-sid")
        );
        assert_eq!(continuity::read(&f.cdir).len(), 1);
        assert!(!ownership::is_ours("w1"));
        assert!(ownership::is_ours("w9"));
    }

    #[test]
    fn stop_running_prepare_success_runs_before_the_close_and_the_create() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let marker = f._tmp.path().join("marker");
        let script = write_pwd_marker_script(f._tmp.path(), &marker);
        set_prepare(&mut f, &[script.to_str().unwrap()]);
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
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let result = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(f.fake.call_count(), 5);
        // The script's own `pwd` resolves any symlink in the OS temp
        // dir (`/var` -> `/private/var` on macOS); canonicalize the
        // expected side the same way before comparing.
        let workdir = std::fs::canonicalize(f._tmp.path().join("home")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            workdir.to_str().unwrap()
        );
        assert_eq!(progress::logged_messages()[1], "home/garden: prepare ok");
        assert_eq!(
            progress::logged_messages()[2],
            "home/garden: closing w1, stopping its agent to start a fresh conversation"
        );
    }

    #[test]
    fn stop_running_prepare_failure_refuses_before_any_close_or_create() {
        let mut f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let script = write_failing_script(f._tmp.path(), &[]);
        set_prepare(&mut f, &[script.to_str().unwrap()]);
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );

        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        // No stderr and no stdout at all: the exit status is the only
        // detail left to name.
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.starts_with("home/garden: prepare failed: exit status")),
            "{err}"
        );
        assert_eq!(f.fake.call_count(), 1);
        assert!(ownership::is_ours("w1"));
    }

    #[test]
    fn stop_running_closes_a_done_agent_too() {
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "done", "pane_count": 1}
            ]}}"#,
            0,
        );
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "done", "pane_count": 1}}}"#,
            0,
        );
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let result = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
    }

    #[test]
    fn stop_running_succeeds_though_the_retiring_conversation_is_its_own_live_registry_holder() {
        // The idle agent being stopped is, by definition, still
        // attached: `~/.claude/sessions/<pid>.json` for its own live
        // pid names the very conversation this call is about to
        // retire. That must not read as a double-open in some other
        // pane -- if it did, this path could never fire for a claude
        // campaign, the default kind.
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let sessions = crate::paths::home_dir().join(".claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{}.json", std::process::id())),
            serde_json::json!({"sessionId": "old-sid", "name": "garden"}).to_string(),
        )
        .unwrap();
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
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

        let result = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
    }

    /// The test binary's own pid via `ps`, for a second live pid
    /// distinct from `std::process::id()` -- the parent that launched
    /// it, guaranteed alive for as long as the test runs.
    fn parent_pid() -> u32 {
        let out = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    #[test]
    fn stop_running_refuses_a_second_live_holder_of_the_retiring_conversation() {
        // Unlike the test above, a SECOND live pid also holds the
        // conversation this call is about to retire -- that pane is
        // not this one, and the close is about to pull the
        // conversation out from under it.
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        ownership::record("home/garden", "w1").unwrap();
        let sessions = crate::paths::home_dir().join(".claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join(format!("{}.json", std::process::id())),
            serde_json::json!({"sessionId": "old-sid", "name": "this-pane"}).to_string(),
        )
        .unwrap();
        let other_pid = parent_pid();
        std::fs::write(
            sessions.join(format!("{other_pid}.json")),
            serde_json::json!({"sessionId": "old-sid", "name": "other-pane"}).to_string(),
        )
        .unwrap();
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("this-pane") && msg.contains("other-pane")),
            "{err:?}"
        );
        // Refused before check_replaceable_running_workspace runs at
        // all: no list, no get, no close.
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn stop_running_refuses_a_workspace_carrying_more_than_one_pane() {
        // agent_status is a single aggregate over every pane in the
        // workspace, so "idle" here could still be a second, genuinely
        // working pane outranked in that aggregate. A close tears down
        // every pane, so this must refuse before ever touching one.
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 2}
            ]}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("2 panes")),
            "{err:?}"
        );
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_refuses_a_workspace_with_no_pane_count_at_all() {
        // A listing that omits pane_count is not a known single pane
        // either -- this must refuse the same as a count above one,
        // never wave an unknown pane count through.
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("no pane count")),
            "{err:?}"
        );
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_refuses_working_blocked_and_missing_status_with_no_close_call() {
        // pane_count: 1 on every row here, so it's the agent_status
        // guard under test, not the separate pane-count one above.
        for workspaces_json in [
            r#"[{"workspace_id": "w1", "label": "home/garden", "agent_status": "working", "pane_count": 1}]"#,
            r#"[{"workspace_id": "w1", "label": "home/garden", "agent_status": "blocked", "pane_count": 1}]"#,
            r#"[{"workspace_id": "w1", "label": "home/garden", "pane_count": 1}]"#,
        ] {
            let f = make_fixture("claude");
            ownership::record("home/garden", "w1").unwrap();
            f.fake.set_response(
                0,
                &format!(r#"{{"result": {{"workspaces": {workspaces_json}}}}}"#),
                0,
            );
            let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
            assert!(matches!(err, EstateError::Guard(_)), "{workspaces_json}");
            // Only the list that found it: no close, create or start.
            assert_eq!(f.fake.call_count(), 1, "{workspaces_json}");
        }
    }

    #[test]
    fn stop_running_refuses_with_no_live_workspace() {
        let f = make_fixture("claude");
        f.fake.set_response(0, empty_list(), 0);
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_refuses_a_workspace_herdr_estate_did_not_create() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "unowned-1", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("did not create")));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_refuses_duplicate_labels() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(matches!(err, EstateError::Guard(_)));
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_refuses_on_a_missing_prompt_file_before_closing_anything() {
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
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        assert!(matches!(
            fresh_with_ids_stop_running(&f.estates, "home/garden"),
            Err(EstateError::Config(_))
        ));
        // Only the read that found the idle workspace; nothing closed.
        assert_eq!(f.fake.call_count(), 1);
    }

    #[test]
    fn stop_running_unconfirmed_close_creates_nothing() {
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        // close_if_ours's pre-close get reports a different label now.
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/moved"}}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(msg) if msg.contains("could not be confirmed closed"))
        );
        assert_no_create_or_start(&f.fake);
    }

    #[test]
    fn stop_running_unconfirmed_close_when_the_pre_close_get_reports_a_second_pane() {
        // The list that found it carried exactly one pane, but herdr's
        // own pre-close read now reports two -- close_if_ours's own
        // re-check, not check_replaceable_running_workspace's.
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle", "pane_count": 1}
            ]}}"#,
            0,
        );
        f.fake.set_response(
            1,
            r#"{"result": {"workspace": {"label": "home/garden", "agent_status": "idle", "pane_count": 2}}}"#,
            0,
        );
        let err = fresh_with_ids_stop_running(&f.estates, "home/garden").unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(msg) if msg.contains("could not be confirmed closed"))
        );
        // Only the list and the pre-close get: no close call at all.
        assert_eq!(f.fake.call_count(), 2);
        assert_no_create_or_start(&f.fake);
    }

    #[test]
    fn cli_fresh_keeps_refusing_an_idle_running_agent() {
        // The picker's second-press path above is never what the CLI's
        // `fresh` reaches: it still goes through `check_live_workspace`,
        // which refuses any live agent_status other than "unknown",
        // idle included. Ownership is recorded here so the alive check
        // is what actually refuses, not the ownership check beneath it
        // (which would refuse just as well on an unrecorded w1, hiding
        // whether the running-agent guard fired at all).
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "idle"}
            ]}}"#,
            0,
        );
        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(
            matches!(err, EstateError::Guard(ref msg) if msg.contains("running agent")),
            "{err:?}"
        );
        assert_eq!(f.fake.call_count(), 1);
    }

    // --- an unconfirmed close stops the verb: never create a
    // replacement over a workspace that might still be there.

    fn assert_no_create_or_start(fake: &FakeHerdr) {
        for i in 0..fake.call_count() {
            let argv = fake.argv(i);
            assert!(
                !(argv.first().map(String::as_str) == Some("workspace")
                    && argv.get(1).map(String::as_str) == Some("create")),
                "unexpected workspace create in call {i}: {argv:?}"
            );
            assert_ne!(
                argv.first().map(String::as_str),
                Some("agent"),
                "unexpected agent call {i}: {argv:?}"
            );
        }
    }

    #[test]
    fn fresh_stops_when_the_pre_close_get_fails() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let before = continuity::read(&f.cdir);
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        f.fake.set_response(1, "", 1); // workspace get fails

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 2); // list, get — nothing further
        assert_no_create_or_start(&f.fake);
        assert_eq!(continuity::read(&f.cdir), before);
    }

    #[test]
    fn fresh_stops_when_the_label_changed_between_list_and_get() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let before = continuity::read(&f.cdir);
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
            r#"{"result": {"workspace": {"label": "someone-elses"}}}"#,
            0,
        );

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 2);
        assert_no_create_or_start(&f.fake);
        assert_eq!(continuity::read(&f.cdir), before);
        // The one refusal that comes after a progress line: the close
        // was announced, then refused, and nothing after it claims a
        // create or a start.
        let logged = progress::logged_messages();
        assert_eq!(logged.len(), 2);
        assert!(logged[0].starts_with("home/garden: starting fresh conversation "));
        assert!(logged[1].starts_with("home/garden: closing w1"));
    }

    #[test]
    fn fresh_stops_when_herdr_reports_the_close_itself_failed() {
        let f = make_fixture("claude");
        continuity::append(&f.cdir, "old-sid").unwrap();
        let before = continuity::read(&f.cdir);
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
        );
        f.fake.set_response(2, "", 1); // workspace close fails

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 3); // list, get, close — no create/start
        assert_no_create_or_start(&f.fake);
        assert_eq!(continuity::read(&f.cdir), before);
    }

    #[test]
    fn dry_run_stops_when_the_pre_close_get_fails() {
        let f = make_fixture("claude");
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        f.fake.set_response(1, "", 1); // workspace get fails

        let err = fresh(&f.estates, "home/garden", true).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 2); // the dry run confirms the same way
    }

    // --- more than one workspace already carries the label: both verbs
    // refuse rather than acting on the first row found.

    #[test]
    fn fresh_refuses_when_two_workspaces_already_carry_the_label() {
        let f = make_fixture("claude");
        // The first row is ours and not alive — today's `.find()` would
        // close and recreate over it, invisible to the second row.
        ownership::record("home/garden", "w1").unwrap();
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("w1") && msg.contains("w2")));
        assert_eq!(f.fake.call_count(), 1); // only the list — nothing closed, nothing created
    }

    #[test]
    fn fresh_dry_run_refuses_when_two_workspaces_already_carry_the_label() {
        let f = make_fixture("claude");
        f.fake.set_response(
            0,
            r#"{"result": {"workspaces": [
                {"workspace_id": "w1", "label": "home/garden", "agent_status": "unknown"},
                {"workspace_id": "w2", "label": "home/garden", "agent_status": "unknown"}
            ]}}"#,
            0,
        );
        let err = fresh(&f.estates, "home/garden", true).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg) if msg.contains("w1") && msg.contains("w2")));
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
        std::fs::write(
            garden.join("campaign.md"),
            "---\n---\n# garden\nGrow beds.\n",
        )
        .unwrap();
        // Not UTF-8, so unreadable as text: still listed, with no brief.
        std::fs::write(taxes.join("campaign.md"), b"---\n---\n\xff\n").unwrap();
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
        assert_eq!(garden_row.brief, "Grow beds.");
        assert_eq!(taxes_row.brief, "");
    }

    #[test]
    fn format_row_shows_new_or_last() {
        let new_row = CampaignRow {
            address: "home/x".to_string(),
            last: String::new(),
            brief: "never printed".to_string(),
        };
        assert!(format_row(&new_row).ends_with("new"));
        assert!(!format_row(&new_row).contains("never printed"));
        let dormant = CampaignRow {
            address: "home/y".to_string(),
            last: "2026-09-21".to_string(),
            brief: String::new(),
        };
        assert!(format_row(&dormant).contains("last 2026-09-21"));
    }

    // --- create ---

    /// The file names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn create_writes_campaign_md_and_a_dated_log_md_and_nothing_else() {
        let f = make_fixture("claude");
        let dir = f.cdir.parent().unwrap().join("quern");
        create_campaign(&f.estates, "home/quern", " Grind the grain. ", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("campaign.md")).unwrap(),
            "---\n---\n\n# quern\n\n## What this is\n\nGrind the grain.\n"
        );
        // Whatever today is, the rest of the log is log_md's.
        let log = std::fs::read_to_string(dir.join("log.md")).unwrap();
        let date = &log["# quern: log\n\n## ".len()..][..10];
        assert!(is_date(date), "{date:?}");
        assert_eq!(log, log_md("quern", "home/quern", "Grind the grain.", date));
        assert_eq!(names_in(&dir), ["campaign.md", "log.md"]);
        let row = list_campaigns(&f.estates)
            .into_iter()
            .find(|r| r.address == "home/quern")
            .unwrap();
        assert_eq!(
            (row.last.as_str(), row.brief.as_str()),
            ("", "Grind the grain.")
        );
        assert_eq!(
            progress::logged_messages(),
            vec![format!(
                "home/quern: creating campaign.md and log.md in {} (untracked)",
                dir.display()
            )]
        );
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn log_md_follows_the_estates_log_convention_dated_as_given() {
        assert_eq!(
            log_md("quern", "home/quern", "Grind the grain.", "2026-09-22"),
            "# quern: log\n\n## 2026-09-22: campaign created\n\n\
             Created as `home/quern` with the brief: Grind the grain.\n\n\
             Next: agree the scope and write it into campaign.md, then commit\n\
             campaign.md and log.md.\n"
        );
    }

    #[test]
    fn the_log_is_dated_by_date_else_by_the_utc_date() {
        use std::process::Command;
        assert_eq!(today(Command::new("echo").arg("2031-01-02")), "2031-01-02");
        // A `date` that fails, prints nothing, or prints no date.
        assert!(!is_date("") && !is_date("Tue Sep 22"));
        for date in [
            today(&mut Command::new("false")),
            today(&mut Command::new("true")),
            today(Command::new("echo").arg("Tue Sep 22")),
        ] {
            assert!(is_date(&date), "{date:?}");
        }
    }

    #[test]
    fn create_never_adopts_anything_already_at_the_path() {
        let f = make_fixture("claude");
        let campaigns = f.cdir.parent().unwrap();
        // An empty directory, and what a rename leaves: a sessions.log.
        // A real run meets them at create_dir, a dry run at its check.
        let residue = "2026-09-01T00:00:00Z host old-sid\n";
        std::fs::create_dir(campaigns.join("empty")).unwrap();
        std::fs::create_dir(campaigns.join("moved")).unwrap();
        std::fs::write(campaigns.join("moved/sessions.log"), residue).unwrap();
        for (address, dry_run) in [
            ("home/empty", false),
            ("home/moved", false),
            ("home/moved", true),
        ] {
            let err = create_campaign(&f.estates, address, "b", dry_run).unwrap_err();
            assert!(
                matches!(&err, EstateError::Guard(m) if m.ends_with("already exists, refusing to adopt it")),
                "{err}"
            );
        }
        assert!(names_in(&campaigns.join("empty")).is_empty());
        assert_eq!(names_in(&campaigns.join("moved")), ["sessions.log"]);
        assert_eq!(
            std::fs::read_to_string(campaigns.join("moved/sessions.log")).unwrap(),
            residue
        );
        assert!(progress::logged_messages().is_empty());
    }

    #[test]
    fn create_refuses_an_unknown_or_uncloned_estate_and_never_makes_a_campaigns_dir() {
        let mut f = make_fixture("claude");
        let bare = f._tmp.path().join("bare");
        std::fs::create_dir(&bare).unwrap();
        for (estate, root) in [("gone", f._tmp.path().join("gone")), ("bare", bare.clone())] {
            let mut row = toml::Table::new();
            row.insert(
                "root".to_string(),
                toml::Value::String(root.to_string_lossy().to_string()),
            );
            f.estates
                .insert(estate.to_string(), toml::Value::Table(row));
        }
        for (address, says) in [
            ("lab/quern", "no estate 'lab'"),
            ("gone/quern", "not cloned"),
            ("bare/quern", "no campaigns directory"),
        ] {
            let err = create_campaign(&f.estates, address, "b", false).unwrap_err();
            assert!(
                matches!(&err, EstateError::Config(m) if m.contains(says)),
                "{err}"
            );
        }
        assert!(names_in(&bare).is_empty());
    }

    #[test]
    fn create_refuses_a_bad_name_an_existing_campaign_or_a_bad_brief() {
        let f = make_fixture("claude");
        let long = "b".repeat(121);
        for (address, brief, reason) in [
            ("home/Quern", "b", "can't create: a-z 0-9 and single -"),
            ("home/garden", "b", "already a campaign"),
            ("home/quern", "  ", "type a one-line brief first"),
            ("home/quern", long.as_str(), "brief too long (max 120)"),
        ] {
            let err = create_campaign(&f.estates, address, brief, false).unwrap_err();
            assert!(
                matches!(&err, EstateError::Usage(m) if *m == format!("{address}: {reason}")),
                "{err}"
            );
        }
        assert_eq!(names_in(f.cdir.parent().unwrap()), ["garden"]);
        assert!(progress::logged_messages().is_empty());
    }

    #[test]
    fn a_failed_write_removes_only_what_create_made() {
        // A write that stops a byte in, as on a full disk, leaving a
        // part-written file.
        fn part_way(f: &mut std::fs::File, text: &str, fails: bool) -> std::io::Result<()> {
            use std::io::Write;
            f.write_all(&text.as_bytes()[..1])?;
            if fails {
                return Err(std::io::Error::other("disk full"));
            }
            f.write_all(&text.as_bytes()[1..])
        }
        fn fail_campaign_md(path: &Path, f: &mut std::fs::File, text: &str) -> std::io::Result<()> {
            part_way(f, text, path.ends_with("campaign.md"))
        }
        fn fail_log_md(path: &Path, f: &mut std::fs::File, text: &str) -> std::io::Result<()> {
            part_way(f, text, path.ends_with("log.md"))
        }
        let f = make_fixture("claude");
        for fill in [fail_campaign_md as Fill, fail_log_md] {
            let err = create_with(&f.estates, "home/quern", "b", false, fill).unwrap_err();
            assert!(
                matches!(&err, EstateError::Config(m) if m.contains("disk full")),
                "{err}"
            );
            // The directory is gone, and the campaign beside it stays.
            assert_eq!(names_in(f.cdir.parent().unwrap()), ["garden"]);
        }
        assert!(f.cdir.join("campaign.md").is_file());
    }

    #[test]
    fn a_file_another_writer_puts_in_the_new_directory_is_never_clobbered_or_removed() {
        // Something drops a log.md in while campaign.md is written:
        // create_new refuses it, and the cleanup takes back campaign.md
        // but leaves that file and the directory holding it.
        fn racer(path: &Path, f: &mut std::fs::File, text: &str) -> std::io::Result<()> {
            std::fs::write(path.with_file_name("log.md"), "theirs\n")?;
            std::io::Write::write_all(f, text.as_bytes())
        }
        let f = make_fixture("claude");
        let dir = f.cdir.parent().unwrap().join("quern");
        let err = create_with(&f.estates, "home/quern", "b", false, racer).unwrap_err();
        assert!(matches!(err, EstateError::Config(_)), "{err}");
        assert_eq!(names_in(&dir), ["log.md"]);
        assert_eq!(
            std::fs::read_to_string(dir.join("log.md")).unwrap(),
            "theirs\n"
        );
    }

    #[test]
    fn create_dry_run_runs_every_check_and_writes_and_logs_nothing() {
        let f = make_fixture("claude");
        create_campaign(&f.estates, "home/quern", "b", true).unwrap();
        assert_eq!(names_in(f.cdir.parent().unwrap()), ["garden"]);
        assert!(progress::logged_messages().is_empty());
    }
}
