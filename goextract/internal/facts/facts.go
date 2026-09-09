// Package facts defines the language-neutral JSON facts document that a sidecar
// emits on stdout — the host↔sidecar contract boundary (CONTEXT D-02). Every slice
// is sorted by a stable key before marshalling so that two runs on unchanged source
// are byte-identical (GRAPH-02 / Pitfall 1: never range a Go map into output order).
//
// The json tags here MUST match the serde field names + enum tags in
// crates/gnr8-core/src/analyze/facts.rs EXACTLY. The Rust host deserializes this
// document under deny_unknown_fields, so any drift (a renamed/added/dropped tag)
// makes the host reject the sidecar output. Treat any contract change as an atomic
// two-file edit (this file + facts.rs). The drift guard in facts_test.go asserts the
// emitted key set against a hardcoded canonical list so a mismatch fails here.
//
// Standard library only (CLAUDE.md rule 2 for the sidecar): encoding/json, io, sort.
package facts

import (
	"encoding/json"
	"io"
	"sort"
)

// GoFacts is the top-level facts document for a single target module.
//
// ExtractorToolchain names the toolchain THIS SIDECAR BINARY was built with, which is
// not the same fact as the toolchain the analyzed module selects. A sidecar can only
// type-check the language version it was compiled for, so a sidecar older than the module
// it reads reports every package gated on the newer release as a load error while still
// exiting 0. Reporting the toolchain lets the host compare the two and refuse the facts
// rather than publish an extraction it knows is incomplete. The field is language-neutral
// by design: any sidecar may report the toolchain identity it runs under, and one that
// does not simply omits it.
type GoFacts struct {
	Module             string           `json:"module"`
	ExtractorToolchain string           `json:"extractor_toolchain,omitempty"`
	Routes             []RouteFact      `json:"routes"`
	Schemas            []SchemaFact     `json:"schemas"`
	Diagnostics        []DiagnosticFact `json:"diagnostics"`
}

// RouteFact describes one HTTP route, derived PURELY from source code. There is
// exactly one code-derived source per fact and no annotation/fallback path anywhere
// (CLAUDE.md rules 1 & 3):
//
//   - OperationID is the handler function/method symbol (e.g. `createGoal`).
//   - Path is the code-derived, group-relative, normalized template (`/`, `/list`,
//     `/{uuid}`). The dynamic mount prefix is a lowering concern on the host side.
//   - RequestBody / Responses / Params come from the recognized handler-body calls.
//
// Security, router-path overrides, and param enum/required-from-annotation are
// DELIBERATELY ABSENT: those were doc-comment-annotation facts. Security now lives in
// the user's gnr8 config (CLAUDE.md rule 4). Group is a source-derived route-group/tag
// name from static Gin groups, not an annotation.
//
// Summary/Description ARE present, and are still a single code-derived source: the
// routed handler's own doc comment read as PLAIN PROSE via the language's native
// synopsis convention (CLAUDE.md rule 0.1 category 2). They carry no grammar — there is
// no marker, prefix, or key/value syntax inside the comment — and they can state
// nothing structural. Every field above them stays code-inferred and a doc comment can
// neither set nor override any of them.
type RouteFact struct {
	Method                 string                   `json:"method"`
	Path                   string                   `json:"path"`
	Handler                string                   `json:"handler"`
	OperationID            string                   `json:"operation_id"`
	Summary                string                   `json:"summary,omitempty"`
	Description            string                   `json:"description,omitempty"`
	Group                  string                   `json:"group,omitempty"`
	Middleware             []string                 `json:"middleware,omitempty"`
	Params                 []ParamFact              `json:"params"`
	RequestBody            *TypeRef                 `json:"request_body"`
	RequestBodyRequired    bool                     `json:"request_body_required"`
	RequestBodyContentType string                   `json:"request_body_content_type,omitempty"`
	RequestBodyVariants    []RequestBodyVariantFact `json:"request_body_variants,omitempty"`
	Responses              []ResponseFact           `json:"responses"`
	Span                   SourceSpan               `json:"span"`
}

// ParamFact describes a path or query parameter, derived purely from code. Path
// params are required; query params default to a string type and not required. There
// is no enum or description — those were annotation-only and are gone.
type ParamFact struct {
	Name          string        `json:"name"`
	Location      string        `json:"location"`
	Required      bool          `json:"required"`
	Schema        Type          `json:"schema"`
	Default       *LiteralValue `json:"default,omitempty"`
	Style         string        `json:"style,omitempty"`
	Explode       *bool         `json:"explode,omitempty"`
	AllowReserved bool          `json:"allow_reserved,omitempty"`
	Span          SourceSpan    `json:"span"`
}

