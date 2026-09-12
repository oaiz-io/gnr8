package tags

import "testing"

func TestScopedClassifiesTokensByScope(t *testing.T) {
	cases := []struct {
		name  string
		value string
		want  []Token
	}{
		{
			name:  "empty value has no tokens",
			value: "",
			want:  nil,
		},
		{
			name:  "a flat tag is all field scope",
			value: "required,min=3,oneof=a b",
			want: []Token{
				{Text: "required", Scope: ScopeField},
				{Text: "min=3", Scope: ScopeField},
				{Text: "oneof=a b", Scope: ScopeField},
			},
		},
		{
			name:  "dive moves the rest onto the elements",
			value: "required,dive,min=1,max=100",
			want: []Token{
				{Text: "required", Scope: ScopeField},
				{Text: "min=1", Scope: ScopeElement},
				{Text: "max=100", Scope: ScopeElement},
			},
		},
		{
			name:  "keys selects the map keys and endkeys returns to the values",
			value: "omitempty,dive,keys,required,endkeys,required",
			want: []Token{
				{Text: "omitempty", Scope: ScopeField},
				{Text: "required", Scope: ScopeMapKey},
				{Text: "required", Scope: ScopeElement},
			},
		},
		{
			name:  "nested dives never return to the field",
			value: "dive,dive,required",
			want: []Token{
				{Text: "required", Scope: ScopeElement, Nested: true},
			},
		},
		{
			name:  "a dive after endkeys descends again from the element",
			value: "dive,keys,min=1,endkeys,dive,required",
			want: []Token{
				{Text: "min=1", Scope: ScopeMapKey},
				{Text: "required", Scope: ScopeElement, Nested: true},
			},
		},
		{
			name:  "blank tokens and surrounding space are ignored",
			value: " required , , dive , required ",
			want: []Token{
				{Text: "required", Scope: ScopeField},
				{Text: "required", Scope: ScopeElement},
			},
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := Scoped(tc.value)
			if len(got) != len(tc.want) {
				t.Fatalf("token count: want %d %v, got %d %v", len(tc.want), tc.want, len(got), got)
			}
			for i := range tc.want {
				if got[i] != tc.want[i] {
					t.Errorf("token %d: want %+v, got %+v", i, tc.want[i], got[i])
				}
			}
		})
	}
}

func TestHasFieldTokenIgnoresInnerScopes(t *testing.T) {
	cases := []struct {
		value string
		want  bool
		why   string
	}{
		{"required", true, "a bare rule states a fact about the field"},
		{"omitempty,required", true, "order within the field's own scope does not matter"},
		{"required,dive,required", true, "the rule before dive still applies to the field"},
		{"omitempty,dive,required", false, "past dive the rule applies to each element"},
		{"omitempty,dive,keys,required,endkeys,required", false, "map keys and values, not the key's presence"},
		{"dive,dive,required", false, "nested dives descend further from the field"},
		{"required_without=Name", false, "a longer rule is a different rule"},
		{"notrequired", false, "substrings are not tokens"},
		{"", false, "an absent tag states nothing"},
	}

	for _, tc := range cases {
		if got := HasFieldToken(tc.value, "required"); got != tc.want {
			t.Errorf("HasFieldToken(%q, \"required\") = %v, want %v — %s", tc.value, got, tc.want, tc.why)
		}
	}
}
