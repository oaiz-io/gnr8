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
from .complete import complete, print_completion
from .config import HELP_SPEC, PROGRAM
from .parser import build_parser


def _check_rename(argv: list[str]) -> int:
    tokens = [arg for arg in argv if not arg.startswith("-")]
    renames = [
        (("books", "list-books"), "books list"),
    ]
    for retired, replacement in renames:
        if tokens[: len(retired)] == list(retired):
            print(
                f"error: {' '.join(retired)} is now {PROGRAM} {replacement}",
                file=sys.stderr,
            )
            return 2
    return 0


def _print_help(argv: list[str]) -> int:
    json_out = False
    rest: list[str] = []
    for arg in argv:
        if arg == "--json":
            json_out = True
            continue
        rest.append(arg)
    if json_out:
        print(HELP_SPEC)
        return 0
    parser = build_parser()
    if not rest:
        parser.print_help()
        return 0
    try:
        parser.parse_args([*rest, "--help"])
    except SystemExit as exc:
        if exc.code in (0, None):
            return 0
        return exc.code if isinstance(exc.code, int) else 1
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    try:
        return _main(argv)
    except KeyboardInterrupt:
        print("error: interrupted", file=sys.stderr)
        return 130


def _main(argv: Optional[list[str]]) -> int:
    argv = sys.argv[1:] if argv is None else argv
    code = _check_rename(argv)
    if code:
        return code
    if argv and argv[0] == "help":
        return _print_help(argv[1:])
    if argv and argv[0] == "completion":
        return print_completion(argv[1:])
    if argv and argv[0] == "__complete":
        return complete(argv[1:])
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
    except ApiError as exc:
        code = output.exit_code_for_status(exc.status_code)
        message = f"{exc.message} ({exc.status_code} {exc.slug})"
        if not exc.message and not exc.slug:
            message = f"the API returned {exc.status_code} with a non-JSON body"
            if code == 6:
                message += "; retry later"
        hints = [str(hint) for hint in exc.hints]
        return output.print_error(
            message, hints, exc.request_id, exc.status_code, code, exc.slug
        )
    except InputError as exc:
        return output.print_error(exc.reason, code=2)
    except OSError as exc:
        return output.print_error(str(exc), code=6)
    except ValueError as exc:
        return output.print_error(f"the response could not be read: {exc}", code=1)
    try:
        output.print_result(result)
    except OSError as exc:
        return output.print_error(str(exc), code=1)
    return 0
