"""sessions.log: the continuity record a campaign's resume reads.

One line per conversation: an ISO-8601 UTC timestamp, the hostname that
opened it, and the conversation id, space-separated. This format is not
invented here — it is byte-compatible with an existing estate tool's own
sessions.log writer, so a campaign directory that already has one keeps
resuming its latest conversation unmodified. herdr-estate owns
continuity itself; herdr's own resume is never used (design note,
"Files").
"""
from __future__ import annotations

import socket
from datetime import datetime, timezone
from pathlib import Path


def log_path(campaign_dir: Path) -> Path:
    return campaign_dir / "sessions.log"


def read(campaign_dir: Path) -> list[str]:
    p = log_path(campaign_dir)
    if not p.is_file():
        return []
    return [ln for ln in p.read_text().splitlines() if ln.strip()]


def latest_conversation(campaign_dir: Path) -> str | None:
    lines = read(campaign_dir)
    return lines[-1].split()[-1] if lines else None


def append(campaign_dir: Path, conversation_id: str) -> None:
    campaign_dir.mkdir(parents=True, exist_ok=True)
    with log_path(campaign_dir).open("a") as f:
        f.write(f"{datetime.now(timezone.utc).isoformat()} "
                f"{socket.gethostname()} {conversation_id}\n")
