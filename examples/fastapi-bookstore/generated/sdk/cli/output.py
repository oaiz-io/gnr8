"""Rendering a result on stdout and an error on stderr.

`--format` selects human, ai-friendly, json, or jsonl; `--json` is the JSON shorthand.
Errors print `error:` plus optional hints and a request id, at most six lines.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from pydantic import BaseModel

from .config import (
    DEBUG_ENV,
    FORMAT_ENV,
    NO_INPUT_ENV,
    OUTPUT_DIR_ENV,
    PAGER_ENV,
    PROGRAM,
    VERSION,
)

OUTPUT_FORMAT = ""
FIELDS = ""
OUTPUT_PATH = ""
QUIET = False
DEBUG = False
YES = False
NO_INPUT = False
COMMAND_PATH = ""
COLOR_MODE = "auto"
NO_PAGER = False
LAST_ANSWER: dict[str, Any] = {}


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
    print(f"{colorize('31', 'error:')} {message}", file=sys.stderr)
    n = 1
    for hint in hints:
        if n >= 6:
            break
        print(f"  hint: {hint}", file=sys.stderr)
        n += 1
    if request_id and n < 6:
        print(f"  request id: {request_id}", file=sys.stderr)
    return code


def use_color() -> bool:
    if OUTPUT_FORMAT and OUTPUT_FORMAT != "human":
        return False
    if COLOR_MODE == "always":
        return True
    if COLOR_MODE == "never":
        return False
    if os.getenv("NO_COLOR"):
        return False
    if os.getenv("TERM") == "dumb":
        return False
    return sys.stdout.isatty()


def colorize(code: str, text: str) -> str:
    if not use_color():
        return text
    return f"\x1b[{code}m{text}\x1b[0m"


def term_width() -> int:
    raw = os.getenv("COLUMNS", "")
    if raw:
        try:
            width = int(raw)
        except ValueError:
            width = 0
        if width >= 20:
            return width
    return 80


def fit_width(text: str) -> str:
    width = term_width()
    lines = []
    for line in text.splitlines():
        if len(line) > width:
            if width > 1:
                line = line[: width - 1] + "…"
            else:
                line = "…"
        lines.append(line)
    return "\n".join(lines) + "\n"


def write_human(text: str) -> None:
    text = fit_width(text.rstrip("\n") + "\n")
    if not NO_PAGER and sys.stdout.isatty() and text.count("\n") >= 24:
        pager = os.getenv(PAGER_ENV) or os.getenv("PAGER") or "less -FIRX"
        try:
            subprocess.run(
                ["sh", "-c", pager],
                input=text.encode(),
                stdout=sys.stdout,
                stderr=sys.stderr,
                check=True,
            )
            return
        except (OSError, subprocess.CalledProcessError):
            pass
    sys.stdout.write(text)


def progress_fetched(count: int) -> None:
    if sys.stderr.isatty():
        print(f"fetched {count} items…", file=sys.stderr)


def capture_response(ctx: Any) -> None:
    headers = getattr(ctx, "response_headers", None) or {}
    request_id = headers.get("X-Request-ID") or headers.get("X-Request-Id", "")
    LAST_ANSWER.clear()
    LAST_ANSWER.update(
        {
            "body": getattr(ctx, "response_body", b"") or b"",
            "status": getattr(ctx, "status", 0) or 0,
            "request_id": request_id,
            "content_type": headers.get("Content-Type", ""),
            "method": getattr(ctx, "method", ""),
            "url": getattr(ctx, "url", ""),
        }
    )
    if DEBUG:
        line = (
            f"debug: {LAST_ANSWER['method']} {LAST_ANSWER['url']}"
            f" -> {LAST_ANSWER['status']}"
        )
        if request_id:
            line += f" request-id={request_id}"
        print(line, file=sys.stderr)


def apply_globals(args: Any) -> None:
    global OUTPUT_FORMAT, FIELDS, OUTPUT_PATH, QUIET, DEBUG, YES
    global NO_INPUT, COMMAND_PATH, COLOR_MODE, NO_PAGER
    resolve_format(
        bool(getattr(args, "json", False)),
        getattr(args, "format", None),
    )
    fields = getattr(args, "fields", None)
    if fields:
        FIELDS = fields
    output = getattr(args, "output", None)
    if output:
        OUTPUT_PATH = output
    if getattr(args, "quiet", False):
        QUIET = True
    if getattr(args, "debug", False):
        DEBUG = True
    if getattr(args, "yes", False):
        YES = True
    if getattr(args, "no_input", False):
        NO_INPUT = True
    color = getattr(args, "color", None)
    if color:
        COLOR_MODE = color
    if getattr(args, "no_pager", False):
        NO_PAGER = True
    COMMAND_PATH = getattr(args, "_command", "") or ""
    if os.getenv(DEBUG_ENV):
        DEBUG = True
    if os.getenv(NO_INPUT_ENV):
        NO_INPUT = True


def print_fields_help(names: tuple[str, ...]) -> int:
    if not names:
        print("no declared response fields")
        return 0
    for name in names:
        print(name)
    return 0


def result_bytes(result: Any) -> bytes:
    body = LAST_ANSWER.get("body")
    if isinstance(body, (bytes, bytearray)) and body:
        return bytes(body)
    if result is None:
        return b""
    if isinstance(result, (bytes, bytearray)):
        return bytes(result)
    return json.dumps(_jsonable(result)).encode()


def field_list() -> list[str]:
    if not FIELDS or FIELDS == "help":
        return []
    return [part.strip() for part in FIELDS.split(",") if part.strip()]


def project_value(value: Any, fields: list[str]) -> Any:
    if isinstance(value, list):
        return [project_value(item, fields) for item in value]
    if isinstance(value, dict):
        return {key: value[key] for key in fields if key in value}
    return value


def decode_result(result: Any) -> tuple[Any, bytes]:
    raw = result_bytes(result)
    if not raw:
        return None, raw
    try:
        return json.loads(raw), raw
    except json.JSONDecodeError:
        return None, raw


def list_items(value: Any) -> tuple[Optional[list[Any]], str, dict[str, Any]]:
    if isinstance(value, list):
        return value, "", {}
    if isinstance(value, dict):
        best_key = ""
        best: Optional[list[Any]] = None
        for key, item in value.items():
            if isinstance(item, list) and (best is None or len(item) > len(best)):
                best_key = key
                best = item
        if best is None:
            return None, "", {}
        meta = {key: item for key, item in value.items() if key != best_key}
        return best, best_key, meta
    return None, "", {}


def _write_stdout(raw: bytes) -> None:
    buffer = getattr(sys.stdout, "buffer", None)
    if buffer is not None:
        buffer.write(raw)
        if raw and not raw.endswith(b"\n"):
            buffer.write(b"\n")
        return
    text = raw.decode()
    sys.stdout.write(text)
    if text and not text.endswith("\n"):
        sys.stdout.write("\n")


def print_json(result: Any) -> None:
    raw = result_bytes(result)
    if not raw:
        return
    if sys.stdout.isatty():
        try:
            parsed = json.loads(raw)
            json.dump(parsed, sys.stdout, indent=2)
            sys.stdout.write("\n")
            return
        except json.JSONDecodeError:
            pass
    _write_stdout(raw)


def print_jsonl(result: Any) -> None:
    value, _raw = decode_result(result)
    items, _key, _meta = list_items(value)
    if items is None:
        items = [value] if value is not None else []
    fields = field_list()
    for item in items:
        projected = project_value(item, fields) if fields else item
        json.dump(projected, sys.stdout, separators=(",", ":"))
        sys.stdout.write("\n")


def print_human(result: Any) -> None:
    value, raw = decode_result(result)
    if not raw:
        return
    fields = field_list()
    payload: Any = project_value(value, fields) if fields else value
    if payload is None:
        write_human(raw.decode())
        return
    buf = io.StringIO()
    json.dump(payload, buf, indent=2)
    buf.write("\n")
    write_human(buf.getvalue())


def output_dir_path() -> Path:
    env = os.getenv(OUTPUT_DIR_ENV, "")
    if env:
        return Path(env)
    return Path(f".{PROGRAM}") / "output"


def preflight_output() -> int:
    directory = output_dir_path()
    root = directory.parent
    try:
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        gitignore = root / ".gitignore"
        if not gitignore.exists():
            gitignore.write_bytes(b"*\n")
            gitignore.chmod(0o600)
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix=".preflight-", dir=directory)
        os.close(fd)
        os.remove(name)
    except OSError as exc:
        print(
            f"error: cannot write {directory}: {exc}. Set {OUTPUT_DIR_ENV} "
            "to a writable directory, or pass --json to print the full "
            "result.",
            file=sys.stderr,
        )
        return 2
    return 0


def _atomic_write(path: Path, body: bytes) -> None:
    fd, name = tempfile.mkstemp(prefix=".tmp-", dir=path.parent)
    try:
        os.write(fd, body)
        os.fsync(fd)
    finally:
        os.close(fd)
    os.chmod(name, 0o600)
    os.replace(name, path)


def _short_id(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()[:6]


def _prune_output(directory: Path) -> None:
    files = [path for path in directory.glob("*.json") if path.name != "latest.json"]
    files.sort(key=lambda path: path.stat().st_mtime)
    total = sum(path.stat().st_size for path in files)
    while files and (len(files) > 100 or total > 100 * 1024 * 1024):
        oldest = files.pop(0)
        total -= oldest.stat().st_size if oldest.exists() else 0
        try:
            oldest.unlink()
        except OSError:
            pass


def write_envelope(result: Any, value: Any, raw: bytes) -> tuple[str, str]:
    directory = output_dir_path()
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    kind = "object"
    items = None
    meta = None
    data = None
    file_meta = None
    page: dict[str, Any] = {}
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        kind = "file"
        name = COMMAND_PATH.replace(" ", "-") or "download"
        bin_path = directory / f"{name}-{_short_id(bytes(result))}.bin"
        bin_path.write_bytes(bytes(result))
        bin_path.chmod(0o600)
        file_meta = {
            "path": str(bin_path),
            "bytes": len(result),
            "contentType": LAST_ANSWER.get("content_type", ""),
            "sha256": hashlib.sha256(bytes(result)).hexdigest(),
        }
    elif not raw:
        kind = "empty"
    else:
        listed, key, listed_meta = list_items(value)
        if listed is not None:
            kind = "list"
            items = listed
            meta = listed_meta
            page["count"] = len(listed)
            if key:
                page["itemsKey"] = key
        else:
            data = value
    stem = COMMAND_PATH.replace(" ", "-") or "result"
    ident = _short_id(raw or b"empty")
    path = directory / f"{stem}-{ident}.json"
    payload: dict[str, Any] = {
        "schema": "https://gnr8.dev/schemas/cli-result-v1.json",
        "version": 1,
        "tool": {"name": PROGRAM, "version": VERSION},
        "command": {"path": COMMAND_PATH},
        "request": {
            "method": LAST_ANSWER.get("method", ""),
            "url": LAST_ANSWER.get("url", ""),
        },
        "response": {
            "status": LAST_ANSWER.get("status", 0),
            "requestId": LAST_ANSWER.get("request_id", ""),
            "contentType": LAST_ANSWER.get("content_type", ""),
            "bytes": len(raw),
        },
        "kind": kind,
        "savedAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    if items is not None:
        payload["items"] = items
    if meta:
        payload["meta"] = meta
    if data is not None:
        payload["data"] = data
    if page:
        payload["page"] = page
    if file_meta is not None:
        payload["file"] = file_meta
    encoded = json.dumps(payload, indent=2).encode() + b"\n"
    _atomic_write(path, encoded)
    _atomic_write(directory / "latest.json", encoded)
    _prune_output(directory)
    return str(path), ""


def _shell_quote(value: str) -> str:
    if not value:
        return "''"
    if all(ch not in value for ch in " \t\n'\"\\$`"):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def _view_row(item: Any) -> str:
    fields = field_list()
    if fields:
        item = project_value(item, fields)
    elif isinstance(item, dict):
        keys = [
            key
            for key, val in item.items()
            if val is None or isinstance(val, (str, int, float, bool))
        ]
        keys.sort()
        keys = keys[:6]
        if keys:
            item = project_value(item, keys)
    encoded = json.dumps(item, separators=(",", ":"))
    if len(encoded) > 80:
        return encoded[:79] + "…"
    return encoded


def print_ai_friendly(result: Any) -> None:
    value, raw = decode_result(result)
    saved = ""
    save_err = ""
    try:
        saved, save_err = write_envelope(result, value, raw)
    except OSError as exc:
        save_err = str(exc)
    listed, key, _meta = list_items(value)
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        outcome = f"saved {len(result)} bytes"
        rows: list[str] = []
        next_page = ""
    elif not raw:
        outcome = "empty"
        rows = []
        next_page = ""
    elif listed is not None:
        noun = key or "items"
        outcome = f"{len(listed)} {noun}"
        rows = [_view_row(item) for item in listed]
        cursor = ""
        if isinstance(value, dict):
            raw_cursor = value.get("nextCursor")
            if not isinstance(raw_cursor, str):
                raw_cursor = value.get("next_cursor")
            if isinstance(raw_cursor, str):
                cursor = raw_cursor
        next_page = ""
        if cursor:
            next_page = (
                f"Next page: {PROGRAM} {COMMAND_PATH} --cursor {cursor}"
                f"    Every page: {PROGRAM} {COMMAND_PATH} --all"
            )
    else:
        outcome = "ok"
        rows = [_view_row(value)]
        next_page = ""
    full = f"not saved ({save_err})" if save_err else saved
    line1 = f"{PROGRAM} {COMMAND_PATH}: {outcome}. Full JSON: {full}"
    chunks = [line1]
    if not QUIET:
        budget = 4000
        shown = 0
        size = len(line1) + 1
        for row in rows:
            extra = len(row) + 1
            if size + extra > budget and shown > 0:
                chunks.append(
                    f"Showing {shown} of {len(rows)}; the rest is in the file."
                )
                break
            chunks.append(row)
            shown += 1
            size += extra
        if next_page and size + len(next_page) + 1 <= budget:
            chunks.append(next_page)
            size += len(next_page) + 1
        if saved and not save_err:
            quoted = _shell_quote(saved)
            if listed is not None:
                recipes = (
                    "Query the saved result instead of re-running "
                    "(do not cat it):\n"
                    f"  jq '.items[]' {quoted}\n"
                    f"  jq '.items | length' {quoted}"
                )
            else:
                recipes = (
                    "Query the saved result instead of re-running "
                    "(do not cat it):\n"
                    f"  jq 'keys' {quoted}\n"
                    f"  jq '.' {quoted}"
                )
            if size + len(recipes) + 1 <= budget:
                chunks.append(recipes)
    sys.stdout.write("\n".join(chunks) + "\n")


def write_output_file(result: Any) -> None:
    raw = result_bytes(result)
    Path(OUTPUT_PATH).write_bytes(raw)
    Path(OUTPUT_PATH).chmod(0o600)


def print_result(result: Any) -> None:
    if OUTPUT_PATH and OUTPUT_PATH != "-":
        write_output_file(result)
    if OUTPUT_FORMAT == "json":
        print_json(result)
        return
    if OUTPUT_FORMAT == "jsonl":
        print_jsonl(result)
        return
    if OUTPUT_FORMAT == "ai-friendly":
        print_ai_friendly(result)
        return
    if QUIET:
        return
    print_human(result)


def confirm(severity: str, resource: str) -> int:
    if severity in ("", "mild"):
        return 0
    if YES:
        return 0
    if NO_INPUT or not sys.stdin.isatty() or not sys.stderr.isatty():
        print(
            f"error: {COMMAND_PATH} requires confirmation; pass --yes",
            file=sys.stderr,
        )
        return 2
    if severity == "severe":
        print(f"Type {resource} to confirm: ", end="", file=sys.stderr)
        answer = sys.stdin.readline().strip()
        if answer != resource:
            print("error: confirmation failed", file=sys.stderr)
            return 2
        return 0
    print(
        f"Proceed with {COMMAND_PATH} {resource}? [y/N] ",
        end="",
        file=sys.stderr,
    )
    answer = sys.stdin.readline().strip()
    if answer not in ("y", "yes", "Y", "YES"):
        print("error: confirmation failed", file=sys.stderr)
        return 2
    return 0