// ResponseFact describes one response keyed by HTTP status.
type ResponseFact struct {
	Status       uint16               `json:"status"`
	Body         *TypeRef             `json:"body"`
	BodyKind     string               `json:"body_kind,omitempty"`
	ContentType  string               `json:"content_type,omitempty"`
	ContentTypes []string             `json:"content_types,omitempty"`
	Headers      []ResponseHeaderFact `json:"headers,omitempty"`
}

// RequestBodyVariantFact is one additional media-type/schema pair accepted by
// an operation. The long-standing RouteFact request-body fields remain the
// primary entry so existing generator crates keep their source-facing view.
type RequestBodyVariantFact struct {
	Body        TypeRef `json:"body"`
	ContentType string  `json:"content_type"`
}

// ResponseHeaderFact is one response header declared by source behavior.
type ResponseHeaderFact struct {
	Name   string `json:"name"`
	Schema Type   `json:"schema"`
}

// SchemaFact is one extracted named type. Its body is carried by the neutral Type
// vocabulary (an object becomes a Type with TypeObject, a string-enum a Type with
// TypeEnum) — there is no separate string discriminator.
type SchemaFact struct {
	ID   string     `json:"id"`
	Name string     `json:"name"`
	Body Type       `json:"body"`
	Span SourceSpan `json:"span"`
}

// FieldFact is one field of an object schema. Inbound and outbound presence and
// null behavior remain independent so each payload direction can be projected
// without widening the other.
type FieldFact struct {
	JSONName                  string     `json:"json_name"`
	SerializerMayOmit         bool       `json:"serializer_may_omit"`
	DeserializerAcceptsAbsent bool       `json:"deserializer_accepts_absent"`
	DeserializerAcceptsNull   bool       `json:"deserializer_accepts_null"`
	SerializerMayEmitNull     bool       `json:"serializer_may_emit_null"`
	ValidatorRequiresPresence bool       `json:"validator_requires_presence"`
	ValidatorRejectsNull      bool       `json:"validator_rejects_null"`
	Schema                    Type       `json:"schema"`
	Description               *string    `json:"description"`
	Example                   *string    `json:"example"`
	Meta                      *FieldMeta `json:"meta,omitempty"`
}

// FieldMeta carries optional field-level constraints, defaults, and extensions.
type FieldMeta struct {
	Constraints *Constraints  `json:"constraints,omitempty"`
	Default     *LiteralValue `json:"default,omitempty"`
	Format      *string       `json:"format,omitempty"`
	Extensions  []Extension   `json:"extensions,omitempty"`
}

// Constraints carries JSON Schema/OpenAPI validation constraints.
type Constraints struct {
	MinLength        *uint64  `json:"min_length,omitempty"`
	MaxLength        *uint64  `json:"max_length,omitempty"`
	MinItems         *uint64  `json:"min_items,omitempty"`
	MaxItems         *uint64  `json:"max_items,omitempty"`
	Minimum          *string  `json:"minimum,omitempty"`
	Maximum          *string  `json:"maximum,omitempty"`
	ExclusiveMinimum *string  `json:"exclusive_minimum,omitempty"`
	ExclusiveMaximum *string  `json:"exclusive_maximum,omitempty"`
	Pattern          *string  `json:"pattern,omitempty"`
	EnumValues       []string `json:"enum_values,omitempty"`
}

// LiteralValue is an adjacently-tagged literal value mirrored by Rust serde.
type LiteralValue struct {
	Type  string `json:"type"`
	Value any    `json:"value,omitempty"`
}

// Extension carries a vendor extension.
type Extension struct {
	Name  string       `json:"name"`
	Value LiteralValue `json:"value"`
}

// Type kind tags — the adjacently-tagged Type discriminant values. These strings are
// the byte-identical mirror of the snake_case serde variant names of `facts::Type`.
const (
	TypePrimitive = "primitive"
	TypeWellKnown = "well_known"
	TypeArray     = "array"
	TypeMap       = "map"
	TypeNamed     = "named"
	TypeObject    = "object"
	TypeEnum      = "enum"
	TypeUnion     = "union"
	TypeAny       = "any"
)

