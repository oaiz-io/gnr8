<!-- generated-by: gsd-doc-writer -->
# Generated CLI

[Agent docs index](../agents/index.md)

This page is **not** about gnr8's own command surface (`gnr8 init`, `generate`, `watch`, `check`).
That lives in [CLI command reference](commands.md). This page is the CLI gnr8 **generates for your
API**: a program derived from the same `ApiGraph` as the SDK, written next to that SDK.

Python emits `<sdk dir>/cli.py`. Go emits `<sdk dir>/cmd/<program>/main.go` — a Go directory is one
package, so the CLI cannot live beside `client.go`.

## Opt in

```rust
.target(
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli("bookstore"),
)
.target(
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli("bookstore"),
)
```

The builder is `.cli("bookstore")`. There is no second way to get a CLI. Absent `.cli(...)`, no
CLI artifact is written.

`SdkCli` is the configuration type behind that method. It names the generated program. It is
unrelated to gnr8's own CLI.

**Packaging is not symmetric.** Combining `.cli(...)` with `.source_only()` or
`.package_metadata(false)` is a configuration error on `PySdk`: without `pyproject.toml` there is
nowhere for `[project.scripts]` to go. `GoSdk::cli` does **not** require package metadata. A Go
directory is one package, so the CLI is a standalone `package main` at `cmd/<program>/main.go` that
`go build ./cmd/<program>` already knows how to produce a binary from. There is no `[project.scripts]`
equivalent to write.

## What is emitted

### Python

One file, `<sdk dir>/cli.py`, plus three lines in `pyproject.toml` when package metadata is on:

```toml
[project.scripts]
"bookstore" = "sdk.cli:main"
```

The scripts key is the program name. The value's package segment is the same `sdk_package` derivation
`[tool.setuptools] packages` already uses. The scripts table sits after every `[project]` key
(including optional description/license/keywords and `[project.urls]`) and before
`[tool.setuptools]`.

`cli.py` imports the Python standard library, the sibling generated package, and — in the default
Pydantic model style — the same `pydantic` the models already import. It adds no dependency the SDK
did not already have; under `.dataclasses()` it is standard library only.

### Go

One file, `<sdk dir>/cmd/<program>/main.go`, `package main`, standard library only plus the sibling
generated client. `flag.NewFlagSet` per subcommand, `os.Args[1]` dispatch, `encoding/json` on stdout,
`os/exec` for the credential helper (`CommandContext`, 10s timeout, stdin nil, stderr discarded, first
stdout line only). The file is `gofmt`-normalized through the same seam the rest of the Go SDK uses.

Go has no `[project.scripts]` equivalent. `.cli()` does not write extra package metadata.

## How to run it

Python, from the project root, with `generated/sdk` as the output directory:

```sh
(cd generated && python3 -m sdk.cli --help)
pipx install ./generated/sdk && bookstore --help
uv tool install ./generated/sdk && bookstore --help
```

`python3 -m sdk.cli` needs the package's parent on `sys.path`, which is what the subshell's `cd`
gives it; the installers put the program on `PATH` instead.

No install step is needed for the first line — it works as soon as the file is written. The
installer shims need a distribution name other than the default last-segment `sdk` if you want
`pipx install bookstore-sdk` — set
`.package(SdkPackageMetadata::new().registry_name("bookstore-sdk"))` on the same `PySdk` stage.
`.cli(...)` does not invent a distribution name.

Go, from the SDK output directory:

```sh
go build -o bookstore ./cmd/bookstore
./bookstore --help
go install ./cmd/bookstore
```

The FastAPI bookstore example at `examples/fastapi-bookstore` is the committed Python slice. The Go
bookstore example at `examples/bookstore` is the committed Go slice (`GoSdk::cli("bookstore")`).
fastapi-bookstore's graph carries union types the Go target cannot emit, so a Go SDK is not added
there.

## Command tree

| Piece | Source |
|---|---|
| program name (`prog=`) | `SdkCli::program` |
| top-level description | graph title, plus `openapi_metadata.description` when present |
| `--version` | `openapi_metadata.version`, else `0.1.0` — the same default `info.version` takes |
| group subparser | `op.group`, kebab-cased, only when `Some` |
| command subparser | kebab-case of `op.id` |
| `--help` / `description=` | the handler's own doc-comment synopsis and remainder |
| `--flag` per parameter | kebab-case of the wire name; all four locations |
| `required=True` | `param.required` |
| `type=int` / `type=float` | integer / float primitives |
| `choices=` | an enum, or a named type whose body is an enum |
| `action="append"` | an array |
| `--body` / `--body-file` | request body; `-` on `--body-file` is stdin |
| `--limit` / `--all` | a `PaginationPolicy` for this operation |
| `--base-url` | first server URL, else `http://localhost:8000` |

Ungrouped operations sit at the program root. There is no `"default"` group level.

Booleans are two flags sharing one dest: `--verified` and `--no-verified`. An optional boolean
without a default has three states; the field is sent only when the value is not `None`.

Paging parameters named by a `PaginationPolicy` are not ordinary flags. They are replaced by
`--limit N` / `--all`.

A success response whose `body_kind` is `sse` is a generation error naming the operation. Drop it from
the graph with a `Transform` if you want a CLI.

