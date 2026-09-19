// Derive operation prose from a declaration's own JSDoc block.
//
// This is AGENTS.md rule 0.1 category 2: the source language's own documentation
// facility, read as PLAIN PROSE. There is no directive syntax, no marker prefix, and no
// key/value grammar — not `@Summary`, not `gnr8:summary`, not anything. A comment with
// grammar is a dialect regardless of who owns it, and dialects grow until they are
// someone's annotation system. This module therefore never matches, tokenizes, or
// branches on comment CONTENT; it takes the text as an opaque string and splits it on
// one positional rule.
//
// The rule is mirrored byte-for-byte in `goextract/internal/docs/docs.go` (the reference
// implementation, which carries the full rationale) and `pyextract/docs.py`, so the
// three sidecars cannot drift. `tests/docs.test.js` holds the same canonical case table
// as `docs_test.go` and `test_docs.py`:
//
//   summary     = text up to and including the first sentence terminator (. ! ?) that is
//                 followed by whitespace or end-of-text, where a `.` preceded by exactly
//                 one uppercase letter does not terminate ("A. Smith" is one sentence).
//                 No terminator anywhere -> the whole text is the summary. Internal
//                 whitespace runs collapse to single spaces.
//   description = the remainder, trimmed, with its line structure preserved.
//
// The split is TOTAL (defined for every input) and LOSSLESS (no text is silently
// dropped), which is what keeps it a split rather than a parse.
//
// Unlike the Go sidecar there is no symbol-name strip: JSDoc descriptions do not begin
// with the method's own name, so there is nothing to remove.
//
// The caller obtains the text via the TypeScript checker's `getDocumentationComment`,
// which returns the LEADING DESCRIPTION ONLY — JSDoc tags are excluded by construction.
// This module therefore NEVER reads a JSDoc tag of any kind, and needs no knowledge of
// which tags exist to avoid them (rule 0.1).

"use strict";

const TERMINATORS = new Set([".", "!", "?"]);
// The CJK ideographic and fullwidth full stops terminate immediately: the writing
// systems that use them do not follow a sentence end with a space. Mirrors `go/doc`.
const IMMEDIATE_TERMINATORS = new Set(["。", "．"]);

// Split returns { summary, description } for a JSDoc description's text. Both are ""
// when the block is empty or whitespace-only; `description` is "" when the block holds
// only a summary sentence. Callers turn "" into an omitted JSON field.
function split(text) {
  if (!text) {
    return { summary: "", description: "" };
  }
  const trimmed = normalizeNewlines(text).trim();
  if (trimmed === "") {
    return { summary: "", description: "" };
  }
  const end = firstSentenceLen(trimmed);
  return {
    summary: trimmed.slice(0, end).split(/\s+/).filter(Boolean).join(" "),
    description: trimmed.slice(end).trim(),
  };
}

// Fold CRLF and lone CR to LF so the split does not depend on checkout style.
function normalizeNewlines(text) {
  if (!text.includes("\r")) {
    return text;
  }
  return text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

// Return the length of the first sentence in `text`, INCLUDING its terminator, or
// text.length when the text holds no terminator.
//
// The period rule is `go/doc`'s: a `.` ends the sentence when followed by whitespace and
// NOT preceded by exactly one uppercase letter, so initials ("A. Smith") and
// abbreviations ("U.S. Army") do not split. `!` and `?` terminate unconditionally — they
// are never used for initials, so the guard would be noise.
//
// Iteration is over CODE UNITS rather than code points, matching `slice`'s indexing.
// Terminators and the uppercase guard are all BMP characters, so an astral character in
// prose is inert here and its surrogate halves simply never match.
function firstSentenceLen(text) {
  let ppp = "";
  let pp = "";
  let p = "";
  for (let index = 0; index < text.length; index += 1) {
    let char = text[index];
    if (char === "\n" || char === "\r" || char === "\t") {
      char = " ";
    }
    if (char === " " && TERMINATORS.has(p) && !isInitial(p, pp, ppp)) {
      return index;
    }
    if (IMMEDIATE_TERMINATORS.has(p)) {
      return index;
    }
    ppp = pp;
    pp = p;
    p = char;
  }
  return text.length;
}

// Report whether a period is part of an initial or abbreviation rather than a sentence
// end: preceded by exactly one uppercase letter (`pp` uppercase, `ppp` not).
function isInitial(p, pp, ppp) {
  return p === "." && isUpper(pp) && !isUpper(ppp);
}

// `toUpperCase` round-trip is the locale-independent uppercase test: a character is
// uppercase when it differs from its own lowercase form and equals its uppercase form.
// Digits, punctuation, and caseless scripts are correctly reported as not-uppercase.
function isUpper(char) {
  return char !== "" && char === char.toUpperCase() && char !== char.toLowerCase();
}

module.exports = { split };
