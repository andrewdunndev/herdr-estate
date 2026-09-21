"""bin/herdr-estate's argument parsing and dispatch."""
from __future__ import annotations

import argparse
import sys

from . import config, verbs
from .errors import EstateError, UsageError


def _address(args: argparse.Namespace) -> str:
    if args.address:
        return args.address
    if args.dry_run:
        raise UsageError("--dry-run needs an explicit <address> — deriving one "
                          "from the current workspace would itself call herdr")
    return verbs.derive_address()


def cmd_resume(args: argparse.Namespace) -> int:
    estates = config.load_estates()
    return verbs.resume(estates, _address(args), dry_run=args.dry_run)


def cmd_fresh(args: argparse.Namespace) -> int:
    estates = config.load_estates()
    return verbs.fresh(estates, _address(args), dry_run=args.dry_run)


def cmd_list(args: argparse.Namespace) -> int:
    estates = config.load_estates()
    rows = verbs.list_campaigns(estates)
    if not rows:
        print("no campaigns found")
        return 0
    for row in rows:
        print(verbs.format_row(row))
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="herdr-estate")
    sub = p.add_subparsers(dest="verb", required=True)

    resume_p = sub.add_parser(
        "resume", help="continue a campaign's latest conversation")
    resume_p.add_argument(
        "address", nargs="?",
        help="estate/campaign; derived from the workspace label when omitted")
    resume_p.add_argument("--dry-run", action="store_true",
                           help="print the herdr commands and agent argv; "
                                "create, close or start nothing")
    resume_p.set_defaults(func=cmd_resume)

    fresh_p = sub.add_parser(
        "fresh", help="retire the campaign's latest conversation and start a new one")
    fresh_p.add_argument(
        "address", nargs="?",
        help="estate/campaign; derived from the workspace label when omitted")
    fresh_p.add_argument("--dry-run", action="store_true",
                          help="print the herdr commands and agent argv; "
                               "create, close or start nothing")
    fresh_p.set_defaults(func=cmd_fresh)

    list_p = sub.add_parser("list", help="list every campaign the estate table knows")
    list_p.set_defaults(func=cmd_list)

    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args) or 0
    except EstateError as e:
        print(f"herdr-estate: {e}", file=sys.stderr)
        return 1
