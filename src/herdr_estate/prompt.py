"""Assemble an estate's prompt files into ONE file: repeated
--append-system-prompt-file is last-wins on claude, so herdr-estate
passes exactly one, concatenated in the estate table's own order."""
from __future__ import annotations

from pathlib import Path

from .errors import ConfigError
from .paths import state_dir


def prompt_path(estate: str, campaign: str, prompt_files: list[Path]) -> Path | None:
    """Where the assembled prompt would land — a pure read, no I/O, so a
    dry-run can name the flag it would pass without writing anything."""
    if not prompt_files:
        return None
    return state_dir() / f"{estate}-{campaign}.prompt.md"


def write_prompt(estate: str, campaign: str, prompt_files: list[Path]) -> Path | None:
    out = prompt_path(estate, campaign, prompt_files)
    if out is None:
        return None
    parts = []
    for f in prompt_files:
        if not f.is_file():
            raise ConfigError(f"prompt file missing: {f}")
        parts.append(f.read_text().strip())
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n\n".join(parts) + "\n")
    return out
