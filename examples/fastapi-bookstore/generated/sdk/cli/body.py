"""Reading a request body.

`--body` takes JSON inline; `--body-file` takes a path, or `-` for stdin.
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


class InputError(Exception):
    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


def load_body(args: argparse.Namespace) -> Any:
    raw = getattr(args, "body", None)
    if raw is None:
        path = getattr(args, "body_file", None)
        if path is None:
            return None
        if path == "-":
            raw = sys.stdin.read()
        else:
            try:
                with open(path, encoding="utf-8") as handle:
                    raw = handle.read()
            except OSError as exc:
                raise InputError(f"cannot read {path!r}: {exc}") from None
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise InputError(f"body is not valid JSON: {exc}") from None
