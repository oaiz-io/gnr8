"""The root argument parser; each command group registers its own subparsers."""

from __future__ import annotations

import argparse

from .commands import root
from .config import DESCRIPTION, PROGRAM, VERSION


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=PROGRAM,
        description=DESCRIPTION,
    )
    parser.add_argument(
        "--version",
        action="version",
        version=VERSION,
    )
    parser.add_argument(
        "--json",
        dest="json",
        action="store_true",
        help="print the server body (shorthand for --format json)",
    )
    parser.add_argument(
        "--format",
        dest="format",
        choices=("human", "ai-friendly", "json", "jsonl"),
        help="output format: human, ai-friendly, json, or jsonl",
    )
    subparsers = parser.add_subparsers(
        dest="_command",
        required=True,
    )
    root.register(subparsers)
    return parser
