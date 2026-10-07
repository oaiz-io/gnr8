package types_test

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/types"
)

// fixtureDir resolves the real goalservice fixture from this test file's location
// (../../../fixtures/goalservice relative to goextract/internal/types).
func fixtureDir(t *testing.T) string {
	t.Helper()
	abs, err := filepath.Abs(filepath.Join("..", "..", "..", "fixtures", "goalservice"))
	if err != nil {
		t.Fatalf("resolve fixture dir: %v", err)
	}
	return abs
}

func extractFixture(t *testing.T) ([]facts.SchemaFact, []facts.DiagnosticFact) {
	t.Helper()
	res, err := load.Load(fixtureDir(t))
	if err != nil {
		t.Fatalf("load fixture: %v", err)
	}
	diags := diag.New()
	schemas := types.Extract(res, diags)
	return schemas, diags.Items()
}

func schemaByName(schemas []facts.SchemaFact, name string) (facts.SchemaFact, bool) {
	for _, s := range schemas {
		if s.Name == name {
			return s, true
		}
	}
	return facts.SchemaFact{}, false
}

// objectFields returns a named schema's object fields, or nil if its body is not an
// object (the neutral Type body carries fields as Type{Type: "object", Of: []FieldFact}).
func objectFields(s facts.SchemaFact) []facts.FieldFact {
	if s.Body.Type != facts.TypeObject {
		return nil
	}
	fields, _ := s.Body.Of.([]facts.FieldFact)
	return fields
}

// enumMembers returns a named schema's enum members, or nil if its body is not an enum.
func enumMembers(s facts.SchemaFact) []string {
	if s.Body.Type != facts.TypeEnum {
		return nil
	}
	members, _ := s.Body.Of.([]string)
	return members
}

func fieldByJSON(s facts.SchemaFact, jsonName string) (facts.FieldFact, bool) {
	for _, f := range objectFields(s) {
		if f.JSONName == jsonName {
			return f, true
		}
	}
	return facts.FieldFact{}, false
}

// primName returns the Prim tag of a primitive Type (e.g. "string", "int"), or "".
func primName(ty facts.Type) string {
	if ty.Type != facts.TypePrimitive {
		return ""
	}
	if p, ok := ty.Of.(*facts.Prim); ok {
		return p.Prim
	}
	return ""
}

// wellKnownName returns the canonical name of a well_known Type (e.g. "uuid"), or "".
func wellKnownName(ty facts.Type) string {
	if ty.Type != facts.TypeWellKnown {
		return ""
	}
	name, _ := ty.Of.(string)
	return name
}

func TestExtractObjectAndEnumCounts(t *testing.T) {
	schemas, _ := extractFixture(t)

	wantObjects := []string{
		"CreateGoalInput", "UpdateGoalInput", "GoalResponse", "ListGoalsOutput",
		"GoalAnalyticsQuery", "HttpError", "CommandMessage", "CommandMessageWithUUID",
	}
	var objects, enums []string
	for _, s := range schemas {
		switch s.Body.Type {
		case facts.TypeObject:
			objects = append(objects, s.Name)
		case facts.TypeEnum:
			enums = append(enums, s.Name)
		default:
			t.Errorf("unexpected schema body kind %q for %s", s.Body.Type, s.Name)
		}
	}
	sort.Strings(objects)
	sort.Strings(wantObjects)

	if len(objects) != len(wantObjects) {
		t.Fatalf("expected %d object schemas, got %d: %v", len(wantObjects), len(objects), objects)
	}
	for i := range wantObjects {
		if objects[i] != wantObjects[i] {
			t.Errorf("object schema mismatch at %d: want %s got %s", i, wantObjects[i], objects[i])
		}
	}

	if len(enums) != 1 || enums[0] != "TargetDirection" {
		t.Fatalf("expected exactly the TargetDirection enum, got %v", enums)
	}

	dir, ok := schemaByName(schemas, "TargetDirection")
	if !ok {
		t.Fatal("TargetDirection enum not found")
	}
	members := enumMembers(dir)
	if len(members) != 2 || members[0] != "gte" || members[1] != "lte" {
		t.Errorf("expected sorted enum members [gte lte], got %v", members)
	}
}

func TestCreateGoalInputFields(t *testing.T) {
	schemas, _ := extractFixture(t)
	s, ok := schemaByName(schemas, "CreateGoalInput")
	if !ok {
		t.Fatal("CreateGoalInput not found")
	}

	// name: required, not optional, string.
	name, ok := fieldByJSON(s, "name")
	if !ok {
		t.Fatal("field 'name' not found")
	}
	if !name.ValidatorRequiresPresence || name.SerializerMayOmit {
		t.Errorf("name: want required=true optional=false, got required=%v optional=%v", name.ValidatorRequiresPresence, name.SerializerMayOmit)
	}
	if got := primName(name.Schema); got != facts.PrimString {
		t.Errorf("name type: want primitive string, got %q (%+v)", got, name.Schema)
	}

	// targetValue: optional+nullable (a pointer), float primitive.
	tv, ok := fieldByJSON(s, "targetValue")
	if !ok {
		t.Fatal("field 'targetValue' not found")
	}
	if !tv.SerializerMayOmit || tv.ValidatorRequiresPresence {
		t.Errorf("targetValue: want optional=true required=false, got optional=%v required=%v", tv.SerializerMayOmit, tv.ValidatorRequiresPresence)
	}
	if !tv.DeserializerAcceptsNull {
		t.Errorf("targetValue: a pointer field must be nullable, got nullable=%v", tv.DeserializerAcceptsNull)
	}
	if got := primName(tv.Schema); got != facts.PrimFloat {
		t.Errorf("targetValue type: want primitive float, got %q (%+v)", got, tv.Schema)
	}

	// workflowChainIds: array of well-known uuid.
	wc, ok := fieldByJSON(s, "workflowChainIds")
	if !ok {
		t.Fatal("field 'workflowChainIds' not found")
	}
	if wc.Schema.Type != facts.TypeArray {
		t.Fatalf("workflowChainIds: want array, got %+v", wc.Schema)
	}
	elem, ok := wc.Schema.Of.(*facts.Type)
	if !ok || elem == nil {
		t.Fatalf("workflowChainIds: array element missing, got %+v", wc.Schema.Of)
	}
	if got := wellKnownName(*elem); got != facts.WellKnownUUID {
		t.Errorf("workflowChainIds element: want well-known uuid, got %q (%+v)", got, *elem)
	}

	// analyticsQuery: named ref to GoalAnalyticsQuery schema.
	aq, ok := fieldByJSON(s, "analyticsQuery")
	if !ok {
		t.Fatal("field 'analyticsQuery' not found")
	}
	if aq.Schema.Type != facts.TypeNamed {
		t.Fatalf("analyticsQuery: want named ref, got %+v", aq.Schema)
	}
	if id, _ := aq.Schema.Of.(string); id != "internal/common/dto.GoalAnalyticsQuery" {
		t.Errorf("analyticsQuery named id: want internal/common/dto.GoalAnalyticsQuery, got %v", aq.Schema.Of)
	}
}

