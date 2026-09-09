package types

import (
	"reflect"
	"strings"
	"testing"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
)

func TestFieldMetaFromTagsParsesConstraintsDefaultsAndExtensions(t *testing.T) {
	tag := reflect.StructTag(`json:"name" binding:"required,min=3,max=80,oneof=alpha beta" default:"alpha" placeholder:"Goal name" render:"textarea" x-gnr8-widget:"compact"`)
	diags := diag.New()

	meta := fieldMetaFromTags(
		"CreateGoalInput",
		"Name",
		tag,
		string(tag),
		facts.PrimitiveType(facts.StringPrim()),
		"dto.go",
		10,
		diags,
	)

	if meta == nil || meta.Constraints == nil {
		t.Fatalf("expected metadata constraints, got %#v", meta)
	}
	if meta.Constraints.MinLength == nil || *meta.Constraints.MinLength != 3 {
		t.Fatalf("minLength not parsed: %#v", meta.Constraints.MinLength)
	}
	if meta.Constraints.MaxLength == nil || *meta.Constraints.MaxLength != 80 {
		t.Fatalf("maxLength not parsed: %#v", meta.Constraints.MaxLength)
	}
	if got := meta.Constraints.EnumValues; len(got) != 2 || got[0] != "alpha" || got[1] != "beta" {
		t.Fatalf("oneof values not parsed in source order: %#v", got)
	}
	if meta.Default == nil || meta.Default.Type != "string" || meta.Default.Value != "alpha" {
		t.Fatalf("default literal not parsed: %#v", meta.Default)
	}
	if len(meta.Extensions) != 3 {
		t.Fatalf("expected 3 extensions, got %#v", meta.Extensions)
	}
	if got := meta.Extensions[0].Name; got != "x-gnr8-placeholder" {
		t.Fatalf("extensions should be sorted by name, first=%q", got)
	}
	if len(diags.Items()) != 0 {
		t.Fatalf("unexpected diagnostics: %#v", diags.Items())
	}
}

func TestFieldMetaFromTagsParsesNumericBindingsAndUnsupportedDiagnostics(t *testing.T) {
	tag := reflect.StructTag(`json:"windowDays" binding:"gte=1,lte=365,uuid" default:"30"`)
	diags := diag.New()

	meta := fieldMetaFromTags(
		"GoalAnalyticsQuery",
		"WindowDays",
		tag,
		string(tag),
		facts.PrimitiveType(facts.IntPrim(64, true)),
		"dto.go",
		11,
		diags,
	)

	if meta == nil || meta.Constraints == nil {
		t.Fatalf("expected metadata constraints, got %#v", meta)
	}
	if meta.Constraints.Minimum == nil || *meta.Constraints.Minimum != "1" {
		t.Fatalf("minimum not parsed: %#v", meta.Constraints.Minimum)
	}
	if meta.Constraints.Maximum == nil || *meta.Constraints.Maximum != "365" {
		t.Fatalf("maximum not parsed: %#v", meta.Constraints.Maximum)
	}
	if meta.Default == nil || meta.Default.Type != "number" || meta.Default.Value != "30" {
		t.Fatalf("numeric default not parsed: %#v", meta.Default)
	}
	if !hasMetadataDiag(diags.Items(), "unsupported binding tag", "uuid") {
		t.Fatalf("expected unsupported binding diagnostic, got %#v", diags.Items())
	}
}

