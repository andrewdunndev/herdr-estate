"""Optional worktrees (design note, "Worktrees: optional").

ensure_worktree is idempotent and additive only. It shells out to plain
git, never `herdr worktree` — that family takes no --env, so the
address could never reach the agent, and it registers a stray base
workspace. It never removes a worktree either: herdr's own
`worktree remove --force` can delete uncommitted work, and removal
stays a human act.
"""
from __future__ import annotations

import subprocess
from pathlib import Path

from .errors import ConfigError
from .paths import expand, state_dir


def _git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args],
                           capture_output=True, text=True, check=check)


def _default_branch(repo: Path) -> str:
    result = _git(repo, "symbolic-ref", "refs/remotes/origin/HEAD", check=False)
    if result.returncode == 0 and result.stdout.strip():
        return result.stdout.strip().rsplit("/", 1)[-1]
    return "main"


def _existing_worktree(repo: Path, branch: str) -> str | None:
    """A path from `git worktree list --porcelain` whose branch matches, or
    None. Porcelain output is blank-line-separated blocks, each starting
    with `worktree <path>`, followed by a `branch refs/heads/<name>` line
    for a normal (non-detached) worktree."""
    listing = _git(repo, "worktree", "list", "--porcelain").stdout
    path = None
    for line in listing.splitlines():
        if line.startswith("worktree "):
            path = line[len("worktree "):]
        elif line.startswith("branch refs/heads/"):
            if path and line[len("branch refs/heads/"):] == branch:
                return path
        elif not line:
            path = None
    return None


def ensure_worktree(repo_path: str, branch: str) -> Path:
    """Idempotent: `git worktree add` only when `branch`'s worktree isn't
    already registered. A new branch is cut from the remote default
    branch when one exists, else from HEAD. Never removes a worktree."""
    repo = expand(repo_path)
    if not repo.is_dir():
        raise ConfigError(f"worktree repo missing: {repo}")
    existing = _existing_worktree(repo, branch)
    if existing:
        return Path(existing)

    _git(repo, "worktree", "prune")
    branch_exists = _git(repo, "show-ref", "--verify", "--quiet",
                          f"refs/heads/{branch}", check=False).returncode == 0
    wt_dir = state_dir() / "worktrees" / repo.name / branch.replace("/", "-")
    wt_dir.parent.mkdir(parents=True, exist_ok=True)
    if branch_exists:
        _git(repo, "worktree", "add", str(wt_dir), branch)
    else:
        base = f"origin/{_default_branch(repo)}"
        base_ok = _git(repo, "rev-parse", "--verify", "--quiet", base,
                        check=False).returncode == 0
        if base_ok:
            _git(repo, "worktree", "add", str(wt_dir), "-b", branch, base)
        else:
            _git(repo, "worktree", "add", str(wt_dir), "-b", branch)
    # git may register the worktree under its realpath (e.g. resolving a
    # /var -> /private/var symlink on macOS): read the path back from git
    # itself so a later idempotent call returns the identical value.
    return Path(_existing_worktree(repo, branch) or wt_dir)
