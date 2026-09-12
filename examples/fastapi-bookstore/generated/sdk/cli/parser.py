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
    subparsers = parser.add_subparsers(
        dest="_command",
        required=True,
    )
    root.register(subparsers)
    return parser
