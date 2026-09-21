"""A minimal, tolerant reader for campaign.md's YAML frontmatter.

Not a YAML parser: it understands exactly the three shapes a campaign
file uses — a top-level scalar (`agent: claude`), a list (flow
`paths: [a, b]` or block `paths:` / `  - a`), and one level of nested
table for a per-kind flag block (`claude:` / `  remote-control: true`).
Unknown top-level keys are kept, not rejected — an older or newer
campaign file, or a key some other tool reads, never breaks the parse.
"""
from __future__ import annotations

import re

_TOP = re.compile(r"^([A-Za-z_][\w-]*):\s*(.*)$")
_LIST_ITEM = re.compile(r"^\s+-\s+(.*)$")
_TABLE_ITEM = re.compile(r"^\s+([\w-]+):\s*(.*)$")


def _strip(v: str) -> str:
    """Drop a trailing ` # comment` and surrounding quotes."""
    v = v.split(" #", 1)[0].strip()
    if len(v) >= 2 and v[0] == v[-1] and v[0] in "\"'":
        v = v[1:-1]
    return v


def _scalar(v: str) -> bool | str:
    s = _strip(v)
    if s == "true":
        return True
    if s == "false":
        return False
    return s


def parse_frontmatter(text: str) -> dict:
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    data: dict = {}
    key: str | None = None
    mode: str | None = None  # "pending" | "list" | "table"
    for raw in lines[1:]:
        if raw.strip() == "---":
            break
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        if not raw[0].isspace():
            m = _TOP.match(raw)
            if not m:
                continue
            key, rest = m.group(1), m.group(2).split(" #", 1)[0].strip()
            if rest.startswith("[") and rest.endswith("]"):
                inner = rest[1:-1].strip()
                data[key] = [_strip(x) for x in inner.split(",")] if inner else []
                mode = None
            elif rest:
                data[key] = _scalar(rest)
                mode = None
            else:
                data[key] = {}
                mode = "pending"
            continue
        if key is None:
            continue
        item = _LIST_ITEM.match(raw)
        if item:
            if mode == "pending":
                data[key] = []
                mode = "list"
            if mode == "list":
                data[key].append(_strip(item.group(1)))
            continue
        sub = _TABLE_ITEM.match(raw)
        if sub:
            if mode == "pending":
                data[key] = {}
                mode = "table"
            if mode == "table":
                data[key][sub.group(1)] = _scalar(sub.group(2))
    return data