// Type is the closed, language-neutral type vocabulary (the IR's narrow waist,
// docs/extensibility.md §2a). It mirrors the Rust `facts::Type` enum, which is
// adjacently tagged: `{"type": "<kind>", "of": <payload>}`. Go has no sum types, so
// the variant is encoded as a discriminant string `Type` plus a single `Of` payload
// whose concrete shape depends on the kind:
//
//   - primitive  -> *Prim          ({"prim": "...", ...})
//   - well_known -> string         ("uuid", "date_time", ...)
//   - array      -> *Type          (the element type)
//   - map        -> *MapType       ({"key": Type, "value": Type})
//   - named      -> string         (the referenced schema id)
//   - object     -> []FieldFact
//   - enum       -> []string
//   - union      -> []Type
//   - any        -> emptyObject{}  ({} — an explicit empty payload, never null)
//
// Every kind carries an `of` payload (Any uses an empty object) so the wire form is
// `{"type": ..., "of": ...}` for all variants, matching the Rust encoding.
type Type struct {
	Type string `json:"type"`
	Of   any    `json:"of"`
}

// MapType is the payload of a `map` Type: a key type and a value type.
type MapType struct {
	Key   Type `json:"key"`
	Value Type `json:"value"`
}

// Prim is a base scalar primitive, internally tagged on `prim` (mirroring the Rust
// `facts::Prim`): `{"prim": "string"}`, `{"prim": "int", "bits": 64, "signed": true}`.
// Bits/Signed are pointers so they are omitted for the variants that do not carry
// them (string/bool/bytes).
type Prim struct {
	Prim   string  `json:"prim"`
	Bits   *uint16 `json:"bits,omitempty"`
	Signed *bool   `json:"signed,omitempty"`
}

// Prim tag values — the byte-identical mirror of the snake_case serde variant names
// of `facts::Prim`.
const (
	PrimString = "string"
	PrimBool   = "bool"
	PrimInt    = "int"
	PrimFloat  = "float"
	PrimBytes  = "bytes"
)

// WellKnown values — the byte-identical mirror of the snake_case serde variant names
// of `facts::WellKnown`. A well_known Type's `Of` is one of these strings.
const (
	WellKnownUUID     = "uuid"
	WellKnownDateTime = "date_time"
	WellKnownDate     = "date"
	WellKnownDuration = "duration"
	WellKnownDecimal  = "decimal"
	WellKnownEmail    = "email"
	WellKnownURI      = "uri"
)

// emptyObject marshals as `{}` — the explicit, non-null payload of the `any` Type
// (its Rust counterpart `Type::Any {}` serializes the same way).
type emptyObject struct{}

// PrimitiveType builds a `primitive` Type wrapping the given Prim.
func PrimitiveType(p Prim) Type { return Type{Type: TypePrimitive, Of: &p} }

// WellKnownType builds a `well_known` Type for the given canonical name.
func WellKnownType(name string) Type { return Type{Type: TypeWellKnown, Of: name} }

// ArrayType builds an `array` Type with the given element type.
func ArrayType(elem Type) Type { return Type{Type: TypeArray, Of: &elem} }

// MapTypeOf builds a `map` Type with the given key and value types.
func MapTypeOf(key, value Type) Type {
	return Type{Type: TypeMap, Of: &MapType{Key: key, Value: value}}
}

// NamedType builds a `named` Type referencing the given schema id.
func NamedType(id string) Type { return Type{Type: TypeNamed, Of: id} }

// ObjectType builds an `object` Type with the given fields.
func ObjectType(fields []FieldFact) Type { return Type{Type: TypeObject, Of: fields} }

// EnumType builds an `enum` Type with the given string members.
func EnumType(members []string) Type { return Type{Type: TypeEnum, Of: members} }

// UnionType builds a `union` Type with the given variant types.
func UnionType(variants []Type) Type { return Type{Type: TypeUnion, Of: variants} }

// AnyType builds an `any` Type (a free-form value), with an explicit empty payload.
func AnyType() Type { return Type{Type: TypeAny, Of: emptyObject{}} }

// IntPrim builds a sized integer Prim (e.g. IntPrim(64, true) == an int64).
func IntPrim(bits uint16, signed bool) Prim {
	b, s := bits, signed
	return Prim{Prim: PrimInt, Bits: &b, Signed: &s}
}

// FloatPrim builds a sized float Prim (e.g. FloatPrim(32) == a float32).
func FloatPrim(bits uint16) Prim {
	b := bits
	return Prim{Prim: PrimFloat, Bits: &b}
}

// StringPrim builds a string Prim.
func StringPrim() Prim { return Prim{Prim: PrimString} }

// BoolPrim builds a bool Prim.
func BoolPrim() Prim { return Prim{Prim: PrimBool} }

// BytesPrim builds a bytes Prim.
func BytesPrim() Prim { return Prim{Prim: PrimBytes} }

// TypeRef is a reference to a schema by its stable id.
type TypeRef struct {
	RefID string `json:"ref_id"`
}

