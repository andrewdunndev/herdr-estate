"""Proof this plugin created a workspace.

A label match alone is not proof herdr-estate created a workspace — the
operator could have opened one by hand under the same address. At
create time, resume/fresh record {workspace_id: address} here; a
workspace may be closed only when its id is in this record (verbs.py
also re-checks the label immediately before closing, since ids are
reused across a server restart).

Stored as one JSON object at $HERDR_PLUGIN_STATE_DIR/owned.json
(fallback: paths.state_dir()'s own fallback), written atomically
(temp file + os.replace) so a crash mid-write never corrupts it. A
missing or corrupt file reads as "nothing owned", never an error.
"""
from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

from .paths import state_dir


def _path() -> Path:
    return state_dir() / "owned.json"


def _load() -> dict[str, str]:
    p = _path()
    if not p.is_file():
        return {}
    try:
        data = json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def _save(records: dict[str, str]) -> None:
    p = _path()
    p.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(dir=str(p.parent), prefix=".owned-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(records, f)
        os.replace(tmp_name, p)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def is_ours(workspace_id: str) -> bool:
    """Whether `workspace_id` is one this plugin recorded creating."""
    return workspace_id in _load()


def record(address: str, workspace_id: str) -> None:
    """Record that `workspace_id` was just created for `address`."""
    records = _load()
    records[workspace_id] = address
    _save(records)


def forget(workspace_id: str) -> None:
    """Drop a record, e.g. once its workspace is closed."""
    records = _load()
    if workspace_id in records:
        del records[workspace_id]
        _save(records)


def prune(existing_ids: set[str]) -> None:
    """Drop every record whose workspace_id isn't in `existing_ids` — the
    live set `workspace list` just reported. Residue from a workspace
    closed some other way (by hand, or by a herdr server restart that
    dropped it) never lingers."""
    records = _load()
    kept = {wid: addr for wid, addr in records.items() if wid in existing_ids}
    if kept != records:
        _save(kept)
