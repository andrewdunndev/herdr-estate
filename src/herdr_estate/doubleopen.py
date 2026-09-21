"""Two independent double-open guards.

registry_holder/transcript_exists read Claude Code's own app-local
state (~/.claude/sessions, ~/.claude/projects) and are ported unchanged
from an existing estate tool's substrate-free half — no herdr, no shell,
just two glob reads. They answer: is *this conversation* already open
in some other live pane, and does it even have a transcript yet.

live_workspace/workspace_agent_running answer a different question over
the herdr CLI: does a live workspace already carry *this address*, and
is its agent still running. `workspace list`'s and `agent list`'s exact
per-item JSON shape is not verified live in this repo (herdr calls here
are --version/--help only); the fields read below follow the one shape
the plugin API survey does confirm (`workspace get` returns
`.result.workspace.label`) and are covered by mocked tests, not a live
run.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

from . import herdrcli


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
    result = herdrcli.call_herdr(["workspace", "list"], check=False)
    if not result.ok:
        return None
    for ws in result.data.get("workspaces", []):
        if ws.get("label") == address:
            return ws
    return None


def workspace_agent_running(workspace_id: str) -> bool:
    """Whether any agent is currently attached to `workspace_id`."""
    result = herdrcli.call_herdr(["agent", "list"], check=False)
    if not result.ok:
        return False
    return any(a.get("workspace_id") == workspace_id
               for a in result.data.get("agents", []))
