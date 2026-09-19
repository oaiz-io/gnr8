"use strict";

// `gnr8 doctor`'s TypeScript-toolchain health probe (WR-02): exit 0 iff BOTH `node` runs (it must, to
// run this file) AND the user's `typescript` is resolvable for the target — using the EXACT same
// deterministic resolution `index.js`/`load.js` use at generate time (target project first, then the
// sidecar's own dev `node_modules`). So `gnr8 doctor` reflects REAL readiness: a NestJS project with
// `node` but no `typescript` reports unhealthy here, instead of passing doctor and failing at generate.
//
// There is ONE resolution decision shared with the extractor (`ts.resolveTypescript`) — no second
// source of truth, no fallback (AGENTS.md rule 3). This file resolves NOTHING else and runs NO target
// code: it only asks `require.resolve` whether `typescript` is present.
//
// Contract: `node probe.js <target-dir>` -> exit 0 (typescript resolvable) | exit 1 (absent / no arg).
// stdout stays empty; a human-readable reason goes to stderr (the host only reads the exit status).

const { resolveTypescript } = require("./ts");

function main(argv) {
  const targetDir = argv[2];
  if (!targetDir) {
    process.stderr.write("tsextract probe: usage: node probe.js <target-dir>\n");
    return 1;
  }
  try {
    resolveTypescript(targetDir);
    return 0;
  } catch (exc) {
    process.stderr.write((exc && exc.message ? exc.message : String(exc)) + "\n");
    return 1;
  }
}

process.exitCode = main(process.argv);
