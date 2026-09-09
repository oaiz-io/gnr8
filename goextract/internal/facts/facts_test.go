package facts_test

import (
	"bytes"
	"encoding/json"
	"reflect"
	"sort"
	"testing"

	"github.com/gnr8/goextract/internal/facts"
)

// unsortedDoc builds a GoFacts value whose slices are intentionally out of order so
// the test proves Marshal sorts (GRAPH-02 discipline at the sidecar boundary).
func unsortedDoc() facts.GoFacts {
	return facts.GoFacts{
		Module:             "github.com/acme/svc",
		ExtractorToolchain: "go1.26.2",
		Routes:             []facts.RouteFact{}, // empty -> must marshal as [], not null
		Schemas: []facts.SchemaFact{
			{
				ID:   "internal/dto.Zebra",
				Name: "Zebra",
				Body: facts.ObjectType([]facts.FieldFact{
					{JSONName: "yankee", Schema: facts.PrimitiveType(facts.StringPrim())},
					{JSONName: "alpha", Schema: facts.PrimitiveType(facts.StringPrim())},
				}),
				Span: facts.SourceSpan{File: "z.go", StartLine: 1, EndLine: 1},
			},
			{
				ID:   "internal/dto.Apple",
				Name: "Apple",
				Body: facts.EnumType([]string{"lte", "gte"}),
				Span: facts.SourceSpan{File: "a.go", StartLine: 1, EndLine: 1},
			},
		},
		Diagnostics: []facts.DiagnosticFact{
			{Severity: "WARN", Message: "second", File: "b.go", Line: 10},
			{Severity: "WARN", Message: "first", File: "a.go", Line: 5},
		},
	}
}

func TestDeterminism(t *testing.T) {
	var buf1, buf2 bytes.Buffer
	if err := facts.Marshal(unsortedDoc(), &buf1); err != nil {
		t.Fatalf("marshal 1: %v", err)
	}
	if err := facts.Marshal(unsortedDoc(), &buf2); err != nil {
		t.Fatalf("marshal 2: %v", err)
	}
	if !bytes.Equal(buf1.Bytes(), buf2.Bytes()) {
		t.Fatalf("non-deterministic output:\n--- run 1 ---\n%s\n--- run 2 ---\n%s", buf1.String(), buf2.String())
	}
}

func TestSchemasSortedByID(t *testing.T) {
	var buf bytes.Buffer
	if err := facts.Marshal(unsortedDoc(), &buf); err != nil {
		t.Fatalf("marshal: %v", err)
	}
	var out facts.GoFacts
	if err := json.Unmarshal(buf.Bytes(), &out); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(out.Schemas) != 2 {
		t.Fatalf("expected 2 schemas, got %d", len(out.Schemas))
	}
	// Apple < Zebra by id.
	if out.Schemas[0].ID != "internal/dto.Apple" || out.Schemas[1].ID != "internal/dto.Zebra" {
		t.Errorf("schemas not sorted by id: %s, %s", out.Schemas[0].ID, out.Schemas[1].ID)
	}
	// Zebra is an object: its fields must be sorted by json name (alpha < yankee).
	zebra := out.Schemas[1]
	if zebra.Body.Type != facts.TypeObject {
		t.Fatalf("expected Zebra body kind %q, got %q", facts.TypeObject, zebra.Body.Type)
	}
	fieldNames := objectFieldNames(t, zebra.Body)
	if !reflect.DeepEqual(fieldNames, []string{"alpha", "yankee"}) {
		t.Errorf("object fields not sorted by json name: %v", fieldNames)
	}
	// Apple is an enum: its members must be sorted lexically (gte < lte).
	apple := out.Schemas[0]
	if apple.Body.Type != facts.TypeEnum {
		t.Fatalf("expected Apple body kind %q, got %q", facts.TypeEnum, apple.Body.Type)
	}
	members := enumMembers(t, apple.Body)
	if !reflect.DeepEqual(members, []string{"gte", "lte"}) {
		t.Errorf("enum members not sorted: %v", members)
	}
	// Diagnostics sorted by (file, line): a.go:5 before b.go:10.
	if len(out.Diagnostics) != 2 || out.Diagnostics[0].File != "a.go" || out.Diagnostics[1].File != "b.go" {
		t.Errorf("diagnostics not sorted: %+v", out.Diagnostics)
	}
}