func TestValidateRequiredTagsMarkFieldsRequired(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(
		filepath.Join(dir, "go.mod"),
		[]byte("module example.com/validatefixture\n\ngo 1.22\n"),
		0o644,
	); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	if err := os.WriteFile(
		filepath.Join(dir, "models.go"),
		[]byte(`package validatefixture

type FileRef struct {
	FileID   string  `+"`json:\"fileId\" validate:\"required\"`"+`
	Filename string  `+"`json:\"filename\" validate:\"required,email\"`"+`
	Label    string  `+"`json:\"label,omitempty\"`"+`
	Note     *string `+"`json:\"note\"`"+`
}

type ScopedRules struct {
	Headers  map[string]string `+"`json:\"headers,omitzero\" binding:\"omitempty,dive,keys,required,endkeys,required\"`"+`
	Elements []string          `+"`json:\"elements\" validate:\"dive,required\"`"+`
	Segments []string          `+"`json:\"segments\" validate:\"required,dive,min=1,max=100\"`"+`
	Nested   [][]string        `+"`json:\"nested\" validate:\"dive,dive,required\"`"+`
	Kinds    []string          `+"`json:\"kinds\" validate:\"dive,oneof=alpha beta\"`"+`
}

type ElementRules struct {
	Recipients []string `+"`json:\"recipients\" validate:\"omitempty,dive,email\"`"+`
}
`),
		0o644,
	); err != nil {
		t.Fatalf("write models.go: %v", err)
	}

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load validate fixture: %v", err)
	}
	diags := diag.New()
	schemas := types.Extract(res, diags)
	s, ok := schemaByName(schemas, "FileRef")
	if !ok {
		t.Fatal("FileRef not found")
	}

	fileID, ok := fieldByJSON(s, "fileId")
	if !ok {
		t.Fatal("field 'fileId' not found")
	}
	if !fileID.ValidatorRequiresPresence {
		t.Fatal("validate:\"required\" should mark fileId required")
	}
	filename, ok := fieldByJSON(s, "filename")
	if !ok {
		t.Fatal("field 'filename' not found")
	}
	if !filename.ValidatorRequiresPresence {
		t.Fatal("validate:\"required,email\" should mark filename required")
	}
	label, ok := fieldByJSON(s, "label")
	if !ok {
		t.Fatal("field 'label' not found")
	}
	if !label.SerializerMayOmit || !label.DeserializerAcceptsNull || label.SerializerMayEmitNull {
		t.Fatalf("omitempty should make output optional/non-null while encoding/json still accepts inbound null, got omit=%v accepts-null=%v emits-null=%v", label.SerializerMayOmit, label.DeserializerAcceptsNull, label.SerializerMayEmitNull)
	}
	note, ok := fieldByJSON(s, "note")
	if !ok {
		t.Fatal("field 'note' not found")
	}
	if !note.DeserializerAcceptsNull || note.ValidatorRequiresPresence {
		t.Fatalf("pointer should be nullable without forcing required, got nullable=%v required=%v", note.DeserializerAcceptsNull, note.ValidatorRequiresPresence)
	}

	// `dive` and `keys`…`endkeys` move the rules that follow them off the field and
	// onto what the field contains, so a `required` past either one says nothing
	// about whether the key is present.
	scoped, ok := schemaByName(schemas, "ScopedRules")
	if !ok {
		t.Fatal("ScopedRules not found")
	}
	scopedCases := []struct {
		jsonName string
		required bool
		why      string
	}{
		{"headers", false, `required inside keys/endkeys constrains the map's keys and values`},
		{"elements", false, `required after dive constrains the slice elements`},
		{"segments", true, `the required before dive constrains the field itself`},
		{"nested", false, `required behind nested dives constrains the innermost elements`},
	}
	for _, tc := range scopedCases {
		field, found := fieldByJSON(scoped, tc.jsonName)
		if !found {
			t.Fatalf("field %q not found", tc.jsonName)
		}
		if field.ValidatorRequiresPresence != tc.required {
			t.Errorf("%s: want required=%v, got %v — %s", tc.jsonName, tc.required, field.ValidatorRequiresPresence, tc.why)
		}
	}

	// The per-element rules in `dive,min=1,max=100` and `dive,oneof=alpha beta`
	// describe each string, not the slice. Constraints lower onto the field's own
	// schema object, so leaking them upward publishes an array whose own value must
	// be 3 characters long or equal to "alpha" — neither of which the author wrote.
	for _, jsonName := range []string{"segments", "kinds"} {
		field, found := fieldByJSON(scoped, jsonName)
		if !found {
			t.Fatalf("field %q not found", jsonName)
		}
		if field.Meta != nil && field.Meta.Constraints != nil {
			t.Errorf("%s: post-dive rules must not bind the container, got %#v", jsonName, field.Meta.Constraints)
		}
	}

	// Every rule in ScopedRules is one gnr8 knows — the scope markers are structural
	// and the rest belong to values the graph does not carry. None of that is
	// unresolved source, so the whole struct must extract quietly.
	for _, d := range diags.Items() {
		if d.Schema == "ScopedRules" {
			t.Errorf("scoped tags must not produce a diagnostic: [%s] %s", d.Code, d.Message)
		}
	}

	// Recognition is a separate question from scope. `email` is a rule gnr8 does not
	// lower, so it is still reported behind a `dive` — the author loses the rule
	// either way, and whether they hear about it must not depend on where they wrote
	// it.
	var reported bool
	for _, d := range diags.Items() {
		if d.Schema == "ElementRules" && strings.Contains(d.Message, "email") {
			reported = true
		}
	}
	if !reported {
		t.Errorf("an unknown rule behind dive must still be reported, got %#v", diags.Items())
	}
}

