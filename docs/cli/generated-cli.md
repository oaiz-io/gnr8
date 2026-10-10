<!-- generated-by: gsd-doc-writer -->
# Generated CLI

[Agent docs index](../agents/index.md)

This page is **not** about gnr8's own command surface (`gnr8 init`, `generate`, `watch`, `check`).
That lives in [CLI command reference](commands.md). This page is the CLI gnr8 **generates for your
API**: a program derived from the same `ApiGraph` as the SDK, written next to that SDK.

Python emits a `<sdk dir>/cli/` subpackage. Go emits a `<sdk dir>/cmd/<program>/` project — a Go
directory is one package, so the CLI cannot live beside `client.go`.

## Opt in

```rust
.target(
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(SdkCli::new("bookstore").base_url("https://api.example.com")),
)
.target(
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(SdkCli::new("bookstore").base_url("https://api.example.com")),
)
```

The builder is `.cli(...)`. There is no second way to get a CLI. Absent `.cli(...)`, no CLI artifact
is written.

`SdkCli` is the configuration type behind that method, and it carries the facts that belong to the
**program** rather than to the API — none of which the graph holds:

| Method | Fact |
|---|---|
| `SdkCli::new(program)` | the name it is invoked as |
| [`base_url(url)`](#the-host-the-program-talks-to) | the host it talks to unless `--base-url` says otherwise |
| [`commands(selector)`](#command-scope) | which operations become commands |
| `hand_owned_main()` | skip emitting `main.go`; a hand-owned `main` calls `Run` (Go only) |
| `owned_command(...)` | a root command whose implementation is hand-owned and never generated (Go only) |
| `topic(...)` | a declared topic, its verbs, positionals, compositions, examples, see-also, and docs URL |
| `rename_error(...)` | a retired invocation that names its replacement and exits 2 |
| `view(...)` | preview and table fields for the rows of one schema |

`CliTopic::owned_command(...)` names a hand-owned command under a topic (Go only). The topic's
dispatcher calls `run<Topic><Name>` in `package cli` (`db types` → `runDbTypes`), or the function
`OwnedCommand::function` names, and the topic's help page and typo hints list it first. The topic
must have at least one generated command (a declared command, or an operation in a group of the same
name), and the name must not equal one of its verbs or sub-nouns. Each owned command, at the root or
under a topic, calls its own function, and that name must not be one the generated `package cli`
already uses: a declaration, an import, a Go predeclared name, `_` or `init`. Like a root owned command, it is absent from `help --json` and shell completion.

A `rename_error(...)` path is matched before any command is dispatched, so it must name a path the
CLI no longer runs. Generation refuses a retired path with a flag token (`--help`), one that is a
live command or a prefix of one
(`books` while `books list` exists), one that extends a live command which takes arguments (`books
get old` while `books get <id>` exists), and one that an earlier retired path already matches.

`.cli("bookstore")` is still accepted — a program name converts into an `SdkCli` — so a program that
needs nothing but a name says nothing but a name. `SdkCli` is unrelated to gnr8's own CLI.

**Packaging is not symmetric.** Combining `.cli(...)` with `.source_only()` or
`.package_metadata(false)` is a configuration error on `PySdk`: without `pyproject.toml` there is
nowhere for `[project.scripts]` to go. `GoSdk::cli` does **not** require package metadata: `cmd/<program>`
is a `package main` that `go build ./cmd/<program>` already knows how to produce a binary from, and
there is no `[project.scripts]` equivalent to write.

## What is emitted

### Python — a subpackage beside the SDK

```text
<sdk dir>/cli/
  __init__.py          the docstring, and the one symbol [project.scripts] names
  __main__.py          what `python -m <package>.cli` runs
  config.py            every fact fixed at generation time
  credentials.py       env var + helper resolution, and build_client
  output.py            formats, ai-friendly envelopes, errors, prompts
  body.py              --body / --body-file / stdin      (only where a body exists)
  parser.py            the root parser; nothing about any one command
  commands/
    __init__.py
    <group>.py         one group's subparsers and the calls they make
    root.py            the commands that sit directly under the program
  main.py              dispatch and exit codes
```

Each group module exposes `register(subparsers)`, so `parser.py` does not grow when the API does,
and how a command parses sits beside how it runs. A command body does two things: turn parsed
arguments into the client method's keyword arguments, and call it. Everything shared — credentials,
output, exit codes — lives in one module each, not once per command.

`cli/` is a subpackage, so it cannot shadow the SDK's own `client.py` / `models.py` / `errors.py`.
It imports the Python standard library, the sibling generated package, and — in the default Pydantic
model style — the same `pydantic` the models already import. It adds no dependency the SDK did not
already have; under `.dataclasses()` it is standard library only.

Plus three lines in `pyproject.toml` when package metadata is on:

```toml
[project.scripts]
"bookstore" = "sdk.cli:main"
```

The scripts key is the program name; the value resolves because `cli/__init__.py` re-exports `main`.
`[tool.setuptools] packages` gains `sdk.cli` and `sdk.cli.commands` on its own — the CLI files are in
the file list `pyproject.toml` is rendered from, and package discovery reads the `__init__.py` files
it finds there.

### Go — a project under `cmd/<program>`

```text
<sdk dir>/cmd/<program>/
  main.go                     package main: stamp version vars, os.Exit(cli.Run(args, Options))
  internal/cli/
    cli.go                    Run(args, Options), the dispatch tree, the usage text
    config.go                 every fact fixed at generation time
    credentials.go            env var + helper resolution, and buildClient
    flags.go                  the flag.Value types and the parse helpers
    output.go                 printResult and TTY detection
    body.go                   loadBody               (only where a body exists)
    errors.go                 handleErr: every typed error to its exit code
    <group>.go                one group's commands
    commands.go               the commands that sit directly under the program
```

`internal/` is Go's own visibility rule, not a convention: the package is importable from
`cmd/<program>/...` and nowhere else, so splitting the program up does not widen anything's API.
`Run` and `Options` are the exported symbols. `version`, `commit` and `date` in `main.go` are
variables so `-ldflags -X` can stamp them. `Run` starts every invocation from a clean state, so a
hand-owned `main`, a REPL or a test may call it more than once in one process without one call's
`--yes` leaking into the next. Standard library only, plus the sibling generated client.
`flag.NewFlagSet` per subcommand, `os.Args[1]` dispatch, `encoding/json` on stdout, `os/exec` for the
credential helper (`CommandContext`, 10s timeout, stdin nil, stderr discarded, first stdout line
only). Every file is `gofmt`-normalized through the same seam the rest of the Go SDK uses, and
carries exactly the imports it uses — Go rejects an unused one.

Go has no `[project.scripts]` equivalent. `.cli()` does not write extra package metadata.

### A group name that collides with a shared file

What each layout has to reserve follows from where it puts a group's file, so the two lists are not
the same:

| Layout | Group file | Reserved group names |
|---|---|---|
| Go | `internal/cli/<group>.go`, beside every shared file | `body`, `cli`, `commands`, `complete`, `config`, `credentials`, `errors`, `flags`, `output` |
| Python | `cli/commands/<group>.py`, one directory below the shared modules | `root` |

Go puts the whole program in one directory, because a Go directory is one package, so a group named
`config` would claim `config.go`. Python's command modules sit in `cli/commands/`, where the only
name already taken is `root.py` — the ungrouped commands — so `cli/commands/config.py` and
`cli/config.py` are different files and a Python group may be called `config`.

Neither reserves `main` (Go's is `cmd/<program>/main.go`, a directory up; Python's is `cli/main.py`,
also not beside the group modules) nor `parser` (Python's `cli/parser.py`, likewise a directory up;
Go has no such file).

A group whose module or file name would collide is a generation error naming the group and
`GroupOperations` — the same remedy every other CLI name collision names.

## How to run it

Python, from the project root, with `generated/sdk` as the output directory:

```sh
(cd generated && python3 -m sdk.cli --help)
pipx install ./generated/sdk && bookstore --help
uv tool install ./generated/sdk && bookstore --help
```

`python3 -m sdk.cli` needs the package's parent on `sys.path`, which is what the subshell's `cd`
gives it; the installers put the program on `PATH` instead.

No install step is needed for the first line — it works as soon as the package is written. The
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
| group prose | `group_docs`, from `GroupOperations::describe` or an imported `tags[].description` |
| command subparser | kebab-case of `op.id` |
| `--help` / `description=` | the handler's own doc-comment synopsis and remainder |
| `--flag` per parameter | kebab-case of the wire name; all four locations |
| `required=True` | `param.required` |
| `type=int` / `type=float` | integer / float primitives |
| `choices=` | an enum, or a named type whose body is an enum |
| `action="append"` | an array |
| `--body` / `--body-file` | request body; `-` on `--body-file` is stdin |
| `--limit` / `--all` / `--cursor` | a `PaginationPolicy` for this operation |
| `--base-url` | `SdkCli::base_url`, and nothing else |

Ungrouped operations sit at the program root. There is no `"default"` group level.

## Help

Help is one page per question, and every page states prose the source already carries.

`--help` on the program is an index: each root command and each group on one line, with the
sentence that describes it. It does not list every command of every group — that is the group's
page, one level down.

```text
bookstore <command> [flags]

Command groups:
  authors  Everything about authors
  books    Everything about books

Run `bookstore <group>` for its commands, `bookstore <group> <command> --help` for its flags.
```

`--help` on a group, and a bare group, print that group's commands. A group answers the questions
asked at its own level; the root index is the answer to a different question.

```text
bookstore books — Everything about books

Usage: bookstore books <command> [flags]

Commands:
  get-book    Fetch one book by its identifier.
  list-books  List books in one genre.
```

A declared command spec adds the rest of the page from facts the pipeline already has. `--help` on a
command prints Arguments for each positional, Flags with that parameter's description and any enum
values argparse/`flag` already list, then Examples, Output, See also, and Docs when the spec names
them.

A Go command that takes `--body` also prints Body after Flags: one row per request-body field with
its name, JSON type, `required` or `optional`, `one of: a|b` for an enum, and the field's description
cut to 80 characters. The fields of a top-level object, or of each item of an array of objects, follow
it one level deep as `parent.child` or `parent[].child`. A field whose named object shape is already
listed — a filter's `or` holding more filters, or a second field of the same type — says
`same shape as filters[]` instead of listing it again. Fields bound as flags through
`CliCommand::body_fields` are listed under Flags, not Body, and a `fixed_body` command has no Body.

```text
Body:
  filters             array of object  optional  Conditions every returned row meets.
  filters[].operator  string  optional  one of: eq|gt|gte|lt|lte|neq  Comparison of the column with value.
  filters[].or        array of object  optional  same shape as filters[]  Group that matches when any matches.
  table               string  required  Table of the database.
``` `CliCommand::example` is required once a spec is declared: generation fails rather than emit a
command that cannot show how to invoke it. Output is generated from the success schema and `view`
when the command does not override it.

`help` is a root command. `bookstore help` is the program index; `bookstore help books list` is the
same page as `bookstore books list --help`. `bookstore help --json` prints the command spec as one
JSON document: each command's invocation, operation id, arguments, flags (name, type, required, help,
enum, default), the request body when the command takes `--body` (`{"schema", "fields"}`, each field
with name, type, required, enum, help and `sameShapeAs`, the prose uncut), examples, see-also, docs
URL, and output note. Topic/verb tokens after `--json` are
not a second encoding of that spec; they still rewrite to `--help` for the human page, and `--json`
always prints the full document.

```text
bookstore books get — Fetch one book by its identifier.

Usage: bookstore books get <id> [flags]

Arguments:
  <id>  The book's identifier. required

Flags:
  ...

Examples:
  bookstore books get 1

Output
  Book: id, title, author

See also  books list
Docs      https://example.com/cli/books/get
```

An unrecognized name prints the closest one it could have meant, then the page the reader wanted:

```text
bookstore: unknown command "lst" under books

Did you mean `bookstore books list-books`?
```

A group's sentence comes from `GroupOperations::describe`, or from the `tags[].description` of an
imported document. A group described in neither renders its name alone: nothing derives a sentence
from the name, because a derived sentence would be a second way to state the same fact (rule 3).

```rust
GroupOperations::new()
    .by_path_prefix("/books", "books")
    .describe("books", "Everything about books")
```

Python gets the same facts through `argparse`: the group subparser carries `help=` and
`description=`, so `--help` at either level renders them without a second code path.

Booleans are two flags sharing one dest: `--verified` and `--no-verified`. A boolean has three
states — unset, explicitly true, explicitly false — and is sent only when one of the two flags was
passed.

Paging parameters named by a `PaginationPolicy` are not ordinary flags. They are replaced by
`--limit N` / `--all` / `--cursor`. Passing `--page-size` is a rename error naming `--limit`.
`--limit` and `--all` walk pages and print one object in the page's own shape
with the last page's metadata and a truthful `hasMore`, instead of a bare array. Each request
asks for the remaining item limit when a page-size parameter is declared. A resume cursor is
retained only when the limit stops at a page boundary; a partial page omits it to avoid skipping
items. Limits must be positive. `--json` on a single page
is still the server's bytes; merged pages clear that capture so the constructed object is what
prints.

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

Reserved flags are computed per command from what that command actually binds: `help`, `base-url`,
`format`, `json`, `fields`, `output`, `quiet`, `debug`, `yes`, `no-input`, `color`, and `no-pager`
always, plus `h`, `o`, `q` and `y` — the short spellings of `output`, `quiet` and `yes`, which Go's `flag`
treats as the same name as `--o`, `--q` and `--y`; `body`/`body-file` where the operation has a request body; `limit`/`all`/`cursor`/`page-size`
where a `PaginationPolicy` names it; and `no-<flag>` for each boolean parameter. `--version` is bound
on the root parser, which is not a command. A parameter named `json`, `format`, or `color` is
therefore a generation error: those flags select the output format and color mode. So is a parameter
named `q`, `o` or `y`; like every collision there is no auto-rename, and the remedy is a source change
or leaving the operation out of the program with `SdkCli::commands(...)`.

## Flag defaults in `--help`

A parameter's `--help` line is its own description (the graph fact: the binding field's doc comment,
the imported spec's `description`, or `DocumentOperation::parameter` when the source has neither),
then whether omitting it is an error, then a source default. Whitespace in the description collapses
to one line because both `flag.PrintDefaults` and argparse `help=` render one line per flag.

A required flag says `required`. The command already refuses to run without it; saying so in
`--help` puts that where the reader is looking instead of one failed invocation later. Python states
the same fact through `argparse`'s own `required=True`.

```text
  -id string
    	The book's identifier. required
```

| | Where the default appears | Why |
|---|---|---|
| Python | `help="default: 10"` | `argparse` renders a default only from a bound `default=`, which is exactly what must not be bound, so the help string carries it |
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

## The host the program talks to

`SdkCli::base_url` is the program's default, and the only source for one:

```rust
.cli(SdkCli::new("bookstore").base_url("https://api.example.com"))
```

`--base-url` is declared on each command so it can follow the subcommand, and overrides the
compiled default per invocation:

```sh
bookstore get-book --book-id 1 --base-url http://127.0.0.1:8000
```

**Without `base_url` the program has no default and `--base-url` is required.** That is deliberate.
The alternative — deriving it from the OpenAPI document's `servers` and falling back to
`http://localhost:8000` — made a fact about the program depend on a fact about the document, so the
only way to point a CLI at production was to publish a deployment URL in the API description; and an
API that declares no `servers` (a normal choice, because the document then describes a contract
rather than one deployment) shipped a client aimed at localhost.

There is no environment variable for the host. One value, one source, plus the flag: `--base-url` is
the user answering at run time, not a second place the fact is written down.


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

`--format human|ai-friendly|json|jsonl` selects how a success is printed. `--json` is shorthand for
`--format json`. `{PROG}_FORMAT` is the environment default when neither flag is set. A TTY stdout
defaults to `human`; anything else defaults to `ai-friendly`.

| Format | stdout | Side effect |
|---|---|---|
| `human` | indented JSON (tables and cards come later) | none |
| `ai-friendly` | a bounded summary (≤4,000 bytes) whose first line names a saved file | a versioned envelope under `.{program}/output/` |
| `json` | the server body, compact off-TTY and indented on a TTY | none |
| `jsonl` | one compact item per line | none |

`--fields` projects items (or the object) in every format; `--fields help` lists the success
schema's wire names and exits 0. `-o/--output` writes the full result to a file; `-o -` prints the full body on stdout without an envelope. `-q/--quiet`
prints only the outcome line in ai-friendly mode, and nothing on success in human mode. `--debug`
(or `{PROG}_DEBUG`) writes a request trace to stderr. `-y/--yes` and `--no-input` (or
`{PROG}_NO_INPUT`) are the two-TTY confirmation rules: a prompt runs only when stdin and stderr
are both TTYs.

The envelope is `gnr8-cli-result` version 1. Its JSON Schema is emitted as
`cmd/<program>/<program>-cli-result-v1.json` for Go and
`cli/<program>-cli-result-v1.json` for Python. Each invocation keeps a separate file,
even when the response is identical. `{PROG}_OUTPUT_DIR` overrides the directory. A
preflight runs before the request when the format is `ai-friendly`; a save that fails after a
successful request still exits 0, says `not saved (…)` on the first line, and prints a `warning:` on
stderr. Envelopes are written to a temporary file and renamed into place, mode 0600, and so are
downloaded files (`.bin`). Retention counts both: past 100 files or 100 MB the oldest go first, but
never `latest.json` or the files the current run wrote, so one result over the byte cap still
survives under the path stdout names.

The summary reserves room for the `Next page:` line and the two `jq` recipes before it spends the
budget on rows, so a full page always says how to continue. Rows print the `view(...)` preview
fields for the row's schema in their declared order, cutting only strings over 80 characters;
without a view they print the first six scalar fields by name. Whether a result is a list is fixed
at generation time from the graph: an array body is a list, a `PaginationPolicy` names a page's
items field, and an object whose only field is an array is a page keyed by that field. Any other
object is one resource, however many arrays it holds. `Next page:` appears only on a command that
binds `--cursor`, reading the policy's next-cursor field.

Errors print `error:` plus the message, then optional `hint:` lines and a `request id:`, at most six
lines. The request id keeps the final line when present; embedded line breaks are displayed as
escapes. Under `--json`/`--format json` the same facts are one JSON object on stderr, with the
server's `slug` when it sent one. Exit codes name the caller's next action:

| Code | When |
|---|---|
| 0 | success |
| 1 | anything not below: HTTP 500 and other statuses, a success body that does not decode, a local write that failed after the request |
| 2 | usage: missing/unknown flags, malformed `--body`, unknown command, a retired invocation |
| 3 | not found (HTTP 404/410) |
| 4 | auth: missing credentials, HTTP 401/403 |
| 5 | refused (HTTP 400/409/412/422) |
| 6 | retry later: a failed connection or a timeout, HTTP 408/429/502/503/504 |
| 130 | interrupted: Python catches Ctrl-C and returns 130; a Go program is ended by SIGINT, which the shell reports as 130 |

Code 6 identifies transient connection or server failures; it does not prove a mutation was not
processed. Local failures after a successful answer —
a body that does not decode or an `-o` file that cannot be written — return 1.

`--color auto|always|never` colors human output (the `error:` prefix today). `NO_COLOR` and
`TERM=dumb` turn it off in `auto`; color is never the only signal. Human lines longer than
`COLUMNS` (or 80) are truncated with `…`. Human TTY output of 24 lines or more is sent to
`{PROG}_PAGER`, then `PAGER`, then `less -FIRX`, unless `--no-pager` is set. Paginated `--limit` /
`--all` walks print `fetched N items…` on stderr when stderr is a TTY.

`completion bash|zsh|fish|powershell` prints a script for that shell. The hidden `__complete`
command answers candidates at every level from the command spec, plus live identifiers via one list
call with a one-second timeout (failure is silent). `help` and `completion` are reserved root
command names.

`--help` and `--version` go to stdout and exit 0. A wrong `--base-url`, a mistyped `--body` and a
missing `--body-file` are the errors a human hits first, and a generated program answers them with a
diagnostic rather than a stack trace.

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

Two things no longer appear in this table because they are no longer graph facts: which operations
become commands, and the default host. Both live in `.gnr8/`, so changing either is a pipeline edit
rather than an API change — the CLI artifact's bytes move and `gnr8 check` reports that drift, which
is correct, because the contract did not move.

| Change class | Codes | CLI effect |
|---|---|---|
| command tree | `operation.added` (Additive), `operation.removed`, `operation.name.changed`, `sdk.group.changed` | subcommand appears / disappears / renamed / moves group |
| flags | `request.parameter.*`, `request.body.*`, `request.enum.value.*`, `request.type.changed` | a flag appears, disappears, or changes requiredness/domain |
| credential | `security.scheme.*`, `security.operation.*`, `security.global.changed` | which env var the CLI reads |
| output | `response.*` | what is printed and what is an error |
| path shape | `document.base_path.changed` | the path a command requests |
| help text only | the 8 `DocOnly` codes | `--help` prose moves; tree, flags, choices and exit codes untouched |

A prose-only change rewrites the generated CLI's `--help` strings. It does not fail the
breaking-change gate. A source edit that leaves the graph identical rewrites nothing, including the
CLI artifact.

## Determinism and ownership

Two generations over the same graph produce byte-identical CLI source, and the contract is **per
file**: a module whose bytes did not change is not rewritten, even when a sibling did. Every file is
an ordinary artifact in the SDK output directory, so each inherits manifest ownership, `gnr8 check`
drift reporting, `--force` protection for hand edits, and deletion when it stops being produced.

Removing `.cli(...)` therefore deletes the whole tree, one file at a time, and reports each in
`deleted`. Narrowing the program with `SdkCli::commands(...)` can also drop a module nothing imports
any more — `body.py` / `body.go` exist only for a command that takes a request body.

gnr8 does not remove the now-empty directory it leaves behind: directory membership is not ownership
evidence, and an unowned neighbour under an output path is never deleted.

TypeScript generated CLIs are out of this slice.

## Changing what is emitted

Every emitted file says `DO NOT EDIT`, and it means it: a hand edit is protected, reported, and then
in your way on every run — `gnr8 generate` refuses to overwrite it and exits non-zero until you pass
`--force` or put the file back. That is the ownership contract working, not a wall. The supported way
to ship different code is to own the *pipeline* rather than the file, which keeps `gnr8 check` green
and the output deterministic.

### Add a file beside the generated ones

The cheapest customization is not a customization at all. gnr8 owns the paths it wrote and nothing
else, so a new file in the same package is simply yours:

```text
generated/sdk/cli/
  main.py            gnr8's, and rewritten whenever the graph moves
  commands/
    books.py         gnr8's
  my_helpers.py      yours — untouched by generate, check, and --force alike
```

Import it from your own code freely. Directory membership is not ownership evidence: `--force`
overwrites gnr8's own files and leaves yours, and `gnr8 check` never mentions them. The one thing to
avoid is a name the emitter might claim later — see the reserved names above.

### Replace an emitted file with a `PostProcess`

To change a file gnr8 owns, hand the pipeline a stage that produces the version you want.
`Artifacts::overlay` replaces one artifact's text wholesale, after every target has run:

```rust
use gnr8::sdk::prelude::*;

/// Our CLI prints NDJSON, because our log pipeline reads stdout.
struct NdjsonOutput;

impl PostProcess for NdjsonOutput {
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        out.overlay(
            "generated/sdk/cli/output.py",
            include_str!("../cli/output_ndjson.py"),
        )
    }
}

// …
.target(PySdk::new().module("example.com/bookstore/sdk").to("generated/sdk").cli("bookstore"))
.post(Custom(NdjsonOutput))
```

The file stays gnr8-owned — it is still written by the pipeline, still byte-identical run over run,
still `gnr8 check`-able — and the text is yours. `overlay` on a path no target produced is a hard
error naming the path, so a rename in a future gnr8 version cannot silently drop your replacement.

`Artifacts::rewrite` transforms the existing text instead of replacing it, which suits a small
insertion:

```rust
out.rewrite("generated/sdk/cli/output.py", |text| {
    text.replace("import json", "import json
import sys")
})
```

Prefer `overlay` where you can. `rewrite` takes an opaque closure, so it cannot distinguish a
deliberate no-op from a pattern that stopped matching because the emitter's output moved — and the
result is still a valid file, so nothing downstream looks wrong. gnr8 raises a
`artifact.rewrite_no_op` WARN naming the file and the stage when a rewrite returns the text it was
given, which turns that silent loss into a visible one; it is still a warning you have to read.

Both run in your `.gnr8/` crate, which is the only extension surface (rule 4) — there is no template
override, no ignore file, and no adoption of a generated file into hand ownership. What you get
instead is that the generated tree is always exactly what the pipeline says it is.
