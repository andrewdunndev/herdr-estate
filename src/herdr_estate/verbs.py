"""resume, fresh and list: the three actions, and the herdr command
shapes behind them.

Every workspace is created with --label <address>, --no-focus, and
--env HERDR_ESTATE_ADDRESS=<address> plus the estate's own env table
(design note, "Behaviour"). Both resume and fresh close and recreate a
live workspace under the same address when it's ours (ownership.py) and
not alive (doubleopen.workspace_alive) — the label match herdr's ids
being reused across a server restart could otherwise misidentify is
never enough on its own: a workspace not in the ownership record is
left alone, and both verbs refuse instead of creating a second one
under the same label. Either verb refuses outright when the workspace
is alive. --dry-run runs every one of these read-only checks — they're
what decides whether the printed plan would create alone or close-then-
create — and only skips the herdr calls that actually create, close or
start something.
"""
from __future__ import annotations

import os
import re
import shlex
import sys
import uuid

from . import campaign as campaign_mod
from . import adapters, continuity, doubleopen, herdrcli, ownership, prompt
from .campaign import Campaign
from .config import DEFAULT_CAMPAIGNS_DIR
from .errors import GuardError, ProtocolError, UsageError
from .herdrcli import HerdrResult
from .paths import expand

AGENT_START_TIMEOUT_MS = "30000"
PANE_PLACEHOLDER = "<PANE_ID>"

# Emptied on every workspace create (--env KEY=), before the estate's own
# env pairs so a user who deliberately sets one of these in their estate
# table wins. The agent inherits the environment of the workspace's
# root shell, which herdr's server spawns — not this process's — so
# there is no env -u to reach for; these three names are the ones that
# describe *this* process (an API key it authenticated with, tmux
# markers for a pane the workspace didn't create) rather than the
# workspace about to exist.
WORKSPACE_ENV_SCRUB = ("ANTHROPIC_API_KEY", "TMUX", "TMUX_PANE")


def agent_name(address: str) -> str:
    n = re.sub(r"[^a-z0-9_-]", "-", address.lower()).lstrip("-") or "estate"
    return (n if n[0].isalpha() else "a" + n)[:32]


def create_args(campaign: Campaign, *, redact: bool = False) -> list[str]:
    """redact=True is for display only (--dry-run): every estate-table env
    value is masked to `KEY=<from estates.toml>` so a printed plan never
    leaks a secret. HERDR_ESTATE_ADDRESS is derived, not from the
    table, and always stays in the clear."""
    args = ["workspace", "create", "--cwd", str(campaign.workdir),
            "--label", campaign.address, "--no-focus",
            "--env", f"HERDR_ESTATE_ADDRESS={campaign.address}"]
    for k in WORKSPACE_ENV_SCRUB:
        args += ["--env", f"{k}="]
    for k, v in campaign.env.items():
        shown = "<from estates.toml>" if redact else v
        args += ["--env", f"{k}={shown}"]
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
        print("would close:")
        print("  herdr " + shlex.join(close_cmd))
    print("would create:")
    print("  herdr " + shlex.join(create_cmd))
    print("would start:")
    print("  herdr " + shlex.join(agent_cmd))


def _created_ids(created: HerdrResult) -> tuple[str, str]:
    """Validate `workspace create`'s response before using it: a
    missing workspace or pane id is a ProtocolError naming the field,
    never a raw TypeError two calls later."""
    workspace_id = created.data.get("workspace", {}).get("workspace_id")
    if not workspace_id:
        raise ProtocolError("workspace create: response missing workspace.workspace_id")
    pane_id = created.data.get("root_pane", {}).get("pane_id")
    if not pane_id:
        raise ProtocolError("workspace create: response missing root_pane.pane_id")
    return workspace_id, pane_id