func TestJSONOmissionOptionsMarkNonPointerFieldsOptional(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(
		filepath.Join(dir, "go.mod"),
		[]byte("module example.com/omissionfixture\n\ngo 1.22\n"),
		0o644,
	); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	if err := os.WriteFile(
		filepath.Join(dir, "models.go"),
		[]byte(`package omissionfixture

type Payload struct {
	EmptyValues []string          `+"`json:\"emptyValues,omitempty\"`"+`
	ZeroValues  []string          `+"`json:\"zeroValues,omitzero\"`"+`
	ZeroMap     map[string]string `+"`json:\"zeroMap,omitzero\"`"+`
	FormZero    []string          `+"`form:\"formZero,omitzero\"`"+`
}
`),
		0o644,
	); err != nil {
		t.Fatalf("write models.go: %v", err)
	}

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load omission fixture: %v", err)
	}
	diags := diag.New()
	schemas := types.Extract(res, diags)
	s, ok := schemaByName(schemas, "Payload")
	if !ok {
		t.Fatal("Payload not found")
	}

	// All three are optional (the option omits the key for a slice/map) AND nullable:
	// the axes are independent, and a nil slice or map is written as `null` whether or
	// not the key can also be dropped.
	for _, name := range []string{"emptyValues", "zeroValues", "zeroMap"} {
		field, found := fieldByJSON(s, name)
		if !found {
			t.Fatalf("field %q not found", name)
		}
		if !field.SerializerMayOmit || !field.DeserializerAcceptsNull {
			t.Fatalf("%s should be optional and nullable, got optional=%v nullable=%v", name, field.SerializerMayOmit, field.DeserializerAcceptsNull)
		}
	}

	formZero, ok := fieldByJSON(s, "formZero")
	if !ok {
		t.Fatal("field 'formZero' not found")
	}
	if formZero.SerializerMayOmit || formZero.DeserializerAcceptsNull {
		t.Fatalf("form omitzero is not a JSON omission signal, got optional=%v nullable=%v", formZero.SerializerMayOmit, formZero.DeserializerAcceptsNull)
	}
}

