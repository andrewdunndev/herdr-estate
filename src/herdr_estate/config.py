"""The estate table: estates.toml, and the address it's keyed by."""
from __future__ import annotations

import os
import tomllib
from pathlib import Path

from .errors import ConfigError

DEFAULT_CAMPAIGNS_DIR = "campaigns"
DEFAULT_AGENT_KIND = "claude"


def config_path() -> Path:
    """$HERDR_PLUGIN_CONFIG_DIR/estates.toml when herdr sets that (running
    as a plugin action) and the file is actually there; otherwise the
    fallback a bare shell invocation uses."""
    d = os.environ.get("HERDR_PLUGIN_CONFIG_DIR")
    if d:
        p = Path(d) / "estates.toml"
        if p.is_file():
            return p
    return Path.home() / ".config" / "herdr-estate" / "estates.toml"


def load_estates(path: Path | None = None) -> dict[str, dict]:
    """{estate_name: estate_row} from estates.toml's [estate.*] tables."""
    p = path or config_path()
    if not p.is_file():
        raise ConfigError(f"no estate table at {p}")
    with p.open("rb") as f:
        data = tomllib.load(f)
    return data.get("estate", {})


def parse_address(address: str) -> tuple[str, str]:
    estate, sep, campaign = address.partition("/")
    if not sep or not estate or not campaign:
        raise ConfigError(f"address must be 'estate/campaign': {address!r}")
    return estate, campaign
