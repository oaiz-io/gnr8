"""pyextract — the stdlib-`ast` Python source sidecar for gnr8.

This package statically reads a Python service tree and emits the language-neutral
JSON facts document the gnr8 host deserializes (the same contract `goextract`
emits). It is the Python twin of `goextract/`: load -> symbol table -> types ->
schemas -> diagnostics -> facts marshal.

Hard invariants (AGENTS.md):
  * stdlib ONLY — `ast`, `json`, `sys`, `os`, `pathlib`, `enum`, `dataclasses`,
    `typing`. No third-party module, ever. The target's pydantic/fastapi/flask are
    NEVER imported.
  * STATIC ONLY — the target source is read as TEXT and parsed with `ast.parse`.
    The sidecar NEVER executes, imports, or dynamically loads the target by any
    means. Parsing is not executing (threat T-static-exec / PYSRC-03).
  * ONE source per fact — an unresolvable/foreign name produces a diagnostic and the
    fact is OMITTED, never guessed (rule 3).
"""