// TestPresenceAndNullabilityMatchEncodingJSON pins both axes to what
// `encoding/json` actually does, for every shape it distinguishes.
//
// The expectations are not derived from the extractor's rules — they are the
// observed output of `json.Marshal(Matrix{})` on go1.26.5, verbatim and in
// declaration order, so the test fails if the extractor and the marshaller ever
// disagree again. An ABSENT key below is serializer-optional; a `null` value is
// output-nullable. Independently, `json.Unmarshal` accepts an explicit null for
// every field below, including ordinary value fields, by leaving non-nilable
// destinations unchanged:
//
//	{"bare_str":"","bare_ptr":null,"bare_slice":null,"bare_map":null,
//	 "bare_iface":null,"bare_struct":{"a":0},"omit_struct":{"a":0},
//	 "bare_time":"0001-01-01T00:00:00Z","omit_time":"0001-01-01T00:00:00Z",
//	 "bare_arr":["",""],"omit_arr":["",""],"bare_custom":null,
//	 "fake_custom":"","req_str":"","req_time":"0001-01-01T00:00:00Z",
//	 "req_struct":{"a":0},"req_custom":"","req_custom_ptr":null,
//	 "req_ptr":null,"req_slice":null,"req_raw":null,"req_raw_ptr":null}
//
// The eight keys absent from that object — omit_str, omit_ptr, omit_slice,
// omit_map, zero_map, zero_struct, zero_time, omit_custom — are the only
// serializer-optional fields.
//
// Two families of mistake are pinned by construction. The DECLARED TYPE is not
// evidence for presence: a bare pointer keeps its key, so `*T` alone never makes
// a field optional. And the OMISSION OPTION is not evidence on its own:
// `,omitempty` drops only encoding/json's "empty" set, so it is a no-op on a
// struct, a `time.Time`, and an array of non-zero length.
func TestPresenceAndNullabilityMatchEncodingJSON(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(
		filepath.Join(dir, "go.mod"),
		[]byte("module example.com/matrixfixture\n\ngo 1.24\n"),
		0o644,
	); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	if err := os.WriteFile(
		filepath.Join(dir, "models.go"),
		[]byte(`package matrixfixture

import (
	"encoding/json"
	"time"
)

type Inner struct {
	A int `+"`json:\"a\"`"+`
}

type NullString string

func (NullString) MarshalJSON() ([]byte, error) {
	return []byte("null"), nil
}

type FakeCustom string

func (FakeCustom) MarshalJSON() {}

type CustomInput string

func (*CustomInput) UnmarshalJSON([]byte) error { return nil }

type Matrix struct {
	BareStr    string            `+"`json:\"bare_str\"`"+`
	OmitStr    string            `+"`json:\"omit_str,omitempty\"`"+`
	BarePtr    *int              `+"`json:\"bare_ptr\"`"+`
	OmitPtr    *int              `+"`json:\"omit_ptr,omitempty\"`"+`
	BareSlice  []string          `+"`json:\"bare_slice\"`"+`
	OmitSlice  []string          `+"`json:\"omit_slice,omitempty\"`"+`
	BareMap    map[string]string `+"`json:\"bare_map\"`"+`
	OmitMap    map[string]string `+"`json:\"omit_map,omitempty\"`"+`
	ZeroMap    map[string]string `+"`json:\"zero_map,omitzero\"`"+`
	BareIface  any               `+"`json:\"bare_iface\"`"+`
	BareStruct Inner             `+"`json:\"bare_struct\"`"+`
	OmitStruct Inner             `+"`json:\"omit_struct,omitempty\"`"+`
	ZeroStruct Inner             `+"`json:\"zero_struct,omitzero\"`"+`
	BareTime   time.Time         `+"`json:\"bare_time\"`"+`
	OmitTime   time.Time         `+"`json:\"omit_time,omitempty\"`"+`
	ZeroTime   time.Time         `+"`json:\"zero_time,omitzero\"`"+`
	BareArr    [2]string         `+"`json:\"bare_arr\"`"+`
	OmitArr    [2]string         `+"`json:\"omit_arr,omitempty\"`"+`
	BareCustom NullString        `+"`json:\"bare_custom\"`"+`
	OmitCustom NullString        `+"`json:\"omit_custom,omitempty\"`"+`
	FakeCustom FakeCustom        `+"`json:\"fake_custom\"`"+`
	ReqStr     string            `+"`json:\"req_str\" binding:\"required\"`"+`
	ReqTime    time.Time         `+"`json:\"req_time\" binding:\"required\"`"+`
	ReqStruct  Inner             `+"`json:\"req_struct\" binding:\"required\"`"+`
	ReqCustom  CustomInput       `+"`json:\"req_custom\" binding:\"required\"`"+`
	ReqCustPtr *CustomInput      `+"`json:\"req_custom_ptr\" binding:\"required\"`"+`
	ReqPtr     *int              `+"`json:\"req_ptr\" binding:\"required\"`"+`
	ReqSlice   []string          `+"`json:\"req_slice\" binding:\"required\"`"+`
	ReqRaw     json.RawMessage   `+"`json:\"req_raw\" binding:\"required\"`"+`
	ReqRawPtr  *json.RawMessage  `+"`json:\"req_raw_ptr\" binding:\"required\"`"+`
}
`),
		0o644,
	); err != nil {
		t.Fatalf("write models.go: %v", err)
	}

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load matrix fixture: %v", err)
	}
	if len(res.Errors) != 0 {
		t.Fatalf("load matrix fixture errors: %+v", res.Errors)
	}
	diags := diag.New()
	schemas := types.Extract(res, diags)
	s, ok := schemaByName(schemas, "Matrix")
	if !ok {
		t.Fatal("Matrix not found")
	}

	for _, tc := range []struct {
		field          string
		optional       bool // json.Marshal omits the key for the zero value
		outputNullable bool // a present outbound key can carry null
	}{
		{"bare_str", false, false},
		{"omit_str", true, false},
		// A bare pointer keeps its key and holds null: NOT optional, nullable.
		{"bare_ptr", false, true},
		{"omit_ptr", true, false},
		// A nil slice/map/interface is written as null even with no option.
		{"bare_slice", false, true},
		{"omit_slice", true, false},
		{"bare_map", false, true},
		{"omit_map", true, false},
		{"zero_map", true, false},
		{"bare_iface", false, true},
		{"bare_struct", false, false},
		// `,omitempty` is a no-op on a struct, a time.Time, and a [2]string.
		{"omit_struct", false, false},
		{"zero_struct", true, false},
		{"bare_time", false, false},
		{"omit_time", false, false},
		{"zero_time", true, false},
		{"bare_arr", false, false},
		{"omit_arr", false, false},
		// A correctly-shaped custom marshaler can emit null from a non-zero value;
		// a same-named method with the wrong signature is not the JSON interface.
		{"bare_custom", false, true},
		{"omit_custom", true, true},
		{"fake_custom", false, false},
		// A validation tag governs the request direction; it does not change what
		// the marshaller writes, so it moves neither axis.
		{"req_str", false, false},
		{"req_time", false, false},
		{"req_struct", false, false},
		{"req_custom", false, false},
		{"req_custom_ptr", false, true},
		{"req_ptr", false, true},
		{"req_slice", false, true},
		{"req_raw", false, true},
		{"req_raw_ptr", false, true},
	} {
		field, found := fieldByJSON(s, tc.field)
		if !found {
			t.Fatalf("field %q not found", tc.field)
		}
		if field.SerializerMayOmit != tc.optional || !field.DeserializerAcceptsNull {
			t.Errorf(
				"%s: want serializer optional=%v and deserializer null acceptance, got optional=%v accepts-null=%v",
				tc.field, tc.optional, field.SerializerMayOmit, field.DeserializerAcceptsNull,
			)
		}
		if field.SerializerMayEmitNull != tc.outputNullable {
			t.Errorf("%s: want output nullable=%v, got %v", tc.field, tc.outputNullable, field.SerializerMayEmitNull)
		}
	}

	reqStr, _ := fieldByJSON(s, "req_str")
	if !reqStr.ValidatorRequiresPresence || !reqStr.ValidatorRejectsNull {
		t.Fatalf("required scalar must reject the zero value left by explicit null: %+v", reqStr)
	}
	reqTime, _ := fieldByJSON(s, "req_time")
	if !reqTime.ValidatorRequiresPresence || !reqTime.ValidatorRejectsNull {
		t.Fatalf("required time.Time must reject the zero value left by explicit null: %+v", reqTime)
	}
	reqStruct, _ := fieldByJSON(s, "req_struct")
	if !reqStruct.ValidatorRequiresPresence || reqStruct.ValidatorRejectsNull {
		t.Fatalf("Gin's validator skips required on a non-pointer nested struct: %+v", reqStruct)
	}
	reqCustom, _ := fieldByJSON(s, "req_custom")
	if !reqCustom.ValidatorRequiresPresence || reqCustom.ValidatorRejectsNull {
		t.Fatalf("a value custom unmarshaler owns explicit-null behavior: %+v", reqCustom)
	}
	reqCustomPtr, _ := fieldByJSON(s, "req_custom_ptr")
	if !reqCustomPtr.ValidatorRequiresPresence || !reqCustomPtr.ValidatorRejectsNull {
		t.Fatalf("encoding/json sets a pointer field to nil before a custom unmarshaler can run: %+v", reqCustomPtr)
	}
	reqSlice, _ := fieldByJSON(s, "req_slice")
	if !reqSlice.ValidatorRequiresPresence || !reqSlice.ValidatorRejectsNull {
		t.Fatalf("required slice must retain presence and explicit-null rejection: %+v", reqSlice)
	}
	reqRaw, _ := fieldByJSON(s, "req_raw")
	if !reqRaw.ValidatorRequiresPresence || reqRaw.ValidatorRejectsNull {
		t.Fatalf("RawMessage literal null remains non-nil and must not gain a rejection fact: %+v", reqRaw)
	}
	reqRawPtr, _ := fieldByJSON(s, "req_raw_ptr")
	if !reqRawPtr.ValidatorRequiresPresence || !reqRawPtr.ValidatorRejectsNull {
		t.Fatalf("a pointer to RawMessage is set to nil for explicit null: %+v", reqRawPtr)
	}
	for name, field := range map[string]facts.FieldFact{
		"req_raw":     reqRaw,
		"req_raw_ptr": reqRawPtr,
	} {
		if field.Schema.Type != facts.TypeAny {
			t.Errorf("%s: RawMessage must remain free-form JSON, got schema %+v", name, field.Schema)
		}
	}

	// The three no-op `,omitempty` fields are the ones worth telling the author
	// about; nothing else is.
	var reported []string
	for _, d := range diags.Items() {
		if d.Code == "schema.omit_option.ineffective" {
			reported = append(reported, d.Subject)
		}
	}
	sort.Strings(reported)
	want := []string{"OmitArr", "OmitStruct", "OmitTime"}
	if !reflect.DeepEqual(reported, want) {
		t.Errorf("ineffective-omitempty diagnostics: want %v, got %v", want, reported)
	}
}

