"""resume, fresh and list: the three actions, and the herdr command
shapes behind them.

Every workspace is created with --label <address> and
--env HERDR_ESTATE_ADDRESS=<address> plus the estate's own env table
(design note, "Behaviour"). fresh is the only verb that ever closes a
workspace, and only after re-reading its label immediately before the
close call confirms it is still ours — ids are reused across a server
restart, so a stale record could otherwise name a workspace the
operator opened by hand. resume never closes anything; it only refuses
to proceed when the address is already live with a running agent.
"""
from __future__ import annotations

import os
import re
import shlex
import sys
import uuid

from . import campaign as campaign_mod
from . import adapters, continuity, doubleopen, herdrcli, prompt
from .campaign import Campaign
from .config import DEFAULT_CAMPAIGNS_DIR
from .errors import GuardError, UsageError
from .paths import expand

AGENT_START_TIMEOUT_MS = "30000"
PANE_PLACEHOLDER = "<PANE_ID>"
WORKSPACE_PLACEHOLDER = "<WORKSPACE_ID>"


def agent_name(address: str) -> str:
    n = re.sub(r"[^a-z0-9_-]", "-", address.lower()).lstrip("-") or "estate"
    return (n if n[0].isalpha() else "a" + n)[:32]


def create_args(campaign: Campaign) -> list[str]:
    args = ["workspace", "create", "--cwd", str(campaign.workdir),
            "--label", campaign.address,
            "--env", f"HERDR_ESTATE_ADDRESS={campaign.address}"]
    for k, v in campaign.env.items():
        args += ["--env", f"{k}={v}"]
    return args


def start_args(campaign: Campaign, pane_id: str, agent_argv: list[str]) -> list[str]:
    return ["agent", "start", agent_name(campaign.address), "--kind", campaign.kind,
            "--pane", pane_id, "--timeout", AGENT_START_TIMEOUT_MS, "--"] + agent_argv


def derive_address() -> str:
    workspace_id = os.environ.get("HERDR_WORKSPACE_ID")
    if not workspace_id:
        raise UsageError("no address given and $HERDR_WORKSPACE_ID is unset — "
                          "pass one explicitly")
    result = herdrcli.call_herdr(["workspace", "get", workspace_id])
    label = result.data.get("workspace", {}).get("label")
    if not label:
        raise UsageError(f"workspace {workspace_id} carries no label to derive "
                          "an address from")
    return label


def close_if_ours(workspace_id: str, address: str) -> None:
    """The proven primitive: re-check the label immediately before closing,
    and skip (never force) when it no longer matches."""
    result = herdrcli.call_herdr(["workspace", "get", workspace_id], check=False)
    label = result.data.get("workspace", {}).get("label") if result.ok else None
    if label != address:
        return
    herdrcli.call_herdr(["workspace", "close", workspace_id])


def _agent_argv(campaign: Campaign, *, verb: str, conversation_id: str) -> list[str]:
    prompt_path = prompt.prompt_path(campaign.estate, campaign.name, campaign.prompt_files)
    argv = adapters.build_argv(campaign, verb=verb, conversation_id=conversation_id,
                                prompt_file=prompt_path)
    return argv[1:]  # argv[0] is always the kind name, never part of the argv herdr forwards


def _print_plan(*, create_cmd: list[str], agent_cmd: list[str],
                 close_cmd: list[str] | None) -> None:
    if close_cmd:
        print("would close (if a live workspace still carries this address):")
        print("  herdr " + shlex.join(close_cmd))
    print("would create:")
    print("  herdr " + shlex.join(create_cmd))
    print("would start:")
    print("  herdr " + shlex.join(agent_cmd))


