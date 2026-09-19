// Package docs derives operation prose from a declaration's own doc comment.
//
// This is AGENTS.md rule 0.1 category 2: the source language's own documentation
// facility, read as PLAIN PROSE. There is no directive syntax, no marker prefix, and
// no key/value grammar — not `@Summary`, not `gnr8:summary`, not anything. A comment
// with grammar is a dialect regardless of who owns it, and dialects grow until they
// are someone's annotation system. This package therefore never matches, tokenizes,
// or branches on comment CONTENT; it takes the text as an opaque string and splits it
// on one positional rule.
//
// The rule, mirrored byte-for-byte in `pyextract/docs.py` and `tsextract/docs.js` so
// the three sidecars cannot drift:
//
//	summary     = text up to and including the first sentence terminator (. ! ?) that
//	              is followed by whitespace or end-of-text, where a `.` preceded by
//	              exactly one uppercase letter does not terminate ("A. Smith" is one
//	              sentence). No terminator anywhere -> the whole text is the summary.
//	              Internal whitespace runs collapse to single spaces: a summary is a
//	              single line by definition.
//	description = the remainder, trimmed, with its line structure preserved.
//
// The function is TOTAL (defined for every input) and LOSSLESS (no text is silently
// dropped), which is what keeps it a split rather than a parse.
//
// The period rule is `go/doc`'s, transcribed rather than called: `doc.Synopsis` is
// deprecated in favour of `(*Package).Synopsis`, it collapses whitespace across the
// WHOLE text rather than just the summary, and it returns "" for text beginning
// "Deprecated:" / "Copyright" / "All rights reserved" — which would make Go behave
// differently from Python and TypeScript on ordinary input. We steal the idea, not the
// implementation (AGENTS.md rule 0: steal freely; never be compliant).
//
// Standard library only (AGENTS.md rule 2 for the sidecar).
package docs

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// Split returns the summary and description for a doc comment's text.
//
// Both are "" when the comment is empty or whitespace-only; description is "" when the
// comment holds only a summary sentence. Callers turn "" into an omitted JSON field.
func Split(text string) (summary, description string) {
	trimmed := strings.TrimSpace(normalizeNewlines(text))
	if trimmed == "" {
		return "", ""
	}
	n := firstSentenceLen(trimmed)
	return collapseWhitespace(trimmed[:n]), strings.TrimSpace(trimmed[n:])
}

// StripSymbolName removes a leading `name ` from a Go doc summary and capitalizes what
// follows, turning the universal Go convention "listWidgets returns widgets." into the
// API-facing "Returns widgets.".
//
// This is Go-only: PEP 257 and JSDoc have no such convention, so `pyextract` and
// `tsextract` deliberately do not mirror it. The summary is returned unchanged when it
// does not start with the symbol name, or when nothing would remain after stripping.
func StripSymbolName(summary, name string) string {
	if name == "" {
		return summary
	}
	rest, found := strings.CutPrefix(summary, name+" ")
	if !found {
		return summary
	}
	rest = strings.TrimSpace(rest)
	if rest == "" {
		return summary
	}
	first, size := utf8.DecodeRuneInString(rest)
	if !unicode.IsLower(first) {
		return rest
	}
	return string(unicode.ToUpper(first)) + rest[size:]
}

// normalizeNewlines folds CRLF and lone CR to LF so the split is identical regardless
// of how the source file was checked out (GRAPH-02: byte-identical output).
func normalizeNewlines(text string) string {
	if !strings.ContainsRune(text, '\r') {
		return text
	}
	return strings.ReplaceAll(strings.ReplaceAll(text, "\r\n", "\n"), "\r", "\n")
}

// firstSentenceLen returns the byte length of the first sentence in s, INCLUDING its
// terminator, or len(s) when s holds no terminator.
//
// The period rule is `go/doc`'s: a `.` ends the sentence when followed by whitespace
// and NOT preceded by exactly one uppercase letter, so initials ("A. Smith") and
// abbreviations ("U.S. Army") do not split. `!` and `?` terminate unconditionally —
// they are never used for initials, so the guard would be noise. The CJK ideographic
// and fullwidth full stops terminate immediately, as they do in `go/doc`, because they
// are not followed by a space in the writing systems that use them.
func firstSentenceLen(s string) int {
	var ppp, pp, p rune
	for i, q := range s {
		if q == '\n' || q == '\r' || q == '\t' {
			q = ' '
		}
		if q == ' ' && terminates(p) && !isInitial(p, pp, ppp) {
			return i
		}
		if p == '。' || p == '．' {
			return i
		}
		ppp, pp, p = pp, p, q
	}
	return len(s)
}

// terminates reports whether r can end a sentence.
func terminates(r rune) bool {
	return r == '.' || r == '!' || r == '?'
}

// isInitial reports whether a period is part of an initial or abbreviation rather than
// a sentence end: preceded by exactly one uppercase letter (`pp` upper, `ppp` not).
func isInitial(p, pp, ppp rune) bool {
	return p == '.' && unicode.IsUpper(pp) && !unicode.IsUpper(ppp)
}

// collapseWhitespace folds every run of whitespace into a single space, so a summary
// that wrapped across comment lines renders as one line.
func collapseWhitespace(text string) string {
	return strings.Join(strings.Fields(text), " ")
}