func TestEmptySlicesMarshalAsArrays(t *testing.T) {
	var buf bytes.Buffer
	doc := facts.GoFacts{
		Module:      "m",
		Routes:      []facts.RouteFact{},
		Schemas:     []facts.SchemaFact{},
		Diagnostics: []facts.DiagnosticFact{},
	}
	if err := facts.Marshal(doc, &buf); err != nil {
		t.Fatalf("marshal: %v", err)
	}
	s := buf.String()
	// Empty slices must serialize as `[]`, never `null` (stable, non-nil keys).
	for _, key := range []string{`"routes": []`, `"schemas": []`, `"diagnostics": []`} {
		if !bytes.Contains(buf.Bytes(), []byte(key)) {
			t.Errorf("expected %q in output, got:\n%s", key, s)
		}
	}
	if bytes.Contains(buf.Bytes(), []byte("null")) {
		t.Errorf("output must not contain null for empty slices:\n%s", s)
	}
}

// canonicalFieldNames is the EXACT set of json field names (struct tags + adjacent
// enum tag/content keys + Prim/Map payload keys) the Rust serde DTO in
// crates/gnr8-core/src/analyze/facts.rs emits. The drift guard below marshals a
// fully-populated facts value exercising every neutral Type variant + all contract facts, then
// asserts the emitted key set equals this list. A renamed/added/dropped Go json tag
// fails INSIDE 01-01 rather than surfacing as a deny_unknown_fields rejection in 01-02.
var canonicalFieldNames = []string{
	// GoFacts
	"module", "extractor_toolchain", "routes", "schemas", "diagnostics",
	// RouteFact ("description" is shared with FieldFact below; "summary" is route-only)
	"method", "path", "handler", "operation_id", "summary", "group", "middleware", "params",
	"request_body", "request_body_required", "request_body_content_type", "request_body_variants", "responses", "span",
	// ParamFact (name/location/required/schema/span)
	"name", "location", "required", "schema", "default",
	// ResponseFact
	"status", "body", "body_kind", "content_type", "content_types", "headers",
	// SchemaFact (id/name/body/span)
	"id",
	// FieldFact
	"json_name", "serializer_may_omit", "deserializer_accepts_absent",
	"deserializer_accepts_null", "serializer_may_emit_null",
	"validator_requires_presence", "validator_rejects_null",
	"description", "example", "meta",
	// FieldMeta / Constraints / Extension / LiteralValue
	"constraints", "default", "format", "extensions", "min_length", "max_length", "min_items", "max_items", "minimum", "maximum",
	"exclusive_minimum", "exclusive_maximum", "pattern", "enum_values",
	// Type (adjacent tag/content)
	"type", "of",
	// MapType payload
	"key", "value",
	// Prim (internally tagged + sized fields)
	"prim", "bits", "signed",
	// TypeRef
	"ref_id",
	// DiagnosticFact
	"severity", "message", "file", "line",
	// SourceSpan
	"start_line", "end_line",
}

