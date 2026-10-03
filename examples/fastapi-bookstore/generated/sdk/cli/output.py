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
PREVIEW: tuple[str, ...] = ()
RESULT_IS_LIST = False
ITEMS_KEY = ""
NEXT_CURSOR_FIELD = ""
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
    slug: str = "",
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
        if slug:
            body["slug"] = slug
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
    # Every global is assigned, not only set: main may run more than once in one
    # process, and an invocation must not inherit the previous one's --yes.
    global OUTPUT_FORMAT, FIELDS, OUTPUT_PATH, QUIET, DEBUG, YES
    global NO_INPUT, COMMAND_PATH, COLOR_MODE, NO_PAGER
    global PREVIEW, RESULT_IS_LIST, ITEMS_KEY, NEXT_CURSOR_FIELD
    resolve_format(
        bool(getattr(args, "json", False)),
        getattr(args, "format", None),
    )
    FIELDS = getattr(args, "fields", None) or ""
    OUTPUT_PATH = getattr(args, "output", None) or ""
    QUIET = bool(getattr(args, "quiet", False))
    DEBUG = bool(getattr(args, "debug", False)) or bool(os.getenv(DEBUG_ENV))
    YES = bool(getattr(args, "yes", False))
    NO_INPUT = bool(getattr(args, "no_input", False)) or bool(os.getenv(NO_INPUT_ENV))
    COLOR_MODE = getattr(args, "color", None) or "auto"
    NO_PAGER = bool(getattr(args, "no_pager", False))
    COMMAND_PATH = getattr(args, "_command", "") or ""
    PREVIEW = tuple(getattr(args, "_preview", ()) or ())
    RESULT_IS_LIST = bool(getattr(args, "_is_list", False))
    ITEMS_KEY = getattr(args, "_items_key", "") or ""
    NEXT_CURSOR_FIELD = getattr(args, "_next_cursor", "") or ""
    LAST_ANSWER.clear()


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
    # The command's result shape is a graph fact fixed at generation time: an array
    # body, or a page whose items sit under ITEMS_KEY. Any other object is one resource.
    if RESULT_IS_LIST:
        return (value if isinstance(value, list) else None), "", {}
    if not ITEMS_KEY or not isinstance(value, dict):
        return None, "", {}
    items = value.get(ITEMS_KEY)
    if items is None:
        items = []
    if not isinstance(items, list):
        return None, "", {}
    meta = {name: item for name, item in value.items() if name != ITEMS_KEY}
    return items, ITEMS_KEY, meta


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


def _prune_output(directory: Path, keep: tuple[Path, ...] = ()) -> None:
    # Envelopes and downloads alike, oldest first, down to 100 files and 100 MB.
    # latest.json and the files this run wrote are never deleted, so one result over
    # the byte cap survives with the path stdout names.
    kept = {"latest.json", *(path.name for path in keep)}
    files = [
        (path.stat(), path)
        for path in directory.iterdir()
        if path.suffix in (".json", ".bin")
        and not path.name.startswith(".")
        and path.name != "latest.json"
        and path.is_file()
    ]
    files.sort(key=lambda entry: entry[0].st_mtime)
    count = len(files)
    total = sum(stat.st_size for stat, _path in files)
    for stat, path in files:
        if count <= 100 and total <= 100 * 1024 * 1024:
            return
        if path.name in kept:
            continue
        try:
            path.unlink()
        except OSError:
            continue
        count -= 1
        total -= stat.st_size


def write_envelope(result: Any, value: Any, raw: bytes) -> tuple[str, str]:
    directory = output_dir_path()
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    kind = "object"
    items = None
    meta = None
    data = None
    file_meta = None
    bin_path: Optional[Path] = None
    page: dict[str, Any] = {}
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        kind = "file"
        name = COMMAND_PATH.replace(" ", "-") or "download"
        bin_path = directory / f"{name}-{_short_id(bytes(result))}.bin"
        _atomic_write(bin_path, bytes(result))
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
    _prune_output(directory, (path,) if bin_path is None else (path, bin_path))
    return str(path), ""