def _check_live_workspace(address: str) -> dict | None:
    """The one read both resume and fresh need before deciding anything:
    is a workspace already live under `address`, and if so is it safe to
    replace. Returns the workspace row when it exists, is not alive
    (doubleopen.workspace_alive) and is ours (ownership.is_ours) — the
    case both verbs close and recreate over. Returns None when nothing is
    live. Raises GuardError otherwise: the workspace is alive (refuse,
    regardless of ownership), or it's not alive but isn't ours (refuse;
    nothing is closed, nothing is created). Runs the same way under
    --dry-run, so a dry run's plan and refusals both match the real run;
    only the mutating herdr calls further down are skipped."""
    ws = doubleopen.live_workspace(address)
    if ws is None:
        return None
    if doubleopen.workspace_alive(ws):
        raise GuardError(f"{address}: already open in workspace "
                          f"{ws.get('workspace_id')} with a running agent")
    workspace_id = ws.get("workspace_id", "")
    if not ownership.is_ours(workspace_id):
        raise GuardError(
            f"{address}: a workspace with this label exists that herdr-estate "
            "did not create — close it yourself")
    return ws


def _open_workspace(campaign: Campaign, address: str, ws: dict | None,
                     create_cmd: list[str], agent_argv: list[str]) -> tuple[str, str]:
    """Everything that can fail without herdr (prompt files exist) is
    validated before `ws` (an owned, non-alive workspace) is closed.
    Creates the replacement, validates the response, records ownership
    of the new workspace, and starts the agent."""
    if campaign.prompt_files:
        prompt.write_prompt(campaign.estate, campaign.name, campaign.prompt_files)
    if ws is not None:
        workspace_id = ws.get("workspace_id", "")
        close_if_ours(workspace_id, address)
        ownership.forget(workspace_id)
    created = herdrcli.call_herdr(create_cmd)
    workspace_id, pane_id = _created_ids(created)
    ownership.record(address, workspace_id)
    herdrcli.call_herdr(start_args(campaign, pane_id, agent_argv))
    return workspace_id, pane_id


def resume(estates: dict, address: str, *, dry_run: bool = False) -> int:
    campaign = campaign_mod.resolve(estates, address, dry_run=dry_run)
    conversation_id = continuity.latest_conversation(campaign.dir)
    if conversation_id is None:
        print(f"{address}: no recorded conversation — resume behaves as fresh")
        return fresh(estates, address, dry_run=dry_run)
    if campaign.kind == "claude":
        if not doubleopen.transcript_exists(conversation_id):
            raise GuardError(f"{address}: conversation {conversation_id[:8]}... is "
                              "recorded but never persisted a transcript")
        if holder := doubleopen.registry_holder(conversation_id):
            raise GuardError(f"{address}: conversation is open as {holder} — "
                              "exit that pane first")

    agent_argv = _agent_argv(campaign, verb="resume", conversation_id=conversation_id)
    create_cmd = create_args(campaign)
    start_cmd = start_args(campaign, PANE_PLACEHOLDER, agent_argv)

    ws = _check_live_workspace(address)

    if dry_run:
        close_cmd = ["workspace", "close", ws["workspace_id"]] if ws else None
        _print_plan(create_cmd=create_args(campaign, redact=True),
                    agent_cmd=start_cmd, close_cmd=close_cmd)
        return 0

    workspace_id, pane_id = _open_workspace(campaign, address, ws, create_cmd, agent_argv)
    print(f"live  {address}  {workspace_id}  {pane_id}  [{conversation_id[:8]}...]")
    return 0


def fresh(estates: dict, address: str, *, dry_run: bool = False) -> int:
    campaign = campaign_mod.resolve(estates, address, dry_run=dry_run)
    old_id = continuity.latest_conversation(campaign.dir)
    if old_id and campaign.kind == "claude" and (holder := doubleopen.registry_holder(old_id)):
        raise GuardError(f"{address}: conversation is open as {holder} — "
                          "exit that pane first")

    new_id = str(uuid.uuid4())
    agent_argv = _agent_argv(campaign, verb="fresh", conversation_id=new_id)
    create_cmd = create_args(campaign)
    start_cmd = start_args(campaign, PANE_PLACEHOLDER, agent_argv)

    ws = _check_live_workspace(address)

    if dry_run:
        close_cmd = ["workspace", "close", ws["workspace_id"]] if ws else None
        _print_plan(create_cmd=create_args(campaign, redact=True),
                    agent_cmd=start_cmd, close_cmd=close_cmd)
        return 0

    workspace_id, pane_id = _open_workspace(campaign, address, ws, create_cmd, agent_argv)
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
