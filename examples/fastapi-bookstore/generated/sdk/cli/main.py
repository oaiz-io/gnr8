"""Dispatch and exit codes.

0 on success, 2 for usage, 3 not found, 4 auth, 5 refused,
6 retry later, 1 for any other failed request.
"""

from __future__ import annotations

import sys
from typing import Optional

from ..errors import ApiError
from . import output
from .body import InputError
from .parser import build_parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    output.apply_globals(args)
    if getattr(args, "fields", None) == "help":
        return output.print_fields_help(getattr(args, "_fields", ()))
    if output.OUTPUT_FORMAT == "ai-friendly":
        code = output.preflight_output()
        if code:
            return code
    handler = getattr(args, "_handler", None)
    if handler is None:
        parser.print_help(sys.stderr)
        return 2
    try:
        result = handler(args)
        output.print_result(result)
        return 0
    except ApiError as exc:
        code = output.exit_code_for_status(exc.status_code)
        message = f"{exc.message} ({exc.status_code} {exc.slug})"
        if not exc.message and not exc.slug:
            message = f"the API returned {exc.status_code} with a non-JSON body"
            if exc.status_code >= 500:
                message += "; retry later"
                code = 6
        hints = [str(hint) for hint in exc.hints]
        return output.print_error(message, hints, exc.request_id, exc.status_code, code)
    except InputError as exc:
        return output.print_error(exc.reason, code=2)
    except OSError as exc:
        return output.print_error(str(exc), code=6)
