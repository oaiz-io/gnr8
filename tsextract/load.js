"use strict";

// Loader: discover every `*.ts` under the target tree and build a
// `ts.Program` + `TypeChecker` over them — the TypeScript twin of
// `pyextract/load.py`.
//
// CRITICAL static-only boundary (AGENTS.md rule 3 / threat T-04-05): the loader
// reads each file as TEXT (via the Compiler API's own file system) and builds a
// Program with `ts.createProgram`. It NEVER `require`s, `import`s, `eval`s, runs
// `vm`, transpiles-and-runs, or otherwise EXECUTES the target by any means.
// `ts.createProgram` only PARSES + TYPE-CHECKS the source; that is the
// load-bearing security invariant of the sidecar.
//
// CRITICAL determinism boundary (Pitfall 1): the CompilerOptions are
// SYNTHESIZED here — the loader NEVER reads the target's own project config.
// The optional/nullable axes depend on `strictNullChecks`, so synthesizing the
// options makes the analysis deterministic regardless of the target's config.

const fs = require("fs");
const path = require("path");
const ts = require("./ts");

// One loaded program: the `ts.Program`, its `TypeChecker`, the discovered file
// list, and the canonical target directory (for relative-id derivation).
class Loaded {
  constructor(program, checker, files, targetDir) {
    this.program = program;
    this.checker = checker;
    this.files = files;
    this.targetDir = targetDir;
  }
}

// Recursively discover the sorted list of `*.ts` absolute paths under
// `targetDir`. Sorted for internal determinism (the host re-sorts the final
// facts, but the loader stays internally deterministic). `node_modules` is
// skipped — the target's source only.
function discover(targetDir) {
  const found = [];
  const walk = (dir) => {
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (_err) {
      return; // unreadable dir — skip; never abort the run
    }
    entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    for (const entry of entries) {
      if (entry.name === "node_modules") {
        continue;
      }
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts")) {
        found.push(full);
      }
    }
  };
  walk(targetDir);
  found.sort();
  return found;
}

// Build the `ts.Program` + `TypeChecker` over the discovered `*.ts` of
// `targetDir`. STATIC ONLY: this parses + type-checks; it never executes the
// target. The CompilerOptions are synthesized (NOT read from the target's own
// project config) — `strictNullChecks` + `experimentalDecorators` are REQUIRED.
function load(targetDir, _diags) {
  const files = discover(targetDir);
  const program = ts.createProgram(files, {
    target: ts.ScriptTarget.ES2020,
    experimentalDecorators: true, // REQUIRED — NestJS decorators (Pitfall 6)
    strictNullChecks: true, // REQUIRED — `| null`/`| undefined` survive (Pitfall 1)
    skipLibCheck: true,
    noEmit: true, // never produce output — static analysis only
  });
  const checker = program.getTypeChecker();
  return new Loaded(program, checker, files, targetDir);
}

// Derive the stable, target-relative schema/span file path for `absPath`
// (slash form kept, NOT a dotted module): e.g. `src/books.dto.ts` ->
// `src/books.dto.ts`. Used by both schema-id and span derivation.
function relFile(targetDir, absPath) {
  return path.relative(targetDir, absPath).split(path.sep).join("/");
}

// Derive the stable schema id for a declaration named `name` in `absPath`:
// `relpath(file)` with the `.ts` suffix dropped + "." + name. Verified from the
// snapshot: `src/books.dto.ts` + `BookFormat` -> `src/books.dto.BookFormat`.
function schemaId(targetDir, absPath, name) {
  const rel = relFile(targetDir, absPath);
  const noExt = rel.endsWith(".ts") ? rel.slice(0, -3) : rel;
  return noExt + "." + name;
}

// Whether `absPath`'s declaration lives UNDER the target tree (not the TS lib,
// not `node_modules`). The single deterministic boundary every registration site
// shares (rule 3): a type whose declaration escapes the target is NOT a target
// schema. Centralized here so the seed-root scan and every transitive
// registration path apply the identical test (the seeders used to inline this).
function underTarget(targetDir, absPath) {
  const rel = relFile(targetDir, absPath);
  return !rel.startsWith("..") && !rel.includes("node_modules");
}

// The 1-based line number for a node's start position (Pitfall 6: always pass
// the SourceFile to `getStart`). Returns `{ file, start_line, end_line }` in the
// neutral SourceSpan shape (single-line spans; the host relativizes the file).
function span(sf, node, targetDir) {
  const pos = sf.getLineAndCharacterOfPosition(node.getStart(sf));
  const line = pos.line + 1;
  return {
    file: relFile(targetDir, sf.fileName),
    start_line: line,
    end_line: line,
  };
}

module.exports = { Loaded, load, discover, relFile, schemaId, underTarget, span, ts };
