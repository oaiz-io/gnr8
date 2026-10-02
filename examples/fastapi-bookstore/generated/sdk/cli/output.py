"""Rendering a result on stdout and an error on stderr.

`--format` selects JSON indent; `--json` is the JSON shorthand.
Errors print `error:` plus optional hints and a request id, at most six lines.
"""

from __future__ import annotations

import json
import os
import sys
from typing import Any, Optional

from pydantic import BaseModel

from .config import FORMAT_ENV


OUTPUT_FORMAT = ""


def _jsonable(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return value.model_dump(mode="json")
    if isinstance(value, list):
        return [_jsonable(item) for item in value]
    if isinstance(value, dict):
        return {key: _jsonable(item) for key, item in value.items()}
    return value


def resolve_format(json_flag: bool, format_flag: Optional[str]) -> None:
    global OUTPUT_FORMAT
    if json_flag:
        OUTPUT_FORMAT = "json"
        return
    if format_flag:
        OUTPUT_FORMAT = format_flag
        return
    env = os.getenv(FORMAT_ENV, "")
    if env in ("human", "ai-friendly", "json", "jsonl"):
        OUTPUT_FORMAT = env
        return
    OUTPUT_FORMAT = "human" if sys.stdout.isatty() else "ai-friendly"


def exit_code_for_status(status: int) -> int:
    if status in (404, 410):
        return 3
    if status in (401, 403):
        return 4
    if status in (400, 409, 412, 422):
        return 5
    if status in (408, 429, 502, 503, 504):
        return 6
    return 1


def kind_for_exit(code: int) -> str:
    if code == 2:
        return "usage"
    if code == 3:
        return "not_found"
    if code == 4:
        return "auth"
    if code == 5:
        return "refused"
    if code == 6:
        return "retry"
    return "error"


def print_error(
    message: str,
    hints: Optional[list[str]] = None,
    request_id: str = "",
    status: int = 0,
    code: int = 1,
) -> int:
    hints = hints or []
    if OUTPUT_FORMAT in ("json", "jsonl"):
        body: dict[str, Any] = {
            "exitCode": code,
            "kind": kind_for_exit(code),
            "message": message,
        }
        if status:
            body["status"] = status
        if hints:
            body["hints"] = hints
        if request_id:
            body["requestId"] = request_id
        print(json.dumps({"error": body}, separators=(",", ":")), file=sys.stderr)
        return code
    print(f"error: {message}", file=sys.stderr)
    n = 1
    for hint in hints:
        if n >= 6:
            break
        print(f"  hint: {hint}", file=sys.stderr)
        n += 1
    if request_id and n < 6:
        print(f"  request id: {request_id}", file=sys.stderr)
    return code


def print_result(result: Any) -> None:
    if isinstance(result, (bytes, bytearray)):
        sys.stdout.buffer.write(result)
        return
    indent: Optional[int] = None if OUTPUT_FORMAT == "jsonl" else 2
    json.dump(_jsonable(result), sys.stdout, indent=indent)
    sys.stdout.write("\n")
