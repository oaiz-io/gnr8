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
    parser.add_argument(
        "--fields",
        dest="fields",
        help="comma-separated response fields, or help to list them",
    )
    parser.add_argument(
        "-o",
        "--output",
        dest="output",
        help="write the full result to a file, or - for stdout",
    )
    parser.add_argument(
        "--quiet",
        dest="quiet",
        action="store_true",
        help="print less on success",
    )
    parser.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        help="print less on success",
    )
    parser.add_argument(
        "--debug",
        dest="debug",
        action="store_true",
        help="write a request trace to stderr",
    )
    parser.add_argument(
        "--yes",
        dest="yes",
        action="store_true",
        help="do not ask before a destructive command",
    )
    parser.add_argument(
        "-y",
        dest="yes",
        action="store_true",
        help="do not ask before a destructive command",
    )
    parser.add_argument(
        "--no-input",
        dest="no_input",
        action="store_true",
        help="never prompt; refuse commands that would ask",
    )
    subparsers = parser.add_subparsers(
        dest="_command",
        required=True,
    )
    root.register(subparsers)
    return parser