// DiagnosticFact is one diagnostic with a source location (D-10 / GO-06).
type DiagnosticFact struct {
	Code      string `json:"code,omitempty"`
	Severity  string `json:"severity"`
	Category  string `json:"category,omitempty"`
	Message   string `json:"message"`
	File      string `json:"file"`
	Line      uint32 `json:"line"`
	EndLine   uint32 `json:"end_line,omitempty"`
	Operation string `json:"operation,omitempty"`
	Schema    string `json:"schema,omitempty"`
	Subject   string `json:"subject,omitempty"`
}

// SourceSpan is the file + line range provenance attached to every node (D-07).
type SourceSpan struct {
	File      string `json:"file"`
	StartLine uint32 `json:"start_line"`
	EndLine   uint32 `json:"end_line"`
}

// Marshal sorts every slice in doc by a stable key and writes pretty-printed JSON to
// w. Sorting before encoding is what makes the output deterministic on unchanged
// source (GRAPH-02). It NEVER ranges a Go map into output order.
//
// Sort keys:
//   - Schemas by Id
//   - each object schema's fields by JsonName; each enum schema's members lexically
//   - Diagnostics by (File, Line, Message)
//   - Routes by (Path, Method)
func Marshal(doc GoFacts, w io.Writer) error {
	sortDoc(&doc)

	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	// SetEscapeHTML(false) keeps message text (e.g. "->") readable and stable.
	enc.SetEscapeHTML(false)
	return enc.Encode(doc)
}

func sortDoc(doc *GoFacts) {
	sort.Slice(doc.Schemas, func(i, j int) bool {
		return doc.Schemas[i].ID < doc.Schemas[j].ID
	})
	for i := range doc.Schemas {
		sortType(&doc.Schemas[i].Body)
	}

	sort.Slice(doc.Diagnostics, func(i, j int) bool {
		di, dj := doc.Diagnostics[i], doc.Diagnostics[j]
		if di.File != dj.File {
			return di.File < dj.File
		}
		if di.Line != dj.Line {
			return di.Line < dj.Line
		}
		return di.Message < dj.Message
	})

	sort.Slice(doc.Routes, func(i, j int) bool {
		ri, rj := doc.Routes[i], doc.Routes[j]
		if ri.Path != rj.Path {
			return ri.Path < rj.Path
		}
		return ri.Method < rj.Method
	})
	for i := range doc.Routes {
		sortRoute(&doc.Routes[i])
	}
}

// sortType recursively orders the deterministic parts of a Type body: an object's
// fields by name and an enum's members lexically, recursing through every type-
// bearing payload. Mirrors the host's `normalize_type` so the two sides agree.
func sortType(t *Type) {
	switch payload := t.Of.(type) {
	case []FieldFact:
		sort.Slice(payload, func(a, b int) bool {
			return payload[a].JSONName < payload[b].JSONName
		})
		for i := range payload {
			sortType(&payload[i].Schema)
			sortFieldMeta(payload[i].Meta)
		}
	case []string:
		sort.Strings(payload)
	case []Type:
		for i := range payload {
			sortType(&payload[i])
		}
	case *Type:
		if payload != nil {
			sortType(payload)
		}
	case *MapType:
		if payload != nil {
			sortType(&payload.Key)
			sortType(&payload.Value)
		}
	}
}

func sortFieldMeta(meta *FieldMeta) {
	if meta == nil {
		return
	}
	sort.Slice(meta.Extensions, func(i, j int) bool {
		return meta.Extensions[i].Name < meta.Extensions[j].Name
	})
}

// sortRoute stably orders every sub-slice of a route so two runs on unchanged source
// are byte-identical (GRAPH-02): params by (name, location), responses by status, and
// each param's type body recursively.
func sortRoute(r *RouteFact) {
	sort.Strings(r.Middleware)
	sort.Slice(r.Params, func(a, b int) bool {
		if r.Params[a].Name != r.Params[b].Name {
			return r.Params[a].Name < r.Params[b].Name
		}
		return r.Params[a].Location < r.Params[b].Location
	})
	for i := range r.Params {
		sortType(&r.Params[i].Schema)
	}
	sort.Slice(r.RequestBodyVariants, func(a, b int) bool {
		return r.RequestBodyVariants[a].ContentType < r.RequestBodyVariants[b].ContentType
	})
	sort.Slice(r.Responses, func(a, b int) bool {
		return r.Responses[a].Status < r.Responses[b].Status
	})
	for i := range r.Responses {
		sort.Slice(r.Responses[i].Headers, func(a, b int) bool {
			return r.Responses[i].Headers[a].Name < r.Responses[i].Headers[b].Name
		})
		for j := range r.Responses[i].Headers {
			sortType(&r.Responses[i].Headers[j].Schema)
		}
	}
}