// fullyPopulatedDoc constructs a facts value touching EVERY neutral Type variant
// (primitive incl. sized int/float, well_known, array, map, named, object, enum,
// union, any) and every presence/nullability fact, plus a populated
// route. Used by the drift guard to harvest the complete emitted key set.
func fullyPopulatedDoc() facts.GoFacts {
	desc, ex := "a description", "an example"
	format := "uuid"
	minLen, maxLen := uint64(1), uint64(120)
	minItems, maxItems := uint64(1), uint64(25)
	minimum, maximum := "0", "100"
	exclusiveMinimum, exclusiveMaximum := "-1", "101"
	pattern := "^[a-z]+$"
	objectFields := []facts.FieldFact{
		{
			JSONName: "ratio", SerializerMayOmit: true, DeserializerAcceptsAbsent: false, DeserializerAcceptsNull: true, SerializerMayEmitNull: true, ValidatorRequiresPresence: true, ValidatorRejectsNull: false,
			Schema:      facts.PrimitiveType(facts.FloatPrim(32)),
			Description: &desc, Example: &ex,
			Meta: &facts.FieldMeta{
				Constraints: &facts.Constraints{
					MinLength:        &minLen,
					MaxLength:        &maxLen,
					MinItems:         &minItems,
					MaxItems:         &maxItems,
					Minimum:          &minimum,
					Maximum:          &maximum,
					ExclusiveMinimum: &exclusiveMinimum,
					ExclusiveMaximum: &exclusiveMaximum,
					Pattern:          &pattern,
					EnumValues:       []string{"low", "high"},
				},
				Default: &facts.LiteralValue{Type: "number", Value: "3.14"},
				Format:  &format,
				Extensions: []facts.Extension{
					{Name: "x-gnr8-render", Value: facts.LiteralValue{Type: "string", Value: "slider"}},
				},
			},
		},
		{
			JSONName: "count", SerializerMayOmit: false, DeserializerAcceptsAbsent: false, DeserializerAcceptsNull: false, SerializerMayEmitNull: false, ValidatorRequiresPresence: true, ValidatorRejectsNull: false,
			Schema: facts.PrimitiveType(facts.IntPrim(64, true)),
		},
		{
			JSONName: "name", SerializerMayOmit: false, DeserializerAcceptsAbsent: true, DeserializerAcceptsNull: true, SerializerMayEmitNull: true, ValidatorRequiresPresence: false, ValidatorRejectsNull: false,
			Schema: facts.PrimitiveType(facts.StringPrim()),
		},
		{
			JSONName: "flag", SerializerMayOmit: true, DeserializerAcceptsAbsent: true, DeserializerAcceptsNull: false, SerializerMayEmitNull: false, ValidatorRequiresPresence: false, ValidatorRejectsNull: false,
			Schema: facts.PrimitiveType(facts.BoolPrim()),
		},
		{JSONName: "raw", Schema: facts.PrimitiveType(facts.BytesPrim())},
		{JSONName: "ids", Schema: facts.ArrayType(facts.WellKnownType(facts.WellKnownUUID))},
		{JSONName: "when", Schema: facts.WellKnownType(facts.WellKnownDateTime)},
		{JSONName: "ref", Schema: facts.NamedType("internal/dto.Other")},
		{JSONName: "either", Schema: facts.UnionType([]facts.Type{
			facts.PrimitiveType(facts.StringPrim()),
			facts.NamedType("internal/dto.Other"),
		})},
		{JSONName: "lookup", Schema: facts.MapTypeOf(
			facts.PrimitiveType(facts.StringPrim()),
			facts.AnyType(),
		)},
		{JSONName: "free", Schema: facts.AnyType()},
	}

	return facts.GoFacts{
		Module:             "github.com/acme/svc",
		ExtractorToolchain: "go1.26.2",
		Routes: []facts.RouteFact{
			{
				Method: "PUT", Path: "/{uuid}", Handler: "updateGoal", OperationID: "updateGoal",
				Summary:     "Updates one goal.",
				Description: "The goal must belong to the calling actor.",
				Group:       "goals",
				Middleware:  []string{"RequireActor"},
				Params: []facts.ParamFact{
					{
						Name: "uuid", Location: "path", Required: true,
						Schema:  facts.WellKnownType(facts.WellKnownUUID),
						Default: &facts.LiteralValue{Type: "string", Value: "00000000-0000-0000-0000-000000000000"},
						Span:    facts.SourceSpan{File: "handlers.go", StartLine: 94, EndLine: 94},
					},
				},
				RequestBody:            &facts.TypeRef{RefID: "internal/dto.UpdateGoalInput"},
				RequestBodyRequired:    true,
				RequestBodyContentType: "application/json",
				RequestBodyVariants: []facts.RequestBodyVariantFact{
					{Body: facts.TypeRef{RefID: "internal/dto.UpdateGoalForm"}, ContentType: "multipart/form-data"},
				},
				Responses: []facts.ResponseFact{
					{
						Status: 200, Body: &facts.TypeRef{RefID: "internal/dto.CommandMessage"},
						BodyKind: "json", ContentType: "application/json", ContentTypes: []string{"application/json"},
						Headers: []facts.ResponseHeaderFact{{Name: "X-Request-ID", Schema: facts.PrimitiveType(facts.StringPrim())}},
					},
				},
				Span: facts.SourceSpan{File: "http.go", StartLine: 57, EndLine: 57},
			},
		},
		Schemas: []facts.SchemaFact{
			{
				ID: "internal/dto.Everything", Name: "Everything",
				Body: facts.ObjectType(objectFields),
				Span: facts.SourceSpan{File: "everything.go", StartLine: 1, EndLine: 1},
			},
			{
				ID: "internal/dto.Direction", Name: "Direction",
				Body: facts.EnumType([]string{"gte", "lte"}),
				Span: facts.SourceSpan{File: "common.go", StartLine: 2, EndLine: 2},
			},
		},
		Diagnostics: []facts.DiagnosticFact{
			{Severity: "WARN", Message: "lossy narrowing", File: "everything.go", Line: 3},
		},
	}
}