// An omission option narrows what a present key can carry; it must never widen it.
// A type parameter's underlying type is its constraint, so reading that constraint
// would make `,omitempty` the only thing that turns a generic field nullable.
func TestOmissionOptionDoesNotMakeATypeParameterNullable(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(
		filepath.Join(dir, "go.mod"),
		[]byte("module example.com/genericfixture\n\ngo 1.22\n"),
		0o644,
	); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	if err := os.WriteFile(
		filepath.Join(dir, "models.go"),
		[]byte(`package genericfixture

type Box[T any] struct {
	Tagged   T `+"`json:\"tagged,omitempty\"`"+`
	Untagged T `+"`json:\"untagged\"`"+`
}
`),
		0o644,
	); err != nil {
		t.Fatalf("write models.go: %v", err)
	}

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load generic fixture: %v", err)
	}
	schemas := types.Extract(res, diag.New())
	s, ok := schemaByName(schemas, "Box")
	if !ok {
		t.Fatal("Box not found")
	}
	tagged, ok := fieldByJSON(s, "tagged")
	if !ok {
		t.Fatal("field 'tagged' not found")
	}
	if !tagged.SerializerMayOmit {
		t.Errorf("`,omitempty` on a type parameter is taken at its word: %+v", tagged)
	}
	if tagged.SerializerMayEmitNull {
		t.Errorf("what a `T` writes depends on the instantiation, so it is not null: %+v", tagged)
	}
	untagged, _ := fieldByJSON(s, "untagged")
	if untagged.SerializerMayEmitNull {
		t.Errorf("the untagged twin must agree: %+v", untagged)
	}
}

func TestOmitZeroMapOmitsNilButEmitsAllocatedEmpty(t *testing.T) {
	type Payload struct {
		Value map[string]string `json:"value,omitzero"`
	}

	nilValue, err := json.Marshal(Payload{})
	if err != nil {
		t.Fatalf("marshal nil map: %v", err)
	}
	if string(nilValue) != `{}` {
		t.Fatalf("nil map should be omitted, got %s", nilValue)
	}

	allocated, err := json.Marshal(Payload{Value: map[string]string{}})
	if err != nil {
		t.Fatalf("marshal allocated empty map: %v", err)
	}
	if string(allocated) != `{"value":{}}` {
		t.Fatalf("allocated empty map should be emitted non-null, got %s", allocated)
	}
}