func TestFieldMetaFromTagsLowersCollectionCardinalityWithoutChangingScalarRules(t *testing.T) {
	tests := []struct {
		name     string
		tag      string
		schema   facts.Type
		minItems *uint64
		maxItems *uint64
	}{
		{
			name:     "binding minimum before dive",
			tag:      `json:"names" binding:"required,min=1,dive"`,
			schema:   facts.ArrayType(facts.PrimitiveType(facts.StringPrim())),
			minItems: uint64Ptr(1),
		},
		{
			name:     "validate minimum without dive",
			tag:      `json:"codes" validate:"required,min=1"`,
			schema:   facts.ArrayType(facts.PrimitiveType(facts.StringPrim())),
			minItems: uint64Ptr(1),
		},
		{
			name:     "binding maximum before dive",
			tag:      `json:"slots" binding:"required,max=100,dive"`,
			schema:   facts.ArrayType(facts.PrimitiveType(facts.IntPrim(64, true))),
			maxItems: uint64Ptr(100),
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			tag := reflect.StructTag(tc.tag)
			diags := diag.New()
			meta := fieldMetaFromTags(
				"CollectionRules",
				"Values",
				tag,
				string(tag),
				tc.schema,
				"dto.go",
				12,
				diags,
			)

			if meta == nil || meta.Constraints == nil {
				t.Fatalf("expected collection constraints, got %#v", meta)
			}
			if !equalUint64Ptr(meta.Constraints.MinItems, tc.minItems) ||
				!equalUint64Ptr(meta.Constraints.MaxItems, tc.maxItems) {
				t.Fatalf("collection bounds mismatch: %#v", meta.Constraints)
			}
			if meta.Constraints.MinLength != nil || meta.Constraints.MaxLength != nil ||
				meta.Constraints.Minimum != nil || meta.Constraints.Maximum != nil {
				t.Fatalf("collection bounds must not become string or numeric bounds: %#v", meta.Constraints)
			}
			if len(diags.Items()) != 0 {
				t.Fatalf("supported collection bounds must not be unresolved: %#v", diags.Items())
			}
		})
	}

	dived := reflect.StructTag(`json:"values" validate:"min=1,dive,min=2,max=8"`)
	divedDiags := diag.New()
	divedMeta := fieldMetaFromTags(
		"CollectionRules",
		"Values",
		dived,
		string(dived),
		facts.ArrayType(facts.PrimitiveType(facts.StringPrim())),
		"dto.go",
		13,
		divedDiags,
	)
	if divedMeta == nil || divedMeta.Constraints == nil ||
		divedMeta.Constraints.MinItems == nil || *divedMeta.Constraints.MinItems != 1 ||
		divedMeta.Constraints.MaxItems != nil {
		t.Fatalf("post-dive bounds must not overwrite collection cardinality: %#v", divedMeta)
	}
	if len(divedDiags.Items()) != 0 {
		t.Fatalf("post-dive item rules remain understood: %#v", divedDiags.Items())
	}

	unknown := reflect.StructTag(`json:"values" validate:"min=1,dive,email"`)
	unknownDiags := diag.New()
	_ = fieldMetaFromTags(
		"CollectionRules",
		"Values",
		unknown,
		string(unknown),
		facts.ArrayType(facts.PrimitiveType(facts.StringPrim())),
		"dto.go",
		14,
		unknownDiags,
	)
	if !hasMetadataDiag(unknownDiags.Items(), "unsupported validate tag", "email") {
		t.Fatalf("unrelated unsupported item rule must remain diagnosed: %#v", unknownDiags.Items())
	}
}

func TestFieldMetaFromTagsScopesConstraintsToTheFieldItself(t *testing.T) {
	// `min=3` bounds the field; everything past `dive` bounds each element. Before
	// scope-awareness the trailing pair overwrote the field's own bound.
	tag := reflect.StructTag(`json:"code" validate:"required,min=3,dive,min=1,max=100"`)
	diags := diag.New()

	meta := fieldMetaFromTags(
		"ScopedRules",
		"Code",
		tag,
		string(tag),
		facts.PrimitiveType(facts.StringPrim()),
		"dto.go",
		12,
		diags,
	)

	if meta == nil || meta.Constraints == nil {
		t.Fatalf("expected the pre-dive constraint to survive, got %#v", meta)
	}
	if meta.Constraints.MinLength == nil || *meta.Constraints.MinLength != 3 {
		t.Fatalf("pre-dive min must bind the field: %#v", meta.Constraints.MinLength)
	}
	if meta.Constraints.MaxLength != nil {
		t.Fatalf("post-dive max must not bind the field: %#v", *meta.Constraints.MaxLength)
	}
	if len(diags.Items()) != 0 {
		t.Fatalf("element-scope tokens are understood, not unresolved: %#v", diags.Items())
	}
}

func TestFieldMetaFromTagsIgnoresMapKeyScopedTokens(t *testing.T) {
	// `keys`/`endkeys` are structural markers, not constraint tokens: parsing them as
	// constraints reported each of them as an unsupported rule, so this one tag raised
	// two bogus `schema.metadata.unresolved` diagnostics.
	tag := reflect.StructTag(`json:"headers,omitzero" binding:"omitempty,dive,keys,required,endkeys,required"`)
	diags := diag.New()

	meta := fieldMetaFromTags(
		"ScopedRules",
		"Headers",
		tag,
		string(tag),
		facts.MapTypeOf(facts.PrimitiveType(facts.StringPrim()), facts.PrimitiveType(facts.StringPrim())),
		"dto.go",
		13,
		diags,
	)

	if meta != nil && meta.Constraints != nil {
		t.Fatalf("nothing in the tag constrains the map itself, got %#v", meta.Constraints)
	}
	if len(diags.Items()) != 0 {
		t.Fatalf("scope markers must not be reported as unsupported: %#v", diags.Items())
	}
}

