"""Dispatch and exit codes.

0 on success, 2 for usage, 3 not found, 4 auth, 5 refused,
6 retry later, 1 for any other failed request.
"""

from __future__ import annotations

import json
import sys
import urllib.error
from typing import Optional

from ..errors import ApiError
from . import output
from .body import InputError
from .complete import complete, print_completion
from .config import HELP_SPEC, PROGRAM
from .parser import build_parser


def _check_rename(argv: list[str]) -> int:
    tokens = argv
    renames = [
        (("books", "list-books",), "books list"),
    ]
    for retired, replacement in renames:
        if tokens[: len(retired)] == list(retired):
            return output.print_error(
                f"{' '.join(retired)} is now {PROGRAM} {replacement}", code=2
            )
    return 0


def _print_help(argv: list[str]) -> int:
    json_out = output.OUTPUT_FORMAT == "json"
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
    output.resolve_format(False, None)
    value_flags = {"--format", "--fields", "--output", "-o", "--base-url", "--color"}
    for command in json.loads(HELP_SPEC)["commands"]:
        for flag in command["flags"]:
            if flag["type"] != "boolean":
                value_flags.add("--" + flag["name"])
    json_flag = False
    index = 0
    while index < len(argv):
        token = argv[index]
        if token == "--":
            break
        if token == "--json":
            json_flag = True
        if token.startswith("--format="):
            output.resolve_format(False, token.split("=", 1)[1])
        if token in value_flags and index + 1 < len(argv):
            index += 1
            if token == "--format":
                output.resolve_format(False, argv[index])
        index += 1
    if json_flag:
        output.OUTPUT_FORMAT = "json"
    global_names = {
        "--json",
        "--format",
        "--fields",
        "--output",
        "-o",
        "--quiet",
        "-q",
        "--debug",
        "--yes",
        "--no-input",
        "--color",
        "--no-pager",
        "--base-url",
    }
    prefix: list[str] = []
    start = 0
    while start < len(argv):
        token = argv[start]
        name = token.split("=", 1)[0]
        if name not in global_names:
            break
        prefix.append(token)
        start += 1
        if "=" not in token and name in value_flags and start < len(argv):
            prefix.append(argv[start])
            start += 1
    argv = [*argv[start:], *prefix]
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
    if output.OUTPUT_FORMAT == "ai-friendly" and output.OUTPUT_PATH != "-":
        code = output.preflight_output()
        if code:
            return code
    handler = getattr(args, "_handler", None)
    if handler is None:
        if output.OUTPUT_FORMAT not in ("json", "jsonl"):
            parser.print_help(sys.stderr)
        return output.print_error("a command is required", code=2)
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
    except (urllib.error.URLError, ConnectionError, TimeoutError) as exc:
        return output.print_error(str(exc), code=6)
    except OSError as exc:
        return output.print_error(str(exc), code=1)
    except ValueError as exc:
        return output.print_error(f"the response could not be read: {exc}", code=1)
    try:
        output.print_result(result)
    except OSError as exc:
        return output.print_error(str(exc), code=1)
    return 0