// TestContractFieldNamesMatchRustDTO is the in-plan contract-drift guard (RESEARCH
// Pitfall 1). It marshals a fully-populated facts value and asserts the COMPLETE set
// of emitted json keys equals the canonical field-name list agreed with the Rust
// serde DTO. Any renamed/added/dropped Go json tag fails here, a wave early.
func TestContractFieldNamesMatchRustDTO(t *testing.T) {
	var buf bytes.Buffer
	if err := facts.Marshal(fullyPopulatedDoc(), &buf); err != nil {
		t.Fatalf("marshal: %v", err)
	}

	var doc any
	if err := json.Unmarshal(buf.Bytes(), &doc); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}

	got := map[string]struct{}{}
	collectKeys(doc, got)

	want := map[string]struct{}{}
	for _, k := range canonicalFieldNames {
		want[k] = struct{}{}
	}

	var missing, extra []string
	for k := range want {
		if _, ok := got[k]; !ok {
			missing = append(missing, k)
		}
	}
	for k := range got {
		if _, ok := want[k]; !ok {
			extra = append(extra, k)
		}
	}
	sort.Strings(missing)
	sort.Strings(extra)

	if len(missing) > 0 {
		t.Errorf("canonical field names NOT emitted by the Go DTO (contract drift): %v", missing)
	}
	if len(extra) > 0 {
		t.Errorf("Go DTO emitted field names NOT in the Rust contract (contract drift): %v", extra)
	}
}

// TestAnyTypeCarriesEmptyObjectPayload pins the `any` wire form to {"type":"any",
// "of":{}} — a non-null empty payload, matching the Rust `Type::Any {}` encoding (a
// bare null/omitted `of` would not round-trip under the host's buffered deserialize).
func TestAnyTypeCarriesEmptyObjectPayload(t *testing.T) {
	b, err := json.Marshal(facts.AnyType())
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if got := string(b); got != `{"type":"any","of":{}}` {
		t.Errorf("any payload mismatch: got %s", got)
	}
}

// TestPrimWireForms pins the internally-tagged Prim encodings against the Rust forms.
func TestPrimWireForms(t *testing.T) {
	cases := map[string]struct {
		ty   facts.Type
		want string
	}{
		"string": {facts.PrimitiveType(facts.StringPrim()), `{"type":"primitive","of":{"prim":"string"}}`},
		"int64":  {facts.PrimitiveType(facts.IntPrim(64, true)), `{"type":"primitive","of":{"prim":"int","bits":64,"signed":true}}`},
		"f32":    {facts.PrimitiveType(facts.FloatPrim(32)), `{"type":"primitive","of":{"prim":"float","bits":32}}`},
	}
	for name, c := range cases {
		b, err := json.Marshal(c.ty)
		if err != nil {
			t.Fatalf("%s: marshal: %v", name, err)
		}
		if got := string(b); got != c.want {
			t.Errorf("%s: got %s, want %s", name, got, c.want)
		}
	}
}

// --- helpers ---

// collectKeys recursively walks a decoded JSON value, recording every object key.
func collectKeys(v any, into map[string]struct{}) {
	switch node := v.(type) {
	case map[string]any:
		for k, child := range node {
			into[k] = struct{}{}
			collectKeys(child, into)
		}
	case []any:
		for _, child := range node {
			collectKeys(child, into)
		}
	}
}

// objectFieldNames extracts the ordered json_name list from an object Type body that
// has been round-tripped through json.Unmarshal (so Of is a []any of maps).
func objectFieldNames(t *testing.T, body facts.Type) []string {
	t.Helper()
	arr, ok := body.Of.([]any)
	if !ok {
		t.Fatalf("object body payload is not an array: %T", body.Of)
	}
	names := make([]string, 0, len(arr))
	for _, f := range arr {
		m, ok := f.(map[string]any)
		if !ok {
			t.Fatalf("object field is not a map: %T", f)
		}
		name, ok := m["json_name"].(string)
		if !ok {
			t.Fatalf("object field missing json_name: %v", m)
		}
		names = append(names, name)
	}
	return names
}

// enumMembers extracts the ordered member list from an enum Type body that has been
// round-tripped through json.Unmarshal (so Of is a []any of strings).
func enumMembers(t *testing.T, body facts.Type) []string {
	t.Helper()
	arr, ok := body.Of.([]any)
	if !ok {
		t.Fatalf("enum body payload is not an array: %T", body.Of)
	}
	members := make([]string, 0, len(arr))
	for _, m := range arr {
		s, ok := m.(string)
		if !ok {
			t.Fatalf("enum member is not a string: %T", m)
		}
		members = append(members, s)
	}
	return members
}