func TestFieldMetaFromTagsReportsUnknownRulesAtEveryScope(t *testing.T) {
	// Scope decides where a rule applies; recognition decides whether gnr8 can say
	// anything about it. `email` is a rule gnr8 does not lower, so it is reported
	// wherever it is written — silence must not depend on the author's position.
	cases := []struct {
		name string
		tag  string
	}{
		{"field scope", `json:"to" validate:"required,email"`},
		{"element scope", `json:"to" validate:"omitempty,dive,email"`},
		{"map key scope", `json:"to" validate:"omitempty,dive,keys,email,endkeys"`},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			tag := reflect.StructTag(tc.tag)
			diags := diag.New()

			fieldMetaFromTags(
				"SendEmailProperties",
				"To",
				tag,
				string(tag),
				facts.ArrayType(facts.PrimitiveType(facts.StringPrim())),
				"schema.go",
				10,
				diags,
			)

			if !hasMetadataDiag(diags.Items(), "unsupported validate tag", "email") {
				t.Fatalf("an unknown rule must be reported at this scope, got %#v", diags.Items())
			}
		})
	}
}

func TestKnownRulesParseIdenticallyAtEveryScope(t *testing.T) {
	// One switch decides both whether gnr8 knows a rule and whether it can apply it,
	// so a rule with a case is silent at every scope and a rule without one is
	// reported at every scope — by construction, not by keeping two lists in step.
	// These cases document that; they cannot enumerate the switch, and a list that
	// looked like it could would be worse than none.
	rules := []string{"required", "omitempty", "min=1", "max=100", "gte=1", "lte=9", "gt=0", "lt=9", "oneof=a b"}

	for _, rule := range rules {
		for _, tag := range []string{rule, "dive," + rule, "dive,keys," + rule + ",endkeys"} {
			diags := diag.New()
			constraintsFromTag(
				"validate",
				"ScopedRules",
				"Field",
				tag,
				facts.PrimitiveType(facts.IntPrim(64, true)),
				"dto.go",
				14,
				diags,
			)
			if len(diags.Items()) != 0 {
				t.Errorf("%q must parse without a diagnostic: %#v", tag, diags.Items())
			}
		}
	}
}

func TestUnreadableRuleValuesAreReportedAtEveryScope(t *testing.T) {
	// The silence past a `dive` is for a rule gnr8 read and cannot carry, not for one
	// it could not read. A known rule whose value is missing or malformed is broken
	// source at any scope, and `dive` is not a place for it to hide.
	//
	// `min`/`max` are judged loosely off the field's scope, because whether the value
	// is a length or a bound depends on the element's type and this parser only holds
	// the field's. Every value either spelling accepts is still a number, which is
	// enough to catch one no element type could accept.
	rules := []string{"min", "min=abc", "max=abc", "gte", "gte=abc", "lt=x", "oneof", "oneof="}

	for _, rule := range rules {
		for _, tag := range []string{rule, "dive," + rule, "dive,keys," + rule + ",endkeys"} {
			diags := diag.New()
			constraintsFromTag(
				"validate",
				"ScopedRules",
				"Field",
				tag,
				facts.PrimitiveType(facts.IntPrim(64, true)),
				"dto.go",
				15,
				diags,
			)
			if len(diags.Items()) != 1 {
				t.Errorf("%q must be reported exactly once, got %#v", tag, diags.Items())
			}
		}
	}
}

func TestBindingHasRequiredRequiresExactToken(t *testing.T) {
	if !bindingHasRequired("omitempty,required") {
		t.Fatal("expected exact required token to mark field required")
	}
	if bindingHasRequired("required_without=Name") {
		t.Fatal("required_without must not mark the field strictly required")
	}
	if bindingHasRequired("notrequired") {
		t.Fatal("substring matches must not mark the field required")
	}
	if !bindingHasRequired("required,dive,required") {
		t.Fatal("the required before dive still requires the field")
	}
	if bindingHasRequired("omitempty,dive,required") {
		t.Fatal("required after dive constrains the elements, not the key")
	}
	if bindingHasRequired("omitempty,dive,keys,required,endkeys,required") {
		t.Fatal("required inside keys/endkeys constrains the map's keys and values")
	}
	if bindingHasRequired("dive,dive,required") {
		t.Fatal("nested dives descend further from the field, never back to it")
	}
}

func TestValidateHasRequiredRequiresExactToken(t *testing.T) {
	if !validateHasRequired("required,email") {
		t.Fatal("expected validate required token to mark field required")
	}
	if validateHasRequired("required_without=Name") {
		t.Fatal("required_without must not mark the field strictly required")
	}
	if validateHasRequired("notrequired") {
		t.Fatal("substring matches must not mark the field required")
	}
}

func hasMetadataDiag(diags []facts.DiagnosticFact, rule string, token string) bool {
	for _, d := range diags {
		if strings.Contains(d.Message, rule) && strings.Contains(d.Message, token) {
			return true
		}
	}
	return false
}

func uint64Ptr(value uint64) *uint64 {
	return &value
}

func equalUint64Ptr(left, right *uint64) bool {
	return left == nil && right == nil || left != nil && right != nil && *left == *right
}
