// Package tags reads the `binding:`/`validate:` struct tags Go web stacks attach to
// request DTOs, and answers one question about them: which value does a given rule
// constrain?
//
// A tag is a comma-separated list, but it is not flat. `dive` steps into the field's
// elements and `keys`…`endkeys` selects a map's keys, so a rule's position decides
// what it talks about: tokens before the first `dive` constrain the field, tokens
// after it constrain something inside the field. Reading a tag without that
// structure attributes an element's rule to its container — an element-level
// `required` makes an optional key required, a per-element bound is published as the
// array's bound.
//
// Honouring the three scope markers therefore narrows what gnr8 reads rather than
// widening it. Every reader of these tags goes through Scoped — the schema required
// axis, the constraint parser, and both the required axis and the enum placement for
// request parameters — so they cannot answer the same question differently.
//
// What a reader does with a scope is its own business, because the two sides carry
// different things. A schema field records constraints beside its type, so a rule
// about the elements has nothing to bind and is dropped. A bound parameter has only
// a schema, so the same rule replaces the element's schema instead.
//
// ScopeMapKey currently reaches nothing in either reader. An OpenAPI object key is
// always an unconstrained string and gnr8 emits no `propertyNames`, so there is
// nowhere to put a rule about keys — and the OpenAPI lowering rejects a map whose key
// is not a plain string, which means building one would abort a whole document over a
// single well-formed struct tag. The scope is still classified here rather than
// folded into ScopeElement, because reading the marker correctly is what lets the
// rules it guards be discarded instead of misattributed to the values.
package tags

import "strings"

// Scope names which value a tag token constrains.
type Scope int

const (
	// ScopeField is the field itself — the only scope the neutral graph records.
	ScopeField Scope = iota
	// ScopeElement is a value inside the field's collection.
	ScopeElement
	// ScopeMapKey is a key of the field's map.
	ScopeMapKey
)

// Token is one comma-separated token of a tag value paired with the scope it
// applies to. Scope markers are structural and never appear as a Token.
type Token struct {
	Text  string
	Scope Scope
	// Nested is true once more than one dive has been crossed. Readers whose
	// IR carries only one item layer must diagnose rather than flatten it.
	Nested bool
}

// Scoped splits a `binding:`/`validate:` value into tokens and marks what each one
// talks about.
func Scoped(value string) []Token {
	if value == "" {
		return nil
	}
	parts := strings.Split(value, ",")
	tokens := make([]Token, 0, len(parts))
	diveDepth := 0
	inKeys := false
	for _, part := range parts {
		text := strings.TrimSpace(part)
		switch text {
		case "":
			continue
		case "dive":
			// One dive already leaves the field; nested dives only descend further.
			diveDepth++
			inKeys = false
			continue
		case "keys":
			inKeys = true
			continue
		case "endkeys":
			inKeys = false
			continue
		}
		scope := ScopeField
		switch {
		case inKeys:
			scope = ScopeMapKey
		case diveDepth > 0:
			scope = ScopeElement
		}
		tokens = append(tokens, Token{Text: text, Scope: scope, Nested: diveDepth > 1})
	}
	return tokens
}

// HasFieldToken reports whether the tag states the given rule about the field
// itself. A matching token reached through `dive` or `keys` describes what lives
// inside the field and does not count.
func HasFieldToken(value string, expected string) bool {
	for _, token := range Scoped(value) {
		if token.Scope == ScopeField && token.Text == expected {
			return true
		}
	}
	return false
}
