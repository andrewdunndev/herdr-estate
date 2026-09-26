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

pub const PREPARE_TIMEOUT_SECS: u64 = 60;

const PREPARE_PIPE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

const PREPARE_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
pub const PANE_PLACEHOLDER: &str = "<PANE_ID>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseMode {
    Exited,
    Stopping,
}

impl CloseMode {
    fn reason(self) -> &'static str {
        match self {
            CloseMode::Exited => "which herdr-estate opened and whose agent has exited",
            CloseMode::Stopping => "stopping its agent to start a fresh conversation",
        }
    }

    fn requires_idle_or_done(self) -> bool {
        self == CloseMode::Stopping
    }
}

// The agent inherits herdr's server env, so blank what describes this one.
pub const WORKSPACE_ENV_SCRUB: &[&str] = &["ANTHROPIC_API_KEY", "TMUX", "TMUX_PANE"];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    NotConfirmed,
}

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

fn refuse_not_ours(address: &str) -> EstateError {
    EstateError::Guard(format!(
        "{address}: a workspace with this label exists that herdr-estate did not create. Close it yourself"
    ))
}

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

fn run_prepare(campaign: &Campaign, address: &str, argv: &[String]) -> Result<(), EstateError> {
    run_prepare_within(
        campaign,
        address,
        argv,
        std::time::Duration::from_secs(PREPARE_TIMEOUT_SECS),
    )
}

fn run_prepare_within(
    campaign: &Campaign,
    address: &str,
    argv: &[String],
    timeout: std::time::Duration,
) -> Result<(), EstateError> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .process_group(0)
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
            kill_process_group(&mut child);
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