Name collisions (two commands, a command vs a group, a flag vs a reserved name) are generation
errors that name both subjects. Reserved flags: `json`, `help`, `version`, `base-url`, `limit`,
`all`, `body`, `body-file`. There is no auto-rename.

`--base-url` is declared on each command so it can follow the subcommand:

```sh
bookstore get-book --book-id 1 --base-url http://127.0.0.1:8000
```

Per-flag `help=` text is not emitted in this slice.

## Output and exit codes

| Code | When | Where |
|---|---|---|
| 0 | success | JSON on stdout (`indent=2`); binary bodies on `sys.stdout.buffer` |
| 1 | `ApiError` | `bookstore: 404 not found (book.missing)` on stderr |
| 1 | missing credentials | names the env vars that would satisfy the command |
| 1 | helper failure | `bookstore: credential helper failed (exit 1)` on stderr |
| 1 | transport failure | `bookstore: <urlopen error [Errno 111] Connection refused>` on stderr |
| 2 | usage | argparse's own message on stderr |
| 2 | unreadable or malformed body | `bookstore: body is not valid JSON: ...` on stderr |

`--help` and `--version` go to stdout and exit 0. Every one of those is a single line of prose:
a wrong `--base-url`, a mistyped `--body` and a missing `--body-file` are the errors a human hits
first, and a generated program answers them with a diagnostic rather than a Python traceback. A
response the SDK cannot decode is the exception — that is the API disagreeing with the SDK, and it
surfaces raw.

## Credentials

The generated CLI does not choose an auth alternative. The emitted client already does that. The
CLI only populates the client's inputs:

| Scheme | Client argument |
|---|---|
| apiKey (header or query) | `api_keys={scheme_id: secret}` |
| HTTP bearer | `bearer_token=` |
| HTTP basic | `basic_auth=(user, password)` split on the first `:` |

It never passes `api_key=`.

Two environment names, both derived, never configured:

```
credential:  {SCREAMING_SNAKE(program)}_{SCREAMING_SNAKE(scheme.id)}
helper:      {SCREAMING_SNAKE(program)}_CREDENTIAL_HELPER
```

Example: program `bookstore`, scheme `ApiKeyAuth` → `BOOKSTORE_API_KEY_AUTH` and
`BOOKSTORE_CREDENTIAL_HELPER`.

If the helper variable is set and non-empty, it is the only credential source for every scheme and
the per-scheme variables are not read. If it is not set, the per-scheme variables are the only
source and no subprocess is spawned. A helper that exits non-zero is an error, not an
environment-variable read.

### Helper contract

```
invocation : argv = shlex.split($PROG_CREDENTIAL_HELPER) + [scheme_id]
             shell=False, stdin=DEVNULL, timeout=10s, cwd inherited
success    : exit 0 and a non-empty first line of stdout
secret     : that first line, trailing newline stripped
failure    : non-zero exit, empty stdout, missing executable, timeout, an unparseable
             command line, or a command line that is only whitespace
             → one stderr line, exit 1
```

The diagnostic names the variable and the reason, never the command line the variable holds: a
helper command can carry a token of its own.

The secret never appears on `argv`. No `keyring` import is emitted. Backends are user-chosen at
runtime, for example:

```sh
export BOOKSTORE_CREDENTIAL_HELPER='security find-generic-password -w -s bookstore'
export BOOKSTORE_CREDENTIAL_HELPER='op read op://vault/bookstore/token'
export BOOKSTORE_CREDENTIAL_HELPER='gh auth token'
export BOOKSTORE_CREDENTIAL_HELPER='pass show bookstore/api'
```

OAuth2 / OIDC flows and token caches are out of scope. The CLI inherits the SDK's auth ceiling
(apiKey header/query and HTTP bearer/basic).

## Change gating

`gnr8 changes` does not gain a `--cli` flag and there are no `cli.*` change codes. A CLI command is
kebab-case of the same operation id the SDK already names, so an existing report already says which
commands moved.

| Change class | Codes | CLI effect |
|---|---|---|
| command tree | `operation.added` (Additive), `operation.removed`, `operation.name.changed`, `sdk.group.changed` | subcommand appears / disappears / renamed / moves group |
| flags | `request.parameter.*`, `request.body.*`, `request.enum.value.*`, `request.type.changed` | a flag appears, disappears, or changes requiredness/domain |
| credential | `security.scheme.*`, `security.operation.*`, `security.global.changed` | which env var the CLI reads |
| output | `response.*` | what is printed and what is an error |
| default host | `document.server.*` (Breaking branch), `document.base_path.changed` | the default `--base-url` |
| help text only | the 8 `DocOnly` codes | `--help` prose moves; tree, flags, choices and exit codes untouched |

A prose-only change rewrites the generated CLI's `--help` strings. It does not fail the
breaking-change gate. A source edit that leaves the graph identical rewrites nothing, including the
CLI artifact.

## Determinism and ownership

Two generations over the same graph produce byte-identical CLI source. Unchanged bytes are not
rewritten. The file is an ordinary artifact in the SDK output directory, so it inherits manifest
ownership, `gnr8 check` drift reporting, `--force` protection for hand edits, and deletion when
`.cli(...)` is removed.

TypeScript generated CLIs are out of this slice.