func TestFormDTOAndMultipartFileMetadata(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(
		filepath.Join(dir, "go.mod"),
		[]byte("module example.com/uploadfixture\n\ngo 1.22\n"),
		0o644,
	); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	if err := os.WriteFile(
		filepath.Join(dir, "models.go"),
		[]byte(`package uploadfixture

import "mime/multipart"

type UploadForm struct {
	File        *multipart.FileHeader `+"`form:\"file\" binding:\"required\" description:\"CSV upload\"`"+`
	Title       string                `+"`form:\"title\" validate:\"required,min=3\" schema:\"example=June report,format=slug\"`"+`
	Visibility  string                `+"`form:\"visibility\" enums:\"private,public\" default:\"private\"`"+`
	Attachments []*multipart.FileHeader `+"`form:\"attachments,omitempty\"`"+`
}
`),
		0o644,
	); err != nil {
		t.Fatalf("write models.go: %v", err)
	}

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load upload fixture: %v", err)
	}
	diags := diag.New()
	schemas := types.Extract(res, diags)
	s, ok := schemaByName(schemas, "UploadForm")
	if !ok {
		t.Fatal("UploadForm form-tag DTO not found")
	}

	file, ok := fieldByJSON(s, "file")
	if !ok {
		t.Fatal("field 'file' not found")
	}
	// A multipart part is present or absent; there is no `null` to write, so the
	// VALUE axis does not apply on a form wire even for a pointer field. The
	// PRESENCE axis is asserted here too: a pointer part stays optional, which is
	// the form binder's rule and not `encoding/json`'s. Leaving it unasserted once
	// let it flip silently.
	if !file.ValidatorRequiresPresence || !file.SerializerMayOmit || file.DeserializerAcceptsNull || primName(file.Schema) != facts.PrimBytes {
		t.Fatalf(
			"file should be required optional non-nullable bytes, got required=%v optional=%v nullable=%v schema=%+v",
			file.ValidatorRequiresPresence, file.SerializerMayOmit, file.DeserializerAcceptsNull, file.Schema,
		)
	}
	if file.Description == nil || *file.Description != "CSV upload" {
		t.Fatalf("description tag not preserved: %#v", file.Description)
	}

	title, ok := fieldByJSON(s, "title")
	if !ok {
		t.Fatal("field 'title' not found")
	}
	if !title.ValidatorRequiresPresence || title.Meta == nil || title.Meta.Constraints == nil || title.Meta.Constraints.MinLength == nil || *title.Meta.Constraints.MinLength != 3 {
		t.Fatalf("validate min/required metadata not preserved: %+v", title)
	}
	if title.Example == nil || *title.Example != "June report" {
		t.Fatalf("schema example not preserved: %#v", title.Example)
	}
	if title.Meta.Format == nil || *title.Meta.Format != "slug" {
		t.Fatalf("schema format not preserved: %#v", title.Meta)
	}

	visibility, ok := fieldByJSON(s, "visibility")
	if !ok {
		t.Fatal("field 'visibility' not found")
	}
	if visibility.Meta == nil || visibility.Meta.Constraints == nil || len(visibility.Meta.Constraints.EnumValues) != 2 {
		t.Fatalf("enums tag not preserved: %+v", visibility)
	}
	if visibility.Meta.Default == nil || visibility.Meta.Default.Type != "string" || visibility.Meta.Default.Value != "private" {
		t.Fatalf("default tag not preserved: %#v", visibility.Meta.Default)
	}

	attachments, ok := fieldByJSON(s, "attachments")
	if !ok {
		t.Fatal("field 'attachments' not found")
	}
	if !attachments.SerializerMayOmit || attachments.DeserializerAcceptsNull {
		t.Fatalf("omitempty file slice should be optional but not nullable, got optional=%v nullable=%v", attachments.SerializerMayOmit, attachments.DeserializerAcceptsNull)
	}
	if attachments.Schema.Type != facts.TypeArray {
		t.Fatalf("attachments should be an array, got %+v", attachments.Schema)
	}
	elem, ok := attachments.Schema.Of.(*facts.Type)
	if !ok || elem == nil || primName(*elem) != facts.PrimBytes {
		t.Fatalf("attachments element should be bytes, got %+v", attachments.Schema.Of)
	}
}

func TestEmbeddedFlattening(t *testing.T) {
	schemas, _ := extractFixture(t)
	s, ok := schemaByName(schemas, "CommandMessageWithUUID")
	if !ok {
		t.Fatal("CommandMessageWithUUID not found")
	}
	if _, ok := fieldByJSON(s, "message"); !ok {
		t.Error("embedded CommandMessage.message not flattened into CommandMessageWithUUID")
	}
	if _, ok := fieldByJSON(s, "uuid"); !ok {
		t.Error("uuid field missing from CommandMessageWithUUID")
	}
}

func TestGoalResponseWellKnownAndFreeFormMap(t *testing.T) {
	schemas, diags := extractFixture(t)
	s, ok := schemaByName(schemas, "GoalResponse")
	if !ok {
		t.Fatal("GoalResponse not found")
	}

	createdAt, ok := fieldByJSON(s, "createdAt")
	if !ok {
		t.Fatal("createdAt field not found")
	}
	if got := wellKnownName(createdAt.Schema); got != facts.WellKnownDateTime {
		t.Errorf("createdAt: want well-known date_time, got %q (%+v)", got, createdAt.Schema)
	}

	metadata, ok := fieldByJSON(s, "metadata")
	if !ok {
		t.Fatal("metadata field not found")
	}
	// A free-form Go map lowers to the neutral Any type (explicitly lossy).
	if metadata.Schema.Type != facts.TypeAny {
		t.Errorf("metadata: want any (free-form), got %+v", metadata.Schema)
	}

	// A free-form-map diagnostic for GoalResponse.Metadata must exist.
	if !hasDiag(diags, "free-form map field", "Metadata") {
		t.Errorf("expected free-form-map diagnostic for GoalResponse.Metadata, got %v", diags)
	}
}