fn kill_process_group(child: &mut std::process::Child) {
    let group = format!("-{}", child.id());
    let signal = |name: &str| {
        let _ = std::process::Command::new("kill")
            .args(["-s", name, "--", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    signal("TERM");
    let grace_ends = std::time::Instant::now() + PREPARE_KILL_GRACE;
    while std::time::Instant::now() < grace_ends {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    signal("KILL");
    let _ = child.kill();
    let _ = child.wait();
}

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
    let kind = campaign.kind.as_str();
    progress::step(&format!(
        "{address}: starting {kind} in {workspace_id}, waiting up to {}s for it to be ready",
        agent_start_timeout_secs()
    ));
    herdrcli::call_herdr(&start_args(campaign, &pane_id, agent_argv), true)?;
    progress::step(&format!("{address}: {kind} is ready in {workspace_id}"));
    Ok((workspace_id, pane_id))
}

pub(crate) fn agent_start_timeout_secs() -> u64 {
    AGENT_START_TIMEOUT_MS.parse::<u64>().unwrap_or(0) / 1000
}

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

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

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
                short_id(&conversation_id)
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
        short_id(&conversation_id)
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
        short_id(&conversation_id)
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

fn fresh_resolved(
    campaign: &Campaign,
    address: &str,
    dry_run: bool,
) -> Result<VerbResult, EstateError> {
    let old_id = fresh_registry_guard(campaign, address)?;
    let ws = check_live_workspace(address)?;
    fresh_open(campaign, address, old_id, ws, CloseMode::Exited, dry_run)
}

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

    let retiring = old_id
        .as_deref()
        .map(|old| format!(", retiring {}...", short_id(old)))
        .unwrap_or_default();
    progress::step(&format!(
        "{address}: starting fresh conversation {}...{retiring}",
        short_id(&new_id)
    ));
    let (workspace_id, pane_id) =
        open_workspace(campaign, address, ws, close_mode, Verb::Fresh, &new_id)?;
    continuity::append(&campaign.dir, &new_id)
        .map_err(|e| EstateError::Config(format!("could not append to the continuity log: {e}")))?;
    println!(
        "live  {address}  {workspace_id}  {pane_id}  [{}...]  (fresh)",
        short_id(&new_id)
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

type Fill = fn(&Path, &mut std::fs::File, &str) -> std::io::Result<()>;

fn write_new(path: &Path, text: &str, fill: Fill) -> std::io::Result<()> {
    let mut f = std::fs::File::create_new(path)?;
    fill(path, &mut f, text).inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

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

fn today(date: &mut std::process::Command) -> String {
    date.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|d| is_date(d))
        .unwrap_or_else(|| continuity::now_utc_iso()[..10].to_string())
}

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

    #[test]
    fn shlex_quote_wraps_a_value_containing_a_space() {
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
        assert_eq!(fake.call_count(), 1);
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
        assert_eq!(fake.call_count(), 1);
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
        assert_eq!(fake.call_count(), 1);
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
        assert_eq!(fake.call_count(), 1);
    }

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

        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("old-sid.jsonl"), "{}\n").unwrap();

        f.fake.set_response(0, empty_list(), 0);
        f.fake.set_response(1, CREATE_OK, 0);
        f.fake.set_response(2, r#"{"result": {}}"#, 0);

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
        assert_eq!(f.fake.call_count(), 1);
        assert!(!marker.exists(), "dry run must never execute prepare");
    }

    fn write_transcript(sid: &str) {
        let proj = crate::paths::home_dir()
            .join(".claude")
            .join("projects")
            .join("p");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(format!("{sid}.jsonl")), "{}\n").unwrap();
    }

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
    fn a_prepare_past_its_deadline_takes_what_it_started_down_with_it() {
        let d = crate::test_support::TempDir::new();
        let _g = crate::test_support::EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().unwrap(),
        )]);
        let campaign = prepare_campaign(d.path());
        let pidfile = d.path().join("sleeper.pid");
        let script = format!("sleep 300 & echo $! > {}; wait", pidfile.display());
        let err = run_prepare_within(
            &campaign,
            "home/garden",
            &argv_of(&["sh", "-c", &script]),
            std::time::Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EstateError::Guard(m) if m.contains("timed out")),
            "{err}"
        );
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = (0..60).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            !crate::doubleopen::pid_alive(pid)
        });
        if !gone {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
        assert!(gone, "the backgrounded sleeper {pid} outlived the timeout");
    }

    #[test]
    fn the_prepare_deadline_is_twice_the_agent_starts() {
        assert_eq!(PREPARE_TIMEOUT_SECS, 2 * agent_start_timeout_secs());
    }

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

    fn write_pwd_marker_script(dir: &Path, marker: &Path) -> PathBuf {
        let script = dir.join("prepare-pwd.sh");
        std::fs::write(&script, format!("#!/bin/sh\npwd > {marker:?}\n")).unwrap();
        make_executable(&script);
        script
    }

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
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
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
            .set_response(2, r#"{"error": {"code": "timeout"}}"#, 1);

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
        assert_eq!(continuity::read(&f.cdir).len(), 1);
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
        assert_eq!(f.fake.call_count(), 1);
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
        );
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake.set_response(4, r#"{"result": {}}"#, 0);

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
        assert_eq!(f.fake.call_count(), 3);
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
        assert_eq!(result.workspace_id.as_deref(), Some("w9"));
        assert_eq!(f.fake.call_count(), 5);
        assert_eq!(f.fake.argv(2), vec!["workspace", "close", "w1"]);
        assert_eq!(
            progress::logged_messages()[1],
            "home/garden: closing w1, stopping its agent to start a fresh conversation"
        );
        let lines = continuity::read(&f.cdir);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" old-sid"), "{lines:?}");
        assert!(!ownership::is_ours("w1"));
        assert!(ownership::is_ours("w9"));
    }

    #[test]
    fn a_start_that_fails_after_the_close_leaves_every_record_matching_herdr() {
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
        f.fake.set_response(2, r#"{"result": {}}"#, 0);
        f.fake.set_response(3, CREATE_OK, 0);
        f.fake
            .set_response(4, r#"{"error": {"code": "timeout"}}"#, 1);

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

    fn parent_pid() -> u32 {
        let out = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    #[test]
    fn stop_running_refuses_a_second_live_holder_of_the_retiring_conversation() {
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
        assert_eq!(f.fake.call_count(), 0);
    }

    #[test]
    fn stop_running_refuses_a_workspace_carrying_more_than_one_pane() {
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
        assert_eq!(f.fake.call_count(), 2);
        assert_no_create_or_start(&f.fake);
    }

    #[test]
    fn cli_fresh_keeps_refusing_an_idle_running_agent() {
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
        f.fake.set_response(1, "", 1);

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 2);
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
        f.fake.set_response(2, "", 1);

        let err = fresh(&f.estates, "home/garden", false).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 3);
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
        f.fake.set_response(1, "", 1);

        let err = fresh(&f.estates, "home/garden", true).unwrap_err();
        assert!(matches!(err, EstateError::Guard(msg)
            if msg.contains("w1") && msg.contains("could not be confirmed closed")));
        assert_eq!(f.fake.call_count(), 2);
    }

    #[test]
    fn fresh_refuses_when_two_workspaces_already_carry_the_label() {
        let f = make_fixture("claude");
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
        assert_eq!(f.fake.call_count(), 1);
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
            assert_eq!(names_in(f.cdir.parent().unwrap()), ["garden"]);
        }
        assert!(f.cdir.join("campaign.md").is_file());
    }

    #[test]
    fn a_file_another_writer_puts_in_the_new_directory_is_never_clobbered_or_removed() {
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
