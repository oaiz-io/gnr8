"""Shell completion.

`completion <shell>` prints a script. `__complete` answers
candidates for the current word from the spec, plus live ids.
"""

from __future__ import annotations

import json
import subprocess
import sys
from typing import Any

from .config import HELP_SPEC, PROGRAM


LIVE_COMPLETES: list[dict[str, Any]] = [
    {
        "path": ["books", "get"],
        "list": ["books", "list"],
        "idField": "id",
        "nameField": "name",
    },
    {
        "path": ["books", "update"],
        "list": ["books", "list"],
        "idField": "id",
        "nameField": "name",
    },
]


BASH_COMPLETION = (
    "# bash completion for bookstore\n_bookstore() {\n  local out line\n  out=\"$(books"
    "tore __complete \"${COMP_WORDS[@]:1}\" 2>/dev/null)\" || return\n  COMPREPLY=()\n "
    " while IFS= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continu"
    "e\n    COMPREPLY+=(\"${line%%$'\\t'*}\")\n  done <<< \"$out\"\n}\ncomplete -o nosp"
    "ace -F _bookstore bookstore\n"
)


ZSH_COMPLETION = (
    "#compdef bookstore\n_bookstore() {\n  local -a completions\n  local out line\n  ou"
    "t=\"$(bookstore __complete \"${words[@]:1}\" 2>/dev/null)\" || return\n  while IFS"
    "= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continue\n    com"
    "pletions+=(\"${line%%$'\\t'*}\")\n  done <<< \"$out\"\n  _describe 'command' compl"
    "etions\n}\n_bookstore \"$@\"\n"
)


FISH_COMPLETION = (
    "function __bookstore_complete\n    bookstore __complete (commandline -opc)[2..-1] "
    "(commandline -ct) 2>/dev/null\nend\ncomplete -c bookstore -f -a '(__bookstore_comp"
    "lete)'\n"
)


POWERSHELL_COMPLETION = (
    "Register-ArgumentCompleter -Native -CommandName bookstore -ScriptBlock {\n  param("
    "$wordToComplete, $commandAst, $cursorPosition)\n  $elems = @($commandAst.CommandEl"
    "ements | Select-Object -Skip 1 | ForEach-Object { $_.ToString() })\n  bookstore __"
    "complete @elems 2>$null | ForEach-Object {\n    if ($_ -notlike ':*') {\n      $na"
    "me = ($_ -split \"`t\")[0]\n      [System.Management.Automation.CompletionResult]:"
    ":new($name, $name, 'ParameterValue', $name)\n    }\n  }\n}\n"
)



def _emit(name: str, help_text: str = "") -> None:
    if help_text:
        print(f"{name}\t{help_text}")
        return
    print(name)


def complete(args: list[str]) -> int:
    prefix = ""
    if args:
        prefix = args[-1]
        args = args[:-1]
    path = [arg for arg in args if not arg.startswith("-")]
    spec = json.loads(HELP_SPEC)
    commands = spec.get("commands") or []
    globals_ = [
        ("--json", "print the server body"),
        ("--format", "output format"),
        ("--fields", "response fields"),
        ("--output", "write the full result to a file"),
        ("--quiet", "print less on success"),
        ("--debug", "write a request trace"),
        ("--yes", "do not ask before a destructive command"),
        ("--no-input", "never prompt"),
        ("--color", "when to color human output"),
        ("--no-pager", "do not page human output"),
        ("--help", "help"),
        ("--base-url", "host to send requests to"),
    ]
    if prefix.startswith("-"):
        if prefix in ("--format", "--format="):
            for value in ("human", "ai-friendly", "json", "jsonl"):
                _emit(value)
        elif prefix in ("--color", "--color="):
            for value in ("auto", "always", "never"):
                _emit(value)
        else:
            for name, help_text in globals_:
                if name.startswith(prefix):
                    _emit(name, help_text)
            joined = " ".join(path)
            for command in commands:
                if command.get("invocation") != joined:
                    continue
                for flag in command.get("flags") or []:
                    name = "--" + str(flag.get("name") or "")
                    if name.startswith(prefix):
                        _emit(name, str(flag.get("help") or ""))
        print(":4")
        return 0
    if not path:
        for name in ("help", "completion"):
            if name.startswith(prefix):
                _emit(name)
    if path == ["completion"]:
        for name in ("bash", "zsh", "fish", "powershell"):
            if name.startswith(prefix):
                _emit(name)
        print(":4")
        return 0
    seen: set[str] = set()
    for command in commands:
        tokens = str(command.get("invocation") or "").split()
        if len(tokens) <= len(path):
            continue
        if tokens[: len(path)] != path:
            continue
        next_name = tokens[len(path)]
        if not next_name.startswith(prefix) or next_name in seen:
            continue
        seen.add(next_name)
        _emit(next_name)
    joined = " ".join(path)
    for command in commands:
        if command.get("invocation") != joined:
            continue
        if not command.get("arguments"):
            continue
        for live in LIVE_COMPLETES:
            if " ".join(live["path"]) == joined:
                complete_live(live, prefix)
    print(":4")
    return 0


def complete_live(live: dict[str, Any], prefix: str) -> None:
    argv = [
        sys.argv[0],
        *live["list"],
        "--json",
        "--fields",
        live["idField"],
        "--limit",
        "50",
    ]
    try:
        completed = subprocess.run(
            argv,
            capture_output=True,
            timeout=1,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return
    if completed.returncode != 0 or not completed.stdout:
        return
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return
    ident_key = live["idField"]
    name_key = live["nameField"]
    for item in spec_items(value):
        ident = item.get(ident_key)
        if not isinstance(ident, str) or not ident:
            continue
        if prefix and not ident.startswith(prefix):
            continue
        help_text = item.get(name_key)
        if not isinstance(help_text, str):
            help_text = ""
        _emit(ident, help_text)


def spec_items(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if isinstance(value, dict):
        for key in ("items", "books", "data"):
            nested = value.get(key)
            items = spec_items(nested)
            if items:
                return items
    return []


def print_completion(args: list[str]) -> int:
    shell = args[0] if args else ""
    scripts = {
        "bash": BASH_COMPLETION,
        "zsh": ZSH_COMPLETION,
        "fish": FISH_COMPLETION,
        "powershell": POWERSHELL_COMPLETION,
    }
    text = scripts.get(shell)
    if text is None:
        print(
            f"Usage: {PROGRAM} completion bash|zsh|fish|powershell",
            file=sys.stderr,
        )
        return 2
    sys.stdout.write(text)
    return 0