def resume(estates: dict, address: str, *, dry_run: bool = False) -> int:
    campaign = campaign_mod.resolve(estates, address)
    conversation_id = continuity.latest_conversation(campaign.dir)
    if conversation_id is None:
        print(f"{address}: no recorded conversation — resume behaves as fresh")
        return fresh(estates, address, dry_run=dry_run)
    if not doubleopen.transcript_exists(conversation_id):
        raise GuardError(f"{address}: conversation {conversation_id[:8]}... is "
                          "recorded but never persisted a transcript")
    if holder := doubleopen.registry_holder(conversation_id):
        raise GuardError(f"{address}: conversation is open as {holder} — "
                          "exit that pane first")

    agent_argv = _agent_argv(campaign, verb="resume", conversation_id=conversation_id)
    create_cmd = create_args(campaign)
    start_cmd = start_args(campaign, PANE_PLACEHOLDER, agent_argv)
    if dry_run:
        _print_plan(create_cmd=create_cmd, agent_cmd=start_cmd, close_cmd=None)
        return 0

    ws = doubleopen.live_workspace(address)
    if ws and doubleopen.workspace_agent_running(ws.get("workspace_id", "")):
        raise GuardError(f"{address}: already open in workspace "
                          f"{ws.get('workspace_id')} with a running agent")

    if campaign.prompt_files:
        prompt.write_prompt(campaign.estate, campaign.name, campaign.prompt_files)
    created = herdrcli.call_herdr(create_cmd)
    workspace_id = created.data.get("workspace", {}).get("workspace_id")
    pane_id = created.data.get("root_pane", {}).get("pane_id")
    herdrcli.call_herdr(start_args(campaign, pane_id, agent_argv))
    print(f"live  {address}  {workspace_id}  {pane_id}  [{conversation_id[:8]}...]")
    return 0


def fresh(estates: dict, address: str, *, dry_run: bool = False) -> int:
    campaign = campaign_mod.resolve(estates, address)
    old_id = continuity.latest_conversation(campaign.dir)
    if old_id and (holder := doubleopen.registry_holder(old_id)):
        raise GuardError(f"{address}: conversation is open as {holder} — "
                          "exit that pane first")

    new_id = str(uuid.uuid4())
    agent_argv = _agent_argv(campaign, verb="fresh", conversation_id=new_id)
    create_cmd = create_args(campaign)
    start_cmd = start_args(campaign, PANE_PLACEHOLDER, agent_argv)
    if dry_run:
        close_cmd = ["workspace", "close", WORKSPACE_PLACEHOLDER] if old_id else None
        _print_plan(create_cmd=create_cmd, agent_cmd=start_cmd, close_cmd=close_cmd)
        return 0

    ws = doubleopen.live_workspace(address)
    if ws and doubleopen.workspace_agent_running(ws.get("workspace_id", "")):
        raise GuardError(f"{address}: already open in workspace "
                          f"{ws.get('workspace_id')} with a running agent — "
                          "exit it first")
    if ws:
        close_if_ours(ws.get("workspace_id", ""), address)

    if campaign.prompt_files:
        prompt.write_prompt(campaign.estate, campaign.name, campaign.prompt_files)
    created = herdrcli.call_herdr(create_cmd)
    workspace_id = created.data.get("workspace", {}).get("workspace_id")
    pane_id = created.data.get("root_pane", {}).get("pane_id")
    herdrcli.call_herdr(start_args(campaign, pane_id, agent_argv))
    continuity.append(campaign.dir, new_id)
    print(f"live  {address}  {workspace_id}  {pane_id}  [{new_id[:8]}...]  (fresh)")
    return 0


def list_campaigns(estates: dict) -> list[dict]:
    rows = []
    for estate_name, row in sorted(estates.items()):
        root_val = row.get("root")
        if not root_val:
            continue
        root = expand(root_val)
        campaigns_dir = root / row.get("campaigns", DEFAULT_CAMPAIGNS_DIR)
        if not campaigns_dir.is_dir():
            continue
        for md in sorted(campaigns_dir.glob("*/campaign.md")):
            address = f"{estate_name}/{md.parent.name}"
            log = continuity.read(md.parent)
            rows.append({"address": address,
                         "last": log[-1].split()[0][:10] if log else ""})
    return rows


def format_row(row: dict) -> str:
    state = f"last {row['last']}" if row["last"] else "new"
    return f"{row['address']:<26} {state}"
