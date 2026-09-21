"""The exception types cli.py catches at the top level."""
from __future__ import annotations


class EstateError(Exception):
    """Base for every error this package raises on purpose (as opposed to
    a herdr failure, which is herdrcli.HerdrError)."""


class ConfigError(EstateError):
    """The estate table, or an estate/campaign it names, can't be read."""


class UsageError(EstateError):
    """The command line or the environment doesn't give enough to act on."""


class GuardError(EstateError):
    """A double-open or label-match guard refused to proceed."""
