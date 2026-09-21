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


def resolve(estates: dict[str, dict], address: str, *, dry_run: bool = False) -> Campaign:
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

    def _rooted(p: str) -> Path:
        """A relative `paths`/`prompts` entry resolves against the
        estate's root, not this process's cwd; `~` and $VAR expansion
        (paths.expand) still applies first, and an already-absolute
        result is left alone."""
        ep = expand(p)
        return ep if ep.is_absolute() else root / ep

    def _paths(source: dict) -> list[Path]:
        return [_rooted(p) for p in source.get("paths", []) if isinstance(p, str) and p]

    # The estate's paths are defaults: they lead, the campaign's own
    # paths follow, and a path repeated in both (post-expansion) is
    # kept once, at its first position.
    add_dirs: list[Path] = []
    for p in _paths(row) + _paths(fm):
        if p not in add_dirs:
            add_dirs.append(p)

    prompt_files = [_rooted(p) for p in row.get("prompts", [])]

    # Same defaulting for a per-kind table: the estate's `[estate.<name>.<kind>]`
    # entries apply to every campaign in the estate, and the campaign's own
    # `<kind>:` table overrides key by key (an explicit `false` there turns
    # an estate default off).
    estate_flags = row.get(kind)
    campaign_flags = fm.get(kind)
    kind_flags = {
        **(estate_flags if isinstance(estate_flags, dict) else {}),
        **(campaign_flags if isinstance(campaign_flags, dict) else {}),
    }

    # A campaign with no `worktree` key runs in the estate's plain
    # directory. With one, resume/fresh ensure it exists, use it as the
    # workspace cwd, and add it to the agent's paths (design note,
    # "Worktrees: optional"). dry_run never runs `git worktree add` or
    # creates a branch: it only computes and prints the path a real run
    # would use.
    workdir = root
    wt = fm.get("worktree")
    if isinstance(wt, dict) and wt.get("repo") and wt.get("branch"):
        if dry_run:
            workdir = worktree_mod.would_ensure_worktree(wt["repo"], wt["branch"])
            print(f"{address}: would ensure worktree: {workdir}")
        else:
            workdir = worktree_mod.ensure_worktree(wt["repo"], wt["branch"])
        add_dirs.append(workdir)

    return Campaign(address=address, estate=estate_name, name=campaign_name,
                     dir=cdir, workdir=workdir, kind=kind, add_dirs=add_dirs,
                     prompt_files=prompt_files, kind_flags=kind_flags,
                     env=dict(row.get("env", {})))