func TestFloat64WidthIsPreserved(t *testing.T) {
	schemas, diags := extractFixture(t)
	owners := []string{"CreateGoalInput", "UpdateGoalInput", "GoalResponse"}
	for _, owner := range owners {
		schema, ok := schemaByName(schemas, owner)
		if !ok {
			t.Fatalf("schema %s not found", owner)
		}
		field, ok := fieldByJSON(schema, "targetValue")
		if !ok {
			t.Fatalf("targetValue field not found on %s", owner)
		}
		prim, ok := field.Schema.Of.(*facts.Prim)
		if !ok || prim.Prim != facts.PrimFloat || prim.Bits == nil || *prim.Bits != 64 {
			t.Errorf("%s.TargetValue: want float64 fact, got %+v", owner, field.Schema)
		}
	}
	for _, d := range diags {
		if containsAll(d.Message, "float64", "float32") {
			t.Errorf("precision-preserving extraction emitted stale narrowing diagnostic: %+v", d)
		}
	}
	// Every diagnostic must carry a file:line.
	for _, d := range diags {
		if d.File == "" || d.Line == 0 {
			t.Errorf("diagnostic missing file:line: %+v", d)
		}
	}
}

func hasDiag(diags []facts.DiagnosticFact, ruleSubstr, identitySubstr string) bool {
	for _, d := range diags {
		if containsAll(d.Message, ruleSubstr, identitySubstr) {
			return true
		}
	}
	return false
}

func containsAll(s string, subs ...string) bool {
	for _, sub := range subs {
		if !contains(s, sub) {
			return false
		}
	}
	return true
}

