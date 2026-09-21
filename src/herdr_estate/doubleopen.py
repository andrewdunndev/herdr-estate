"""Two independent double-open guards, plus the live-workspace lookup
ownership.py and the alive check build on.

registry_holder/transcript_exists read Claude Code's own app-local
state (~/.claude/sessions, ~/.claude/projects) and are ported unchanged
from an existing estate tool's substrate-free half — no herdr, no shell,
just two glob reads. They answer: is *this conversation* already open
in some other live pane, and does it even have a transcript yet. Both
are claude-specific; verbs.py only calls them for a campaign whose
agent kind is "claude".

live_workspace answers a different question over the herdr CLI: does a
live workspace already carry *this address*. `workspace list` returns
`{"result": {"workspaces": [{"workspace_id", "label", "agent_status",
...}]}}`; a non-zero exit (call_herdr's default check=True) or a 0 exit
whose JSON didn't parse into a `workspaces` array both raise
ProtocolError — an error that stops the verb, never a silent "no
workspace, carry on". Every call also prunes ownership.py's record to
the workspace ids this listing still reports, since this is the one
place the full listing is already in hand.

workspace_alive reads `agent_status` off the row live_workspace already
fetched — no separate `agent list` call. "working", "idle" and "done"
mean an agent is attached to the workspace; "unknown" means a bare
shell with none. It fails closed: a missing key, a null, or any value
other than exactly "unknown" counts as alive.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

from . import herdrcli, ownership
from .errors import ProtocolError


def registry_holder(conversation_id: str) -> str | None:
    """Where `conversation_id` is currently open, if anywhere. A registry
    entry whose pid is dead is crash residue, not a live holder."""
    sessions_dir = Path.home() / ".claude" / "sessions"
    if not sessions_dir.is_dir():
        return None
    for f in sessions_dir.glob("*.json"):
        try:
            d = json.loads(f.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if d.get("sessionId") != conversation_id:
            continue
        try:
            os.kill(int(f.stem), 0)
        except (ValueError, ProcessLookupError, PermissionError):
            continue
        return f"{d.get('name') or 'unnamed'} (pid {f.stem})"
    return None


def transcript_exists(conversation_id: str) -> bool:
    """A conversation persists only once it has at least one turn."""
    return bool(list((Path.home() / ".claude" / "projects").glob(f"*/{conversation_id}.jsonl")))


def live_workspace(address: str) -> dict | None:
    """The workspace herdr already has open under `address`, if any."""
    result = herdrcli.call_herdr(["workspace", "list"])
    workspaces = result.data.get("workspaces")
    if not isinstance(workspaces, list):
        raise ProtocolError("workspace list: response carried no 'workspaces' array")
    ownership.prune({w["workspace_id"] for w in workspaces if w.get("workspace_id")})
    for ws in workspaces:
        if ws.get("label") == address:
            return ws
    return None


def workspace_alive(ws: dict) -> bool:
    """Fail closed: anything other than exactly "unknown" counts as
    alive, including a missing key, a null, or an unrecognised value."""
    return ws.get("agent_status") != "unknown"
