"""Derive operation prose from a declaration's own docstring.

This is AGENTS.md rule 0.1 category 2: the source language's own documentation
facility, read as PLAIN PROSE. There is no directive syntax, no marker prefix, and no
key/value grammar — not ``@Summary``, not ``gnr8:summary``, not anything. A comment with
grammar is a dialect regardless of who owns it, and dialects grow until they are
someone's annotation system. This module therefore never matches, tokenizes, or branches
on docstring CONTENT; it takes the text as an opaque string and splits it on one
positional rule.

The rule is mirrored byte-for-byte in ``goextract/internal/docs/docs.go`` (the reference
implementation, which carries the full rationale) and ``tsextract/docs.js``, so the three
sidecars cannot drift. ``tests/test_docs.py`` holds the same canonical case table as
``docs_test.go`` and ``docs.test.js``::

    summary     = text up to and including the first sentence terminator (. ! ?) that is
                  followed by whitespace or end-of-text, where a `.` preceded by exactly
                  one uppercase letter does not terminate ("A. Smith" is one sentence).
                  No terminator anywhere -> the whole text is the summary. Internal
                  whitespace runs collapse to single spaces.
    description = the remainder, trimmed, with its line structure preserved.

The split is TOTAL (defined for every input) and LOSSLESS (no text is silently dropped),
which is what keeps it a split rather than a parse.

Unlike the Go sidecar there is no symbol-name strip: PEP 257 docstrings do not begin
with the function's own name, so there is nothing to remove.

Standard library only (AGENTS.md rule 2 for the sidecar).
"""

_TERMINATORS = (".", "!", "?")
# The CJK ideographic and fullwidth full stops terminate immediately: the writing systems
# that use them do not follow a sentence end with a space. Mirrors `go/doc`.
_IMMEDIATE_TERMINATORS = ("。", "．")


def split(text):
    """Return ``(summary, description)`` for a docstring's text.

    Both are ``""`` when the docstring is empty or whitespace-only; ``description`` is
    ``""`` when the docstring holds only a summary sentence. Callers turn ``""`` into an
    omitted JSON field.
    """
    if not text:
        return "", ""
    trimmed = _normalize_newlines(text).strip()
    if not trimmed:
        return "", ""
    end = _first_sentence_len(trimmed)
    return " ".join(trimmed[:end].split()), trimmed[end:].strip()


def _normalize_newlines(text):
    """Fold CRLF and lone CR to LF so the split does not depend on checkout style."""
    if "\r" not in text:
        return text
    return text.replace("\r\n", "\n").replace("\r", "\n")


def _first_sentence_len(text):
    """Return the length of the first sentence in ``text``, INCLUDING its terminator.

    Returns ``len(text)`` when the text holds no terminator. The period rule is
    ``go/doc``'s: a ``.`` ends the sentence when followed by whitespace and NOT preceded
    by exactly one uppercase letter, so initials ("A. Smith") and abbreviations
    ("U.S. Army") do not split. ``!`` and ``?`` terminate unconditionally — they are
    never used for initials, so the guard would be noise.
    """
    ppp = pp = p = ""
    for index, char in enumerate(text):
        if char in "\n\r\t":
            char = " "
        if char == " " and p in _TERMINATORS and not _is_initial(p, pp, ppp):
            return index
        if p in _IMMEDIATE_TERMINATORS:
            return index
        ppp, pp, p = pp, p, char
    return len(text)


def _is_initial(p, pp, ppp):
    """Report whether a period is part of an initial or abbreviation, not a sentence end.

    True when the period is preceded by exactly one uppercase letter (``pp`` uppercase,
    ``ppp`` not).
    """
    return p == "." and pp.isupper() and not ppp.isupper()
