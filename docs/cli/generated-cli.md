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

Booleans are two flags sharing one dest: `--verified` and `--no-verified`. A boolean has three
states — unset, explicitly true, explicitly false — and is sent only when one of the two flags was
passed.

Paging parameters named by a `PaginationPolicy` are not ordinary flags. They are replaced by
`--limit N` / `--all`.

A success response whose `body_kind` is `sse` is a generation error naming the operation. Leave it out
of the program with `SdkCli::commands(...)` — see [Command scope](#command-scope).

Name collisions are generation errors that name both subjects. There is no auto-rename — the fix is
`RenameOperation` or a source change. Four classes:

| Class | Example |
|---|---|
| two operations map to one command in one group | `getBook` and `get_book` → `get-book` |
| a top-level command collides with a group name | an ungrouped `books` beside a `books` group |
| a flag collides with a global the command binds | a parameter named `base_url` |
| two parameters of one operation map to one flag | `page_size` beside `pageSize`; or `no_verified` beside a boolean `verified`, whose negation is already `--no-verified` |

The last class matters because the emitted program would not start at all: Go's `flag` panics on a
name already in use, and `argparse` raises `ArgumentError` while building the parser, so even
`--help` fails.

Reserved flags: `help`, `base-url` on every command; `body`/`body-file` where the operation has a
request body; `limit`/`all` where a `PaginationPolicy` names it; and `no-<flag>` for each boolean
parameter.

`--base-url` is declared on each command so it can follow the subcommand:

```sh
bookstore get-book --book-id 1 --base-url http://127.0.0.1:8000
```

Per-flag prose is not emitted. The one thing a flag's `--help` does carry is a source default:

| | Where the default appears | Why |
|---|---|---|
| Python | `help="default: 10"` | `argparse` shows a default only through the help string |
| Go | `(default 10)`, from `flag.PrintDefaults` | `flag` renders the registered default itself, and omits it when it is the zero value for the type |
| Go, booleans | `(default true)` in the usage string | a `flag.Value` has no default for `PrintDefaults` to render |

**A default is shown, never sent.** Omit the flag and the CLI sends nothing, so the request is the
one the SDK's own method builds and the server applies its own default. This is what the keyword
means: OpenAPI says the Schema Object's `default` "documents the receiver's behavior rather than
inserting the value into the data", and JSON Schema files it under annotations. Sending it would
make an omitted flag indistinguishable from a user who typed the value, and would pin every CLI
caller to today's value if the server's changed.

A source default does not make a required parameter optional: a required flag must still be
supplied, and omitting it is a usage error.

## Command scope

By default every operation in the graph becomes a command. `SdkCli::commands(selector)` narrows that
to the operations the selector matches:

```rust
use gnr8::sdk::prelude::*;

.target(
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(SdkCli::new("bookstore").commands(OperationSelector::not(
            OperationSelector::any([
                OperationSelector::operation("streamJobEvents"),
                OperationSelector::operation("watchWorkflow"),
            ]),
        ))),
)
```

`.cli("bookstore")` still works — a program name converts into an `SdkCli`, so the two spellings are
one method with one argument.

This is the same [`OperationSelector`](../pipeline/transforms.md) every selector-taking transform
uses, plus `OperationSelector::not(...)` for exclusion. `Any`/`All` compose inside it.

**Scope is a fact about the program, not about the API.** An operation left out is still in
`openapi.yaml` and still a method on the generated client — the CLI simply does not wrap it, the way
a hand-written CLI wraps part of the SDK it calls. That is why scope belongs here and not in a
`Transform`: a transform that drops the operation from the graph would also remove it from the
OpenAPI document and from every SDK, and `gnr8 changes` would report `operation.removed` as a
breaking change.

Consequences worth knowing:

- A selector that matches no operation is a configuration error, like every other selector consumer.
  So is a scope that leaves the program with no commands at all.
- Name and flag collisions are checked over the **selected** operations only. An operation that is
  not a command can no longer fail generation for a flag it never emits.
- `ConfigurePagination`, `ApplySecurity` and the other selector-taking transforms are unaffected by
  CLI scope: they configure graph facts, which do not depend on which program wraps them.
- There is no "hidden command". Out of scope means not emitted; `--help` still lists everything the
  program can do.

## Renaming

There is one canonical way to change a command's name, and it is not CLI-specific:

| To change | Use | What moves with it |
|---|---|---|
| the command (verb) | `RenameOperation::new("getBook", "fetchBook")` | the CLI command, the SDK method in all three languages, and the OpenAPI `operationId` |
| the group (noun) | `GroupOperations` | the CLI group, the SDK grouping, and the OpenAPI tag |

A flag's spelling is the wire name re-cased, so it changes when the parameter changes in the source.

**Aliases are a non-goal.** A generated second name for one operation is two names for one fact, and
it moves whenever the graph moves. If you want a shorter invocation, your shell already has one:

```sh
alias bkls='bookstore list-books'
```

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
