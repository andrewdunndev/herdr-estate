"""Resolve estate/campaign into a directory, a working directory, and
the pieces a per-kind adapter needs (paths, prompt files, kind flags)."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from . import frontmatter
from . import worktree as worktree_mod
from .config import DEFAULT_AGENT_KIND, DEFAULT_CAMPAIGNS_DIR, parse_address
from .errors import ConfigError
from .paths import expand


@dataclass
class Campaign:
    address: str
    estate: str
    name: str
    dir: Path
    workdir: Path
    kind: str
    add_dirs: list[Path]
    prompt_files: list[Path]
    kind_flags: dict
    env: dict = field(default_factory=dict)


def resolve(estates: dict[str, dict], address: str) -> Campaign:
    estate_name, campaign_name = parse_address(address)
    row = estates.get(estate_name)
    if row is None:
        raise ConfigError(f"no estate '{estate_name}' in the estate table")
    if "root" not in row:
        raise ConfigError(f"estate '{estate_name}' has no 'root'")
    root = expand(row["root"])
    if not root.is_dir():
        raise ConfigError(f"estate '{estate_name}' not cloned at {root}")
    campaigns_dir = root / row.get("campaigns", DEFAULT_CAMPAIGNS_DIR)
    cdir = campaigns_dir / campaign_name
    md = cdir / "campaign.md"
    if not md.is_file():
        raise ConfigError(
            f"no campaign at {cdir} — create {md} first")
    fm = frontmatter.parse_frontmatter(md.read_text())
    kind = fm.get("agent") or row.get("agent", DEFAULT_AGENT_KIND)
    if not isinstance(kind, str):
        kind = DEFAULT_AGENT_KIND
    add_dirs = [expand(p) for p in fm.get("paths", []) if isinstance(p, str) and p]
    prompt_files = [expand(p) for p in row.get("prompts", [])]
    raw_flags = fm.get(kind)
    kind_flags = raw_flags if isinstance(raw_flags, dict) else {}

    # A campaign with no `worktree` key runs in the estate's plain
    # directory. With one, resume/fresh ensure it exists, use it as the
    # workspace cwd, and add it to the agent's paths (design note,
    # "Worktrees: optional").
    workdir = root
    wt = fm.get("worktree")
    if isinstance(wt, dict) and wt.get("repo") and wt.get("branch"):
        workdir = worktree_mod.ensure_worktree(wt["repo"], wt["branch"])
        add_dirs.append(workdir)

    return Campaign(address=address, estate=estate_name, name=campaign_name,
                     dir=cdir, workdir=workdir, kind=kind, add_dirs=add_dirs,
                     prompt_files=prompt_files, kind_flags=kind_flags,
                     env=dict(row.get("env", {})))