def _shell_quote(value: str) -> str:
    if not value:
        return "''"
    if all(ch not in value for ch in " \t\n'\"\\$`"):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def _preview_value(value: Any) -> Any:
    if isinstance(value, str) and len(value) > 80:
        return value[:79] + "…"
    return value


def _view_row(item: Any) -> str:
    # The declared view fields in their declared order, cutting only a long string;
    # without a view, the first six scalar fields by name.
    fields = field_list()
    if fields or not isinstance(item, dict):
        shown = project_value(item, fields) if fields else item
        return json.dumps(shown, separators=(",", ":"), ensure_ascii=False)
    keys = list(PREVIEW)
    if not keys:
        keys = sorted(
            key
            for key, val in item.items()
            if val is None or isinstance(val, (str, int, float, bool))
        )[:6]
    row = {key: _preview_value(item[key]) for key in keys if key in item}
    return json.dumps(row, separators=(",", ":"), ensure_ascii=False)


def _showing_line(shown: int, total: int) -> str:
    return f"Showing {shown} of {total}; the rest is in the file."


def print_ai_friendly(result: Any) -> None:
    value, raw = decode_result(result)
    saved = ""
    save_err = ""
    try:
        saved, save_err = write_envelope(result, value, raw)
    except OSError as exc:
        save_err = str(exc)
    if save_err:
        print(f"warning: the full result was not saved: {save_err}", file=sys.stderr)
    listed, key, _meta = list_items(value)
    next_page = ""
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        outcome = f"saved {len(result)} bytes"
        rows: list[str] = []
    elif not raw:
        outcome = "empty"
        rows = []
    elif listed is not None:
        outcome = f"{len(listed)} {key or 'items'}"
        rows = [_view_row(item) for item in listed]
        # NEXT_CURSOR_FIELD is set only on a command that binds --cursor, from its
        # PaginationPolicy, so the hint never names a flag the command lacks.
        cursor = value.get(NEXT_CURSOR_FIELD) if NEXT_CURSOR_FIELD else None
        if isinstance(cursor, str) and cursor:
            next_page = (
                f"Next page: {PROGRAM} {COMMAND_PATH} --cursor {_shell_quote(cursor)}"
                f"    Every page: {PROGRAM} {COMMAND_PATH} --all"
            )
    else:
        outcome = "ok"
        rows = [_view_row(value)]
    full = f"not saved ({save_err})" if save_err else saved
    line1 = f"{PROGRAM} {COMMAND_PATH}: {outcome}. Full JSON: {full}"
    chunks = [line1]
    if not QUIET:
        # The next-page line and the jq recipes are what a caller needs next, so
        # they are reserved first and the rows take what is left of the budget.
        budget = 4000
        tail: list[str] = []
        if next_page:
            tail.append(next_page)
        if saved and not save_err:
            quoted = _shell_quote(saved)
            if listed is not None:
                queries = [
                    f"  jq '.items[]' {quoted}",
                    f"  jq '.items | length' {quoted}",
                ]
            else:
                queries = [f"  jq 'keys' {quoted}", f"  jq '.' {quoted}"]
            tail.append(
                "Query the saved result instead of re-running (do not cat it):\n"
                + "\n".join(queries)
            )
        reserve = sum(len(part) + 1 for part in tail)
        size = len(line1) + 1
        shown = 0
        for index, row in enumerate(rows):
            extra = len(row) + 1
            more = 0
            if index < len(rows) - 1:
                more = len(_showing_line(len(rows), len(rows))) + 1
            if size + extra + reserve + more > budget and shown > 0:
                break
            chunks.append(row)
            shown += 1
            size += extra
        if shown < len(rows):
            chunks.append(_showing_line(shown, len(rows)))
        chunks.extend(tail)
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
