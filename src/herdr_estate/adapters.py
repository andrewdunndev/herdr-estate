"""Per-kind adapters: turn a resolved Campaign plus a verb into the
agent's own argv. Generic keys map through the small table the design
note gives (paths, prompts, address, conversation id); a kind's own
frontmatter table (`claude:` in campaign.md) passes straight through as
`--<key>` flags, true meaning a bare flag and any other value meaning
`--<key> <value>`.

Shipped: claude. An unknown kind gets no mapped flags at all — one
warning line, then bin/herdr-estate runs `<kind>` with nothing appended
but the bare conversation verb; adding a kind later is adding one more
entry to ADAPTERS.
"""
from __future__ import annotations

import sys

from .campaign import Campaign


def _kind_flags(campaign: Campaign) -> list[str]:
    argv: list[str] = []
    for key, value in campaign.kind_flags.items():
        flag = f"--{key}"
        if value is True:
            argv.append(flag)
        elif value is False:
            continue
        else:
            argv += [flag, str(value)]
    return argv


def claude_argv(campaign: Campaign, *, verb: str, conversation_id: str,
                prompt_file) -> list[str]:
    argv = ["claude"]
    argv += ["--resume", conversation_id] if verb == "resume" else \
            ["--session-id", conversation_id]
    argv += ["--name", campaign.address]
    if prompt_file:
        argv += ["--append-system-prompt-file", str(prompt_file)]
    for d in campaign.add_dirs:
        argv += ["--add-dir", str(d)]
    argv += _kind_flags(campaign)
    return argv


ADAPTERS = {"claude": claude_argv}


def build_argv(campaign: Campaign, *, verb: str, conversation_id: str,
               prompt_file) -> list[str]:
    adapter = ADAPTERS.get(campaign.kind)
    if adapter is None:
        print(f"herdr-estate: no adapter for agent kind '{campaign.kind}' — "
              "starting with no mapped flags", file=sys.stderr)
        return [campaign.kind]
    return adapter(campaign, verb=verb, conversation_id=conversation_id,
                    prompt_file=prompt_file)
