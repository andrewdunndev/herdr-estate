"""Small path helpers shared across the package."""
from __future__ import annotations

import os
from pathlib import Path


def expand(p: str | Path) -> Path:
    """`~` and `$VAR` expansion, the same tolerance campaign files and
    the estate table both rely on."""
    return Path(os.path.expandvars(str(p))).expanduser()


def state_dir() -> Path:
    """Where this plugin's own runtime files (assembled prompts, worktree
    checkouts) live: $HERDR_PLUGIN_STATE_DIR when herdr sets it (running
    as a plugin action), else a plain XDG-ish fallback for a bare shell
    invocation."""
    d = os.environ.get("HERDR_PLUGIN_STATE_DIR")
    if d:
        return Path(d)
    xdg = os.environ.get("XDG_STATE_HOME")
    base = Path(xdg) if xdg else Path.home() / ".local" / "state"
    return base / "herdr-estate"
