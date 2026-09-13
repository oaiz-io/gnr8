"""Dispatch and exit codes.

0 on success, 1 for a failed request, 2 for a usage or input error.
"""

from __future__ import annotations

import sys
from typing import Optional

from ..errors import ApiError
from .body import InputError
from .config import PROGRAM
from .output import print_result
from .parser import build_parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    handler = getattr(args, "_handler", None)
    if handler is None:
        parser.print_help(sys.stderr)
        return 2
    try:
        result = handler(args)
        print_result(result)
        return 0
    except ApiError as exc:
        print(
            f"{PROGRAM}: {exc.status_code} {exc.message} ({exc.slug})",
            file=sys.stderr,
        )
        return 1
    except InputError as exc:
        print(f"{PROGRAM}: {exc.reason}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"{PROGRAM}: {exc}", file=sys.stderr)
        return 1