func contains(s, sub string) bool {
	return len(sub) == 0 || (len(s) >= len(sub) && indexOf(s, sub) >= 0)
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

const namedConstraintDeclarations = `package bounds
import ("encoding/json"; "time")
type Tags []string
type Slots [3]int
type Labels map[string]string
type Name string
type Rank int
type Ratio float64
type Choice string
const ChoiceA Choice = "a"
type RankAlias = Rank
type TagsAlias = Tags
type RankPtr *Rank
type RankPtrAlias = RankPtr
type Flag bool
`

func extractNamedConstraintSource(t *testing.T, fields string) ([]facts.SchemaFact, []facts.DiagnosticFact) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/bounds\n\ngo 1.23\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	source := namedConstraintDeclarations + "type Payload struct {\n" + fields + "\nTime time.Time `json:\"time\"`\nRaw json.RawMessage `json:\"raw\"`\n}\n"
	if err := os.WriteFile(filepath.Join(dir, "models.go"), []byte(source), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := load.Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	d := diag.New()
	schemas := types.Extract(res, d)
	return schemas, d.Items()
}

func namedConstraintField(t *testing.T, schemas []facts.SchemaFact, name string) facts.FieldFact {
	t.Helper()
	payload, ok := schemaByName(schemas, "Payload")
	if !ok {
		t.Fatal("missing Payload")
	}
	f, ok := fieldByJSON(payload, name)
	if !ok {
		t.Fatalf("missing %s", name)
	}
	return f
}

func constraintJSON(f facts.FieldFact) string {
	if f.Meta == nil || f.Meta.Constraints == nil {
		return "{}"
	}
	b, _ := json.Marshal(f.Meta.Constraints)
	return string(b)
}

func TestNamedFieldConstraintBounds(t *testing.T) {
	shapes := []struct{ named, plain, component, kind string }{
		{"Tags", "[]string", "Tags", facts.TypeArray}, {"Slots", "[3]int", "Slots", facts.TypeArray},
		{"Labels", "map[string]string", "Labels", facts.TypeMap}, {"Name", "string", "Name", facts.TypePrimitive},
		{"Rank", "int", "Rank", facts.TypePrimitive}, {"Ratio", "float64", "Ratio", facts.TypePrimitive},
		{"Choice", "string", "Choice", facts.TypeEnum}, {"RankAlias", "int", "Rank", facts.TypePrimitive},
		{"*Rank", "*int", "Rank", facts.TypePrimitive}, {"***Rank", "***int", "Rank", facts.TypePrimitive},
		{"**RankAlias", "**int", "Rank", facts.TypePrimitive}, {"*TagsAlias", "*[]string", "Tags", facts.TypeArray},
	}
	var fields strings.Builder
	for i, sh := range shapes {
		for _, tag := range []string{"binding", "validate"} {
			for _, rule := range []string{"min=1,max=8", "gte=1,lte=8", "gt=0,lt=9"} {
				n := fmt.Sprintf("F%d_%s_%d", i, tag, fields.Len())
				fmt.Fprintf(&fields, "%s %s `json:\"%s\" %s:\"%s\"`\n%sControl %s `json:\"%sControl\" %s:\"%s\"`\n", n, sh.named, n, tag, rule, n, sh.plain, n, tag, rule)
			}
		}
	}
	schemas, diagnostics := extractNamedConstraintSource(t, fields.String())
	for _, d := range diagnostics {
		if d.Code == "schema.metadata.unresolved" {
			t.Errorf("supported constraint unresolved: %+v", d)
		}
	}
	payload, _ := schemaByName(schemas, "Payload")
	for _, f := range objectFields(payload) {
		if !strings.HasPrefix(f.JSONName, "F") || strings.HasSuffix(f.JSONName, "Control") {
			continue
		}
		var i int
		fmt.Sscanf(f.JSONName, "F%d_", &i)
		sh := shapes[i]
		control := namedConstraintField(t, schemas, f.JSONName+"Control")
		if constraintJSON(f) != constraintJSON(control) {
			t.Errorf("%s (%s): constraints %s want %s", f.JSONName, sh.named, constraintJSON(f), constraintJSON(control))
		}
		if f.Schema.Type != facts.TypeNamed {
			t.Errorf("%s lost named reference: %+v", f.JSONName, f.Schema)
		}
		component, ok := schemaByName(schemas, sh.component)
		if !ok || component.Body.Type != sh.kind || component.ID != f.Schema.Of {
			t.Errorf("%s reference/component mismatch: %+v / %+v", f.JSONName, f.Schema, component)
		}
	}
}

func TestNamedFieldConstraintInvalidSizesAreDiagnosed(t *testing.T) {
	var fields strings.Builder
	names := map[string]string{}
	for _, shape := range []string{"Tags", "Labels", "Name"} {
		for _, tag := range []string{"binding", "validate"} {
			for _, rule := range []string{"min=-1", "max=1.5", "gte=abc", "lt=0", "gt=18446744073709551615", "gt=2", "lt=9"} {
				n := fmt.Sprintf("F%d", len(names))
				names[n] = rule
				fmt.Fprintf(&fields, "%s %s `json:\"%s\" %s:\"%s\"`\n", n, shape, n, tag, rule)
			}
		}
	}
	schemas, diagnostics := extractNamedConstraintSource(t, fields.String())
	for n, rule := range names {
		f := namedConstraintField(t, schemas, n)
		c := constraintJSON(f)
		valid := rule == "gt=2" || rule == "lt=9"
		if valid {
			if c == "{}" || strings.Contains(c, "imum") {
				t.Errorf("%s valid size %s: %s", n, rule, c)
			}
			continue
		}
		if c != "{}" {
			t.Errorf("%s invalid size %s emitted %s", n, rule, c)
		}
		found := false
		for _, d := range diagnostics {
			if d.Code == "schema.metadata.unresolved" && d.Schema == "Payload" && d.Subject == n && strings.Contains(d.Message, rule) && d.Category == "schema" && d.Line > 0 && strings.HasSuffix(d.File, "models.go") {
				found = true
			}
		}
		if !found {
			t.Errorf("%s missing source-aware diagnostic for %s: %+v", n, rule, diagnostics)
		}
	}
}

func TestNamedFieldConstraintUnsupportedNamedKinds(t *testing.T) {
	var fields strings.Builder
	names := map[string]string{}
	for _, shape := range []string{"RankPtr", "RankPtrAlias", "*RankPtr", "**RankPtrAlias", "Flag"} {
		for _, tag := range []string{"binding", "validate"} {
			for _, rule := range []string{"min=1", "max=9", "gte=1", "lte=9", "gt=0", "lt=10"} {
				n := fmt.Sprintf("F%d", len(names))
				names[n] = rule
				fmt.Fprintf(&fields, "%s %s `json:\"%s\" %s:\"%s\"`\n", n, shape, n, tag, rule)
			}
		}
	}
	fields.WriteString("Control *Rank `json:\"control\" binding:\"min=1,max=9\"`\nAlias RankAlias `json:\"alias\" validate:\"gte=1,lte=9\"`\n")
	schemas, diagnostics := extractNamedConstraintSource(t, fields.String())
	if _, ok := schemaByName(schemas, "RankPtr"); ok {
		t.Error("defined pointer must not acquire a component")
	}
	for n, rule := range names {
		f := namedConstraintField(t, schemas, n)
		if f.Schema.Type != facts.TypeNamed || constraintJSON(f) != "{}" {
			t.Errorf("%s unsupported named shape: %+v / %s", n, f.Schema, constraintJSON(f))
		}
		found := false
		for _, d := range diagnostics {
			if d.Code == "schema.metadata.unresolved" && d.Category == "schema" && d.Schema == "Payload" && d.Subject == n && d.Line > 0 && strings.HasSuffix(d.File, "models.go") && strings.Contains(d.Message, rule) {
				found = true
			}
		}
		if !found {
			t.Errorf("%s missing diagnostic for %s", n, rule)
		}
	}
	for _, n := range []string{"control", "alias"} {
		if constraintJSON(namedConstraintField(t, schemas, n)) != `{"minimum":"1","maximum":"9"}` {
			t.Errorf("%s missing supported numeric bounds", n)
		}
	}
}

func TestNamedFieldConstraintScopesStayOnField(t *testing.T) {
	schemas, diagnostics := extractNamedConstraintSource(t, `
 Values *TagsAlias `+"`json:\"values\" validate:\"min=1,dive,min=2,max=8,unknown,gte=abc\"`"+`
 Keys Labels `+"`json:\"keys\" binding:\"min=1,dive,keys,min=2,endkeys,max=8\"`"+`
 Bytes []byte `+"`json:\"bytes\" validate:\"min=1,dive,max=8\"`"+`
 Free map[string]any `+"`json:\"free\"`"+`
 `)
	for n, want := range map[string]string{"values": `{"min_items":1}`, "keys": `{"min_properties":1}`, "bytes": `{"min_items":1}`, "free": "{}"} {
		if got := constraintJSON(namedConstraintField(t, schemas, n)); got != want {
			t.Errorf("%s constraints %s want %s", n, got, want)
		}
	}
	for _, token := range []string{"unknown", "gte=abc"} {
		found := false
		for _, d := range diagnostics {
			if d.Code == "schema.metadata.unresolved" && d.Category == "schema" && d.Subject == "Values" && d.Line > 0 && strings.HasSuffix(d.File, "models.go") && strings.Contains(d.Message, token) {
				found = true
			}
		}
		if !found {
			t.Errorf("missing source diagnostic %s", token)
		}
	}
	if len(diagnostics) != 3 {
		t.Errorf("expected two unresolved tokens plus free-form diagnostic, got %+v", diagnostics)
	}
}
