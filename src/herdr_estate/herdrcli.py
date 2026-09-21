"""The one seam between herdr_estate and the herdr CLI itself.

Every herdr invocation anywhere in this package goes through
``call_herdr()``. Tests replace this one function and never run a real
herdr binary.
"""
from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass, field

from .errors import EstateError

# Dropped from the environment of every herdr call: a stale API key (the
# session should authenticate via the parent shell's own login) and
# TMUX*/CLAUDE* markers that describe *this* process, not the workspace
# herdr is about to create. Nothing under HERDR_* matches either rule, so
# the control socket and the rest of herdr's own plugin env ride through
# untouched.
ENV_SCRUB_EXACT = {"ANTHROPIC_API_KEY"}
ENV_SCRUB_PREFIX = ("TMUX", "CLAUDE")


def scrub_env(env: dict[str, str]) -> dict[str, str]:
    return {k: v for k, v in env.items()
            if k not in ENV_SCRUB_EXACT and not k.startswith(ENV_SCRUB_PREFIX)}


class HerdrError(EstateError):
    """Raised by call_herdr(check=True) on a non-zero herdr exit."""

    def __init__(self, args: list[str], returncode: int, stderr: str, stdout: str):
        self.herdr_args = args
        self.returncode = returncode
        detail = (stderr or stdout or "").strip()[:300]
        super().__init__(f"herdr {' '.join(args)} failed: {detail}")


@dataclass
class HerdrResult:
    ok: bool
    returncode: int
    data: dict = field(default_factory=dict)
    stderr: str = ""


def call_herdr(args: list[str], *, check: bool = True) -> HerdrResult:
    """Run `$HERDR_BIN_PATH <args>`, scrubbing the child's environment,
    and parse its JSON envelope (`{"result": ...}`) into HerdrResult.data.

    check=True (the default) raises HerdrError on a non-zero exit; pass
    check=False for a probe that reports failure as HerdrResult.ok=False
    instead (the label-match-before-close pattern needs this).
    """
    bin_path = os.environ.get("HERDR_BIN_PATH", "herdr")
    proc = subprocess.run([bin_path, *args], capture_output=True, text=True,
                           env=scrub_env(dict(os.environ)))
    ok = proc.returncode == 0
    if check and not ok:
        raise HerdrError(args, proc.returncode, proc.stderr, proc.stdout)
    data: dict = {}
    if ok and proc.stdout.strip():
        try:
            payload = json.loads(proc.stdout)
        except json.JSONDecodeError:
            payload = None
        if isinstance(payload, dict):
            result = payload.get("result", payload)
            data = result if isinstance(result, dict) else {}
    return HerdrResult(ok=ok, returncode=proc.returncode, data=data, stderr=proc.stderr)
