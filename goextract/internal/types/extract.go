// Package types walks the reachable named types of the target module and lowers
// each DTO struct / enum to a router-agnostic facts.SchemaFact (GO-02, GO-03).
//
// Scope discipline (02-01): only named types DECLARED IN THE TARGET MODULE are
// considered, and a struct is treated as a DTO schema only when it (or an
// embedded struct) carries at least one payload tag (`json:` or `form:`). This
// excludes server/wiring structs such as HttpServer while capturing JSON DTOs and
// multipart/form DTOs.
// Routes/handlers are 02-02; this package does not look at them.
package types

import (
	"go/token"
	gotypes "go/types"
	"math"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/tags"
)

// well-known package paths for type mapping (RESEARCH Pattern 6).
const (
	uuidPkgPath      = "github.com/google/uuid"
	timePkgPath      = "time"
	jsonPkgPath      = "encoding/json"
	multipartPkgPath = "mime/multipart"
)

// Extract returns one SchemaFact per DTO struct and per string-enum named type
// declared in the target module, plus any float64 / free-form-map diagnostics it
// discovers. Output order is not guaranteed here; facts.Marshal sorts everything.
func Extract(res *load.Result, diags *diag.Accumulator) []facts.SchemaFact {
	modulePath := mainModulePath(res)
	schemas := make([]facts.SchemaFact, 0)

	for _, pkg := range res.Packages {
		if !isTargetPackage(pkg.PkgPath, modulePath) || pkg.Types == nil {
			continue
		}
		scope := pkg.Types.Scope()
		for _, name := range scope.Names() {
			obj := scope.Lookup(name)
			tn, ok := obj.(*gotypes.TypeName)
			if !ok || tn.IsAlias() {
				continue
			}
			named, ok := gotypes.Unalias(tn.Type()).(*gotypes.Named)
			if !ok {
				continue
			}
			if fact, ok := schemaFor(named, modulePath, res.Fset, scope, diags); ok {
				schemas = append(schemas, fact)
			}
		}
	}
	return schemas
}

// schemaFor lowers one named type to a SchemaFact, or reports ok=false if the
// type is neither a DTO struct nor a string enum.
func schemaFor(
	named *gotypes.Named,
	modulePath string,
	fset *token.FileSet,
	scope *gotypes.Scope,
	diags *diag.Accumulator,
) (facts.SchemaFact, bool) {
	switch under := named.Underlying().(type) {
	case *gotypes.Struct:
		if under.NumFields() > 0 && !structHasPayloadTag(under) {
			return facts.SchemaFact{}, false
		}
		span := spanOf(fset, named.Obj().Pos())
		fields := extractFields(named.Obj().Name(), under, modulePath, fset, diags)
		return facts.SchemaFact{
			ID:   schemaID(named, modulePath),
			Name: named.Obj().Name(),
			Body: facts.ObjectType(fields),
			Span: span,
		}, true
	case *gotypes.Basic:
		body := mapType(under, namedSchemaCtx(named, modulePath, fset, diags))
		if under.Kind() == gotypes.String {
			if values := enumValues(named, scope); len(values) > 0 {
				body = facts.EnumType(values)
			}
		}
		return facts.SchemaFact{
			ID:   schemaID(named, modulePath),
			Name: named.Obj().Name(),
			Body: body,
			Span: spanOf(fset, named.Obj().Pos()),
		}, true
	case *gotypes.Slice, *gotypes.Array, *gotypes.Map:
		return facts.SchemaFact{
			ID:   schemaID(named, modulePath),
			Name: named.Obj().Name(),
			Body: mapType(under, namedSchemaCtx(named, modulePath, fset, diags)),
			Span: spanOf(fset, named.Obj().Pos()),
		}, true
	default:
		return facts.SchemaFact{}, false
	}
}

func namedSchemaCtx(
	named *gotypes.Named,
	modulePath string,
	fset *token.FileSet,
	diags *diag.Accumulator,
) mapCtx {
	file, line := positionOf(fset, named.Obj().Pos())
	return mapCtx{
		structName:   named.Obj().Name(),
		fieldName:    named.Obj().Name(),
		declaredType: typeString(named),
		modulePath:   modulePath,
		file:         file,
		line:         line,
		diags:        diags,
	}
}

// extractFields walks struct fields, flattening embedded structs (Pattern 5).
func extractFields(
	structName string,
	st *gotypes.Struct,
	modulePath string,
	fset *token.FileSet,
	diags *diag.Accumulator,
) []facts.FieldFact {
	fields := make([]facts.FieldFact, 0, st.NumFields())
	for i := 0; i < st.NumFields(); i++ {
		f := st.Field(i)
		tag := reflect.StructTag(st.Tag(i))

		if f.Embedded() {
			if embedded, ok := embeddedStruct(f.Type()); ok {
				// Promote the embedded struct's fields, but attribute diagnostics
				// to the embedded type's own name (its float64 fields belong to it).
				fields = append(fields, extractFields(
					embeddedTypeName(f.Type()), embedded, modulePath, fset, diags)...)
			}
			continue
		}

		jsonName, wire, omitOpt, skip := parsePayloadTag(tag, f.Name())
		if skip {
			continue
		}

		file, line := positionOf(fset, f.Pos())
		ctx := mapCtx{
			structName:   structName,
			fieldName:    f.Name(),
			declaredType: typeString(f.Type()), // the AS-WRITTEN type, e.g. "*float64"
			modulePath:   modulePath,
			file:         file,
			line:         line,
			diags:        diags,
		}
		schema := mapType(f.Type(), ctx)
		validatorRequiresPresence := bindingHasRequired(tag.Get("binding")) || validateHasRequired(tag.Get("validate"))
		serializerMayOmit, deserializerAcceptsNull := presenceAndNullability(wire, omitOpt, f.Type())
		serializerMayEmitNull := outputMayEmitNull(wire, omitOpt, f.Type())
		validatorRejectsNull := validatorRequiresPresence && validationRejectsNull(f.Type())
		if wire == wireJSON && omitOpt == optOmitEmpty && !serializerMayOmit {
			diags.IneffectiveOmitEmpty(structName, f.Name(), typeString(f.Type()), file, line)
		}
		meta := fieldMetaFromTags(structName, f.Name(), tag, st.Tag(i), schema, file, line, diags)
		description := optString(tag.Get("description"))
		if description == nil {
			description = optString(schemaTagValue(tag.Get("schema"), "description"))
		}
		example := optString(tag.Get("example"))
		if example == nil {
			example = optString(schemaTagValue(tag.Get("schema"), "example"))
		}

		fields = append(fields, facts.FieldFact{
			JSONName:                  jsonName,
			SerializerMayOmit:         serializerMayOmit,
			DeserializerAcceptsAbsent: true,
			DeserializerAcceptsNull:   deserializerAcceptsNull,
			SerializerMayEmitNull:     serializerMayEmitNull,
			ValidatorRequiresPresence: validatorRequiresPresence,
			ValidatorRejectsNull:      validatorRejectsNull,
			Schema:                    schema,
			Description:               description,
			Example:                   example,
			Meta:                      meta,
		})
	}
	return fields
}

func bindingHasRequired(binding string) bool {
	return tagHasRequired(binding)
}

func validateHasRequired(validate string) bool {
	return tagHasRequired(validate)
}

// tagHasRequired reports whether a validation tag requires the field itself.
// Only field-scope tokens count: a `required` reached through `dive` or `keys`
// describes what lives inside the field, not whether its key is present.
func tagHasRequired(value string) bool {
	return tags.HasFieldToken(value, "required")
}

func fieldMetaFromTags(
	structName string,
	fieldName string,
	tag reflect.StructTag,
	rawTag string,
	schema facts.Type,
	file string,
	line uint32,
	diags *diag.Accumulator,
) *facts.FieldMeta {
	meta := &facts.FieldMeta{}

	constraints := constraintsFromBinding(structName, fieldName, tag.Get("binding"), schema, file, line, diags)
	mergeConstraints(constraints, constraintsFromValidate(structName, fieldName, tag.Get("validate"), schema, file, line, diags))
	applyDirectConstraints(constraints, tag, schema, structName, fieldName, file, line, diags)
	if !constraintsEmpty(constraints) {
		meta.Constraints = constraints
	}

	if rawDefault, ok := tag.Lookup("default"); ok {
		meta.Default = literalForSchema(rawDefault, schema, structName, fieldName, file, line, diags)
	}
	if meta.Default == nil {
		if rawDefault := schemaTagValue(tag.Get("schema"), "default"); rawDefault != "" {
			meta.Default = literalForSchema(rawDefault, schema, structName, fieldName, file, line, diags)
		}
	}
	if format, ok := tag.Lookup("format"); ok && format != "" {
		meta.Format = stringPtr(format)
	}
	if meta.Format == nil {
		if format := schemaTagValue(tag.Get("schema"), "format"); format != "" {
			meta.Format = stringPtr(format)
		}
	}

	extensions := make([]facts.Extension, 0)
	if placeholder, ok := tag.Lookup("placeholder"); ok {
		extensions = append(extensions, facts.Extension{
			Name:  "x-gnr8-placeholder",
			Value: stringLiteral(placeholder),
		})
	}
	if render, ok := tag.Lookup("render"); ok {
		extensions = append(extensions, facts.Extension{
			Name:  "x-gnr8-render",
			Value: stringLiteral(render),
		})
	}
	if rawExtensions, ok := tag.Lookup("extensions"); ok {
		extensions = append(extensions, parseExtensions(rawExtensions)...)
	}

	rawTags := parseStructTag(rawTag)
	keys := make([]string, 0, len(rawTags))
	for key := range rawTags {
		if strings.HasPrefix(key, "x-") {
			keys = append(keys, key)
		}
	}
	sort.Strings(keys)
	for _, key := range keys {
		extensions = append(extensions, facts.Extension{
			Name:  key,
			Value: inferLiteral(rawTags[key]),
		})
	}
	if len(extensions) > 0 {
		meta.Extensions = extensions
	}

	if meta.Constraints == nil && meta.Default == nil && meta.Format == nil && len(meta.Extensions) == 0 {
		return nil
	}
	return meta
}

func constraintsFromBinding(
	structName string,
	fieldName string,
	binding string,
	schema facts.Type,
	file string,
	line uint32,
	diags *diag.Accumulator,
) *facts.Constraints {
	return constraintsFromTag("binding", structName, fieldName, binding, schema, file, line, diags)
}

func constraintsFromValidate(
	structName string,
	fieldName string,
	validate string,
	schema facts.Type,
	file string,
	line uint32,
	diags *diag.Accumulator,
) *facts.Constraints {
	return constraintsFromTag("validate", structName, fieldName, validate, schema, file, line, diags)
}

func constraintsFromTag(
	tagKind string,
	structName string,
	fieldName string,
	value string,
	schema facts.Type,
	file string,
	line uint32,
	diags *diag.Accumulator,
) *facts.Constraints {
	constraints := &facts.Constraints{}
	if value == "" {
		return constraints
	}
	for _, tok := range tags.Scoped(value) {
		token := tok.Text
		if token == "required" || token == "omitempty" {
			// Presence, not shape: owned by the required and optional axes, at every scope.
			continue
		}
		name, value, hasValue := strings.Cut(token, "=")
		name = strings.TrimSpace(name)
		value = strings.TrimSpace(value)

		// This switch answers both questions a rule raises, which is what keeps the
		// answers consistent. Reaching a case means gnr8 knows the rule; reaching
		// `default` means it does not, at any scope. Scope then decides only whether the
		// rule can be applied — a rule about the elements or map keys inside the field
		// has nothing to bind, because the neutral graph carries constraints on the
		// field itself (`FieldMeta.Constraints`) and there is no element schema to hang
		// them on.
		//
		// So a rule gnr8 knows is dropped in silence past a `dive`: the source is
		// well-formed and understood, and `unresolved` means gnr8 could not read the
		// source, not that it chose not to carry it. A rule gnr8 has never heard of is
		// reported wherever it appears — it cannot tell what `dive,somevalidator` was
		// meant to say or whether losing it matters, and silence should not depend on
		// where the author happened to write it.
		//
		// A rule's value obeys the same split. Silence is for the rule gnr8 read and
		// cannot carry, never for one it could not read, so each case judges the value
		// before it consults scope: `dive,gte=abc` is as unreadable as `gte=abc` and is
		// reported the same way. Only the part of the judgement that needs the element's
		// type can stop at a `dive`, because this function is handed the field's schema
		// and there is nothing below it to ask.
		fieldScope := tok.Scope == tags.ScopeField
		switch name {
		case "min", "max":
			if !fieldScope {
				// Whether `min=1` is a length or a bound depends on the element's type,
				// which is out of reach. What does not depend on it: every value either
				// spelling accepts is a number, a length being the unsigned case of one.
				// So a value no element type could accept is still reportable.
				if !hasValue || !validNumber(value) {
					unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				}
				continue
			}
			if !hasValue || !applyMinMaxConstraint(constraints, name, value, schema) {
				unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
			}
		case "gte", "lte", "gt", "lt":
			if !hasValue || !validNumber(value) {
				unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				continue
			}
			if !fieldScope {
				continue
			}
			// On a collection these spell the rule `min`/`max` spell — how many
			// elements or keys the field may hold — so they must publish the same
			// keyword. A numeric bound on an array or object states nothing any
			// validator reads.
			if kind := collectionKindOf(schema); kind != collectionKindNone {
				if !applyCollectionBound(constraints, name, value, kind) {
					unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				}
				continue
			}
			if schemaIsStringLike(schema) {
				if !applyStringLengthBound(constraints, name, value) {
					unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				}
				continue
			}
			bound := stringPtr(value)
			switch name {
			case "gte":
				constraints.Minimum = bound
			case "lte":
				constraints.Maximum = bound
			case "gt":
				constraints.ExclusiveMinimum = bound
			case "lt":
				constraints.ExclusiveMaximum = bound
			}
		case "oneof":
			if !hasValue {
				unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				continue
			}
			values := strings.Fields(value)
			if len(values) == 0 {
				unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
				continue
			}
			if !fieldScope {
				continue
			}
			constraints.EnumValues = values
		default:
			unsupportedConstraintTag(diags, tagKind, structName, fieldName, token, file, line)
		}
	}
	return constraints
}

// collectionKind reports whether a field's own type makes a size rule a rule
// about members, and which keyword states it. A slice or array is counted in
// elements, a map in keys; anything else is not a collection.
type collectionKind uint8

const (
	collectionKindNone collectionKind = iota
	collectionKindItems
	collectionKindProperties
)

func collectionKindOf(schema facts.Type) collectionKind {
	switch schema.Type {
	case facts.TypeArray:
		return collectionKindItems
	case facts.TypeMap:
		return collectionKindProperties
	}
	return collectionKindNone
}

// applyCollectionBound carries one size rule onto a collection field. The
// validator reads `min`/`gte`, `max`/`lte`, `gt` and `lt` all as bounds on
// len(), so on a collection they state one fact and must publish one keyword
// pair rather than the numeric bounds a scalar would take. A strict bound is
// exactly an inclusive one here because a length is a whole number: `gt=0` is
// `minItems: 1`.
func applyCollectionBound(c *facts.Constraints, name string, value string, kind collectionKind) bool {
	parsed, ok := discreteSizeBound(name, value)
	if !ok {
		return false
	}
	lower := name == "min" || name == "gte" || name == "gt"
	switch {
	case kind == collectionKindItems && lower:
		c.MinItems = &parsed
	case kind == collectionKindItems:
		c.MaxItems = &parsed
	case lower:
		c.MinProperties = &parsed
	default:
		c.MaxProperties = &parsed
	}
	return true
}

func applyStringLengthBound(c *facts.Constraints, name string, value string) bool {
	parsed, ok := discreteSizeBound(name, value)
	if !ok {
		return false
	}
	if name == "gte" || name == "gt" {
		c.MinLength = &parsed
	} else {
		c.MaxLength = &parsed
	}
	return true
}

func discreteSizeBound(name string, value string) (uint64, bool) {
	parsed, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return 0, false
	}
	switch name {
	case "gt":
		if parsed == math.MaxUint64 {
			return 0, false
		}
		parsed++
	case "lt":
		if parsed == 0 {
			// `lt=0` demands a negative size. Nothing satisfies it and no
			// keyword states it, so the rule is reported rather than invented.
			return 0, false
		}
		parsed--
	}
	return parsed, true
}

func applyMinMaxConstraint(c *facts.Constraints, name string, value string, schema facts.Type) bool {
	if kind := collectionKindOf(schema); kind != collectionKindNone {
		return applyCollectionBound(c, name, value, kind)
	}
	if schemaIsStringLike(schema) {
		parsed, err := strconv.ParseUint(value, 10, 64)
		if err != nil {
			return false
		}
		if name == "min" {
			c.MinLength = &parsed
		} else {
			c.MaxLength = &parsed
		}
		return true
	}
	if schemaIsNumeric(schema) {
		if !validNumber(value) {
			return false
		}
		if name == "min" {
			c.Minimum = stringPtr(value)
		} else {
			c.Maximum = stringPtr(value)
		}
		return true
	}
	return false
}

func mergeConstraints(dst, src *facts.Constraints) {
	if dst == nil || src == nil {
		return
	}
	if src.MinLength != nil {
		dst.MinLength = src.MinLength
	}
	if src.MaxLength != nil {
		dst.MaxLength = src.MaxLength
	}
	if src.MinItems != nil {
		dst.MinItems = src.MinItems
	}
	if src.MaxItems != nil {
		dst.MaxItems = src.MaxItems
	}
	if src.MinProperties != nil {
		dst.MinProperties = src.MinProperties
	}
	if src.MaxProperties != nil {
		dst.MaxProperties = src.MaxProperties
	}
	if src.Minimum != nil {
		dst.Minimum = src.Minimum
	}
	if src.Maximum != nil {
		dst.Maximum = src.Maximum
	}
	if src.ExclusiveMinimum != nil {
		dst.ExclusiveMinimum = src.ExclusiveMinimum
	}
	if src.ExclusiveMaximum != nil {
		dst.ExclusiveMaximum = src.ExclusiveMaximum
	}
	if src.Pattern != nil {
		dst.Pattern = src.Pattern
	}
	if len(src.EnumValues) > 0 {
		dst.EnumValues = src.EnumValues
	}
}

func applyDirectConstraints(c *facts.Constraints, tag reflect.StructTag, schema facts.Type, structName, fieldName, file string, line uint32, diags *diag.Accumulator) {
	if c == nil {
		return
	}
	if minLength := firstTagValue(tag, "minLength", "minlength"); minLength != "" {
		if parsed, err := strconv.ParseUint(minLength, 10, 64); err == nil {
			c.MinLength = &parsed
		} else {
			invalidSchemaTag(diags, structName, fieldName, "minLength", minLength, file, line)
		}
	}
	if maxLength := firstTagValue(tag, "maxLength", "maxlength"); maxLength != "" {
		if parsed, err := strconv.ParseUint(maxLength, 10, 64); err == nil {
			c.MaxLength = &parsed
		} else {
			invalidSchemaTag(diags, structName, fieldName, "maxLength", maxLength, file, line)
		}
	}
	if minimum := firstTagValue(tag, "minimum"); minimum != "" {
		if validNumber(minimum) || !schemaIsNumeric(schema) {
			c.Minimum = stringPtr(minimum)
		} else {
			invalidSchemaTag(diags, structName, fieldName, "minimum", minimum, file, line)
		}
	}
	if maximum := firstTagValue(tag, "maximum"); maximum != "" {
		if validNumber(maximum) || !schemaIsNumeric(schema) {
			c.Maximum = stringPtr(maximum)
		} else {
			invalidSchemaTag(diags, structName, fieldName, "maximum", maximum, file, line)
		}
	}
	if pattern := firstTagValue(tag, "pattern"); pattern != "" {
		c.Pattern = stringPtr(pattern)
	}
	if enumValues := firstTagValue(tag, "enums", "enum"); enumValues != "" {
		c.EnumValues = splitEnumValues(enumValues)
	}
}

func constraintsEmpty(c *facts.Constraints) bool {
	return c == nil ||
		(c.MinLength == nil &&
			c.MaxLength == nil &&
			c.MinItems == nil &&
			c.MaxItems == nil &&
			c.MinProperties == nil &&
			c.MaxProperties == nil &&
			c.Minimum == nil &&
			c.Maximum == nil &&
			c.ExclusiveMinimum == nil &&
			c.ExclusiveMaximum == nil &&
			c.Pattern == nil &&
			len(c.EnumValues) == 0)
}

func unsupportedConstraintTag(diags *diag.Accumulator, tagKind, structName, fieldName, token, file string, line uint32) {
	diags.SchemaMetadataUnresolved(
		structName,
		fieldName,
		"unsupported "+tagKind+" tag on "+structName+"."+fieldName+": "+strconv.Quote(token)+
			" ignored by gnr8 metadata extraction (GO-06)",
		file,
		line,
	)
}

func literalForSchema(
	value string,
	schema facts.Type,
	structName string,
	fieldName string,
	file string,
	line uint32,
	diags *diag.Accumulator,
) *facts.LiteralValue {
	if value == "null" {
		lit := nullLiteral()
		return &lit
	}
	if schemaIsBool(schema) {
		parsed, err := strconv.ParseBool(value)
		if err != nil {
			diags.SchemaMetadataUnresolved(
				structName,
				fieldName,
				"default tag on "+structName+"."+fieldName+" is not a valid bool: "+strconv.Quote(value),
				file,
				line,
			)
			lit := stringLiteral(value)
			return &lit
		}
		lit := boolLiteral(parsed)
		return &lit
	}
	if schemaIsNumeric(schema) {
		if !validNumber(value) {
			diags.SchemaMetadataUnresolved(
				structName,
				fieldName,
				"default tag on "+structName+"."+fieldName+" is not a valid number: "+strconv.Quote(value),
				file,
				line,
			)
			lit := stringLiteral(value)
			return &lit
		}
		lit := numberLiteral(value)
		return &lit
	}
	lit := stringLiteral(value)
	return &lit
}

func inferLiteral(value string) facts.LiteralValue {
	switch value {
	case "null":
		return nullLiteral()
	case "true":
		return boolLiteral(true)
	case "false":
		return boolLiteral(false)
	default:
		if validNumber(value) {
			return numberLiteral(value)
		}
		return stringLiteral(value)
	}
}

func stringLiteral(value string) facts.LiteralValue {
	return facts.LiteralValue{Type: "string", Value: value}
}

func numberLiteral(value string) facts.LiteralValue {
	return facts.LiteralValue{Type: "number", Value: value}
}

func boolLiteral(value bool) facts.LiteralValue {
	return facts.LiteralValue{Type: "bool", Value: value}
}

func nullLiteral() facts.LiteralValue {
	return facts.LiteralValue{Type: "null"}
}

func schemaIsStringLike(schema facts.Type) bool {
	if schema.Type == facts.TypeWellKnown {
		return true
	}
	if schema.Type != facts.TypePrimitive {
		return false
	}
	if prim, ok := schema.Of.(*facts.Prim); ok && prim != nil {
		return prim.Prim == facts.PrimString || prim.Prim == facts.PrimBytes
	}
	return false
}

func schemaIsNumeric(schema facts.Type) bool {
	if schema.Type != facts.TypePrimitive {
		return false
	}
	if prim, ok := schema.Of.(*facts.Prim); ok && prim != nil {
		return prim.Prim == facts.PrimInt || prim.Prim == facts.PrimFloat
	}
	return false
}

func schemaIsBool(schema facts.Type) bool {
	if schema.Type != facts.TypePrimitive {
		return false
	}
	if prim, ok := schema.Of.(*facts.Prim); ok && prim != nil {
		return prim.Prim == facts.PrimBool
	}
	return false
}

func validNumber(value string) bool {
	_, err := strconv.ParseFloat(value, 64)
	return err == nil
}

func stringPtr(value string) *string {
	return &value
}

func firstTagValue(tag reflect.StructTag, keys ...string) string {
	for _, key := range keys {
		if value, ok := tag.Lookup(key); ok && value != "" {
			return value
		}
		if value := schemaTagValue(tag.Get("schema"), key); value != "" {
			return value
		}
	}
	return ""
}

func schemaTagValue(raw string, key string) string {
	if raw == "" {
		return ""
	}
	for _, token := range strings.Split(raw, ",") {
		name, value, ok := strings.Cut(strings.TrimSpace(token), "=")
		if !ok {
			continue
		}
		if strings.EqualFold(strings.TrimSpace(name), key) {
			return strings.TrimSpace(value)
		}
	}
	return ""
}

func splitEnumValues(value string) []string {
	fields := strings.FieldsFunc(value, func(r rune) bool {
		return r == ',' || r == '|' || r == ' '
	})
	out := make([]string, 0, len(fields))
	for _, field := range fields {
		field = strings.TrimSpace(field)
		if field != "" {
			out = append(out, field)
		}
	}
	return out
}

func parseExtensions(value string) []facts.Extension {
	parts := strings.Split(value, ",")
	extensions := make([]facts.Extension, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		name, value, hasValue := strings.Cut(part, "=")
		name = strings.TrimSpace(name)
		if !strings.HasPrefix(name, "x-") {
			name = "x-" + name
		}
		if hasValue {
			extensions = append(extensions, facts.Extension{Name: name, Value: inferLiteral(strings.TrimSpace(value))})
		} else {
			extensions = append(extensions, facts.Extension{Name: name, Value: boolLiteral(true)})
		}
	}
	return extensions
}

func invalidSchemaTag(diags *diag.Accumulator, structName, fieldName, key, value, file string, line uint32) {
	diags.SchemaMetadataUnresolved(
		structName,
		fieldName,
		"invalid schema tag on "+structName+"."+fieldName+": "+key+"="+strconv.Quote(value)+
			" ignored by gnr8 metadata extraction (GO-06)",
		file,
		line,
	)
}

func parseStructTag(raw string) map[string]string {
	out := map[string]string{}
	for raw != "" {
		raw = strings.TrimLeft(raw, " ")
		if raw == "" {
			break
		}
		i := 0
		for i < len(raw) && raw[i] > ' ' && raw[i] != ':' && raw[i] != '"' && raw[i] != 0x7f {
			i++
		}
		if i == 0 || i+1 >= len(raw) || raw[i] != ':' || raw[i+1] != '"' {
			break
		}
		key := raw[:i]
		raw = raw[i+1:]
		i = 1
		for i < len(raw) {
			if raw[i] == '\\' {
				i += 2
				continue
			}
			if raw[i] == '"' {
				break
			}
			i++
		}
		if i >= len(raw) {
			break
		}
		quoted := raw[:i+1]
		value, err := strconv.Unquote(quoted)
		if err == nil {
			out[key] = value
		}
		raw = raw[i+1:]
	}
	return out
}

// mapCtx carries the per-field diagnostic identity (owning struct, field name,
// the as-written Go type, and the field's file:line) through the recursive
// mapType walk so float64 / free-form-map diagnostics render the DECLARED field
// type (e.g. "*float64") and the right position, not an unwrapped inner type.
type mapCtx struct {
	structName   string
	fieldName    string
	declaredType string
	modulePath   string
	file         string
	line         uint32
	diags        *diag.Accumulator
}

// mapType lowers a Go type into the neutral facts.Type vocabulary, incl. well-known
// types and the float64 / free-form-map diagnostics (RESEARCH Pattern 6).
func mapType(t gotypes.Type, ctx mapCtx) facts.Type {
	// Go 1.27 changed encoding/json.RawMessage from a defined []byte type to an
	// alias of encoding/json/jsontext.Value. Preserve the public standard-library
	// identity before Unalias erases it; RawMessage is still a free-form JSON
	// value, never a component schema owned by that internal package.
	if isJSONRawMessage(t) {
		return facts.AnyType()
	}
	switch u := gotypes.Unalias(t).(type) {
	case *gotypes.Pointer:
		// Nullability/optionality are recorded on the field; the type describes the elem.
		return mapType(u.Elem(), ctx)
	case *gotypes.Slice:
		return facts.ArrayType(mapType(u.Elem(), ctx))
	case *gotypes.Array:
		return facts.ArrayType(mapType(u.Elem(), ctx))
	case *gotypes.Map:
		if _, ok := gotypes.Unalias(u.Elem()).(*gotypes.Interface); ok {
			ctx.diags.FreeFormMap(ctx.structName, ctx.fieldName, ctx.declaredType, ctx.file, ctx.line)
			return facts.AnyType()
		}
		key := mapType(u.Key(), ctx)
		value := mapType(u.Elem(), ctx)
		return facts.MapTypeOf(key, value)
	case *gotypes.Named:
		return mapNamed(u, ctx)
	case *gotypes.Basic:
		return mapBasic(u, ctx)
	default:
		return facts.AnyType()
	}
}

func mapNamed(u *gotypes.Named, ctx mapCtx) facts.Type {
	obj := u.Obj()
	pkgPath := ""
	if obj.Pkg() != nil {
		pkgPath = obj.Pkg().Path()
	}
	switch {
	case pkgPath == uuidPkgPath && obj.Name() == "UUID":
		return facts.WellKnownType(facts.WellKnownUUID)
	case pkgPath == timePkgPath && obj.Name() == "Time":
		return facts.WellKnownType(facts.WellKnownDateTime)
	case pkgPath == jsonPkgPath && obj.Name() == "RawMessage":
		return facts.AnyType()
	case pkgPath == jsonPkgPath && obj.Name() == "Number":
		return facts.PrimitiveType(facts.FloatPrim(64))
	case pkgPath == multipartPkgPath && obj.Name() == "FileHeader":
		return facts.PrimitiveType(facts.BytesPrim())
	}
	// A named string (with or without a const set) refs its own schema; the enum
	// values are resolved by the enum SchemaFact (see Extract). A non-string named
	// type is a struct ref. Both are stable, package-qualified ids.
	return facts.NamedType(schemaID(u, ctx.modulePath))
}

func mapBasic(u *gotypes.Basic, ctx mapCtx) facts.Type {
	switch u.Kind() {
	case gotypes.Bool:
		return facts.PrimitiveType(facts.BoolPrim())
	case gotypes.String:
		return facts.PrimitiveType(facts.StringPrim())
	case gotypes.Int, gotypes.Int8, gotypes.Int16, gotypes.Int32:
		return facts.PrimitiveType(facts.IntPrim(32, true))
	case gotypes.Int64:
		return facts.PrimitiveType(facts.IntPrim(64, true))
	case gotypes.Uint, gotypes.Uint8, gotypes.Uint16, gotypes.Uint32:
		return facts.PrimitiveType(facts.IntPrim(32, false))
	case gotypes.Uint64:
		// Carry the `signed` axis faithfully: an unsigned source type is NOT a
		// signed int. The neutral Prim::Int { signed } exists precisely so a
		// target can distinguish uint64 from int64 (one source of truth per fact).
		return facts.PrimitiveType(facts.IntPrim(64, false))
	case gotypes.Float32:
		return facts.PrimitiveType(facts.FloatPrim(32))
	case gotypes.Float64:
		return facts.PrimitiveType(facts.FloatPrim(64))
	default:
		// An unsupported basic kind (complex64/128, uintptr, untyped constants,
		// ...) has no faithful neutral primitive. Emit a diagnostic and fall back
		// to the HONEST free-form `any` rather than fabricating a `string` fact
		// with no evidence (GO-06 / CLAUDE.md rule 3: diagnose, never guess).
		ctx.diags.UnsupportedType(ctx.structName, ctx.fieldName, ctx.declaredType, ctx.file, ctx.line)
		return facts.AnyType()
	}
}

// --- helpers -------------------------------------------------------------

func mainModulePath(res *load.Result) string {
	for _, pkg := range res.Packages {
		if pkg.Module != nil && pkg.Module.Main {
			return pkg.Module.Path
		}
	}
	// Fallback: longest common path prefix is unreliable; if no main module is
	// reported, return empty so isTargetPackage matches nothing rather than
	// pulling in stdlib/deps.
	return ""
}

func isTargetPackage(pkgPath, modulePath string) bool {
	if modulePath == "" {
		return false
	}
	if pkgPath != modulePath && !strings.HasPrefix(pkgPath, modulePath+"/") {
		return false
	}
	// Exclude the fixture's `expected/` tree: those packages (e.g. expected/sdk)
	// are hand-authored Phase-3 ACCEPTANCE SNAPSHOTS, not analyzer input. They
	// re-declare DTO names (CreateGoalInput, GoalResponse, ...) and would double
	// the schema set. Generated/expected output is never analysis input.
	rel := strings.TrimPrefix(strings.TrimPrefix(pkgPath, modulePath), "/")
	for _, seg := range strings.Split(rel, "/") {
		if seg == "expected" {
			return false
		}
	}
	return true
}

// schemaID is the package-qualified, module-relative type name, e.g.
// "internal/common/dto.CreateGoalInput".
func schemaID(named *gotypes.Named, modulePath string) string {
	obj := named.Obj()
	pkgPath := ""
	if obj.Pkg() != nil {
		pkgPath = obj.Pkg().Path()
	}
	rel := pkgPath
	if modulePath != "" && strings.HasPrefix(pkgPath, modulePath) {
		rel = strings.TrimPrefix(pkgPath, modulePath)
		rel = strings.TrimPrefix(rel, "/")
	}
	if rel == "" {
		return obj.Name()
	}
	return rel + "." + obj.Name()
}

func structHasPayloadTag(st *gotypes.Struct) bool {
	for i := 0; i < st.NumFields(); i++ {
		tag := reflect.StructTag(st.Tag(i))
		if _, ok := tag.Lookup("json"); ok {
			return true
		}
		if _, ok := tag.Lookup("form"); ok {
			return true
		}
		if st.Field(i).Embedded() {
			if embedded, ok := embeddedStruct(st.Field(i).Type()); ok {
				if structHasPayloadTag(embedded) {
					return true
				}
			}
		}
	}
	return false
}

// enumValues collects the sorted-by-caller string const values whose type is the
// given named string type, scanning the package scope (RESEARCH Pattern 6).
func enumValues(named *gotypes.Named, scope *gotypes.Scope) []string {
	values := make([]string, 0)
	for _, name := range scope.Names() {
		c, ok := scope.Lookup(name).(*gotypes.Const)
		if !ok {
			continue
		}
		cn, ok := gotypes.Unalias(c.Type()).(*gotypes.Named)
		if !ok || cn.Obj() != named.Obj() {
			continue
		}
		// Const value is a quoted Go string literal; strip the quotes.
		values = append(values, strings.Trim(c.Val().ExactString(), `"`))
	}
	return values
}

func embeddedStruct(t gotypes.Type) (*gotypes.Struct, bool) {
	named, ok := gotypes.Unalias(deref(t)).(*gotypes.Named)
	if !ok {
		return nil, false
	}
	st, ok := named.Underlying().(*gotypes.Struct)
	return st, ok
}

func embeddedTypeName(t gotypes.Type) string {
	if named, ok := gotypes.Unalias(deref(t)).(*gotypes.Named); ok {
		return named.Obj().Name()
	}
	return ""
}

func deref(t gotypes.Type) gotypes.Type {
	if p, ok := t.(*gotypes.Pointer); ok {
		return p.Elem()
	}
	return t
}

// payloadWire is the serializer that owns a struct field's wire form. What a
// field's two axes mean is a property of that serializer, so the wire selects
// the rule: only `encoding/json` writes `null`, and only it defines what the
// omission options drop.
type payloadWire int

const (
	// wireJSON: `encoding/json` governs — either a `json:` tag or no payload tag
	// at all (an untagged field is still marshalled, under its Go name).
	wireJSON payloadWire = iota
	// wireForm: a form/multipart binder governs. There is no `null` on that wire.
	wireForm
)

// The `json` tag's two omission options. They differ in WHICH values they drop:
// `omitempty` drops encoding/json's "empty" set, `omitzero` drops a type's zero
// value. Both are first-class `encoding/json` options (CLAUDE.md rule 0.1
// category 1) — neither is a marker any generator invented.
const (
	optOmitEmpty = "omitempty"
	optOmitZero  = "omitzero"
)

// parsePayloadTag returns the effective payload field name, the serializer that
// owns the field, the json omission option it carries (empty when none), and
// whether the field is skipped. JSON tags win when present; form tags let
// multipart/form DTOs participate in the same neutral schema extraction.
func parsePayloadTag(tag reflect.StructTag, goName string) (name string, wire payloadWire, omitOpt string, skip bool) {
	if raw, ok := tag.Lookup("json"); ok && raw != "" {
		name, omitOpt, skip = parseWireTag(raw, goName, wireJSON)
		return name, wireJSON, omitOpt, skip
	}
	if raw, ok := tag.Lookup("form"); ok && raw != "" {
		name, omitOpt, skip = parseWireTag(raw, goName, wireForm)
		return name, wireForm, omitOpt, skip
	}
	return goName, wireJSON, "", false
}

func parseWireTag(raw string, goName string, wire payloadWire) (name string, omitOpt string, skip bool) {
	parts := strings.Split(raw, ",")
	wireName := parts[0]
	if wireName == "-" && len(parts) == 1 {
		return "", "", true
	}
	if wireName == "" {
		wireName = goName
	}
	for _, opt := range parts[1:] {
		// Which options a wire reads is long-standing behavior, not a claim about which
		// of them `encoding/json` owns — it owns both (see above). `,omitempty` has
		// always been read on either wire; `,omitzero` was added for the json wire
		// alone. This change neither extends nor narrows that.
		if opt == optOmitEmpty || (opt == optOmitZero && wire == wireJSON) {
			omitOpt = opt
		}
	}
	return wireName, omitOpt, false
}

// presenceAndNullability derives serializer omission and deserializer null acceptance.
// Each wire has exactly one rule per fact (CLAUDE.md rule 3).
//
// `encoding/json` accepts a JSON null for every ordinary Go destination. Nilable values become nil;
// non-nilable values are left unchanged. A custom unmarshaler may reject null, but its method body is
// not a typed fact the extractor can prove; `force_non_nullable(..., SchemaUse::Input)` is the checked
// correction for that application-specific behavior. Serializer presence remains independently
// derived from the omission option.
//
// The form/multipart wire is left as it was. Its binder is not `encoding/json`
// and nothing here has established what it does with a bare pointer, so this
// change does not speak for it.
func presenceAndNullability(wire payloadWire, omitOpt string, t gotypes.Type) (optional, nullable bool) {
	if wire == wireForm {
		// A part is present or absent; there is no `null` to write, so the value axis
		// does not apply. Presence keeps its long-standing rule: a pointer part or an
		// omission option means the part may be missing.
		return isPointer(t) || omitOpt != "", false
	}
	return omitOptionOmits(t, omitOpt), true
}

func isPointer(t gotypes.Type) bool {
	_, ok := t.(*gotypes.Pointer)
	return ok
}

// omitOptionOmits reports whether a json omission option actually causes
// `encoding/json` to drop the key for this type.
//
//   - `omitzero` drops the type's zero value, for every type.
//   - `omitempty` drops only encoding/json's "empty" set: false, 0, "", a nil
//     pointer, a nil interface, and a zero-length array/slice/map/string. It is a
//     NO-OP on a struct, on a `time.Time`, and on an array of non-zero length —
//     those keys are always written, whatever the author intended.
func omitOptionOmits(t gotypes.Type, omitOpt string) bool {
	switch omitOpt {
	case optOmitZero:
		return true
	case optOmitEmpty:
		unaliased := gotypes.Unalias(t)
		if _, isTypeParam := unaliased.(*gotypes.TypeParam); isTypeParam {
			// Whether `omitempty` bites depends on the instantiation. Take the tag at
			// its word: a key that may be absent is the safe answer for a decoder.
			return true
		}
		switch u := unaliased.Underlying().(type) {
		case *gotypes.Basic, *gotypes.Pointer, *gotypes.Interface, *gotypes.Slice, *gotypes.Map:
			return true
		case *gotypes.Array:
			return u.Len() == 0
		default:
			return false
		}
	default:
		return false
	}
}

// zeroMarshalsNull reports whether this type's zero value marshals as JSON null.
// A nil pointer, slice, map, and interface all do; nothing else does. A named
// type is read through its underlying type, so `json.RawMessage` ([]byte) has a
// null zero while `time.Time` (a struct) and `uuid.UUID` ([16]byte) do not.
func zeroMarshalsNull(t gotypes.Type) bool {
	unaliased := gotypes.Unalias(t)
	if _, isTypeParam := unaliased.(*gotypes.TypeParam); isTypeParam {
		// A type parameter's underlying type is its CONSTRAINT, not the type that
		// will be marshalled — `T any` would read as an interface and claim every
		// generic field is nullable. What a `T` writes depends on the instantiation,
		// which this declaration does not fix, and unknown is not null.
		//
		// omitOptionOmits resolves the same `T` the other way, and the two are not in
		// conflict: there the author WROTE `,omitempty`, so there is a tag to read,
		// while here the only evidence would be `T` itself and there is none. Note
		// this is observable, not cosmetic — a `T` field maps to `Type::Any`, which
		// lowers to `type: object` and `dict[str, Any]`, neither of which admits
		// `null`. Widening it would need evidence about the instantiation.
		return false
	}
	switch unaliased.Underlying().(type) {
	case *gotypes.Pointer, *gotypes.Slice, *gotypes.Map, *gotypes.Interface:
		return true
	default:
		return false
	}
}

// outputMayEmitNull reports whether a present outbound key can carry JSON null.
// An omission option removes the nil zero value before it reaches the encoder, so
// ordinary pointers, slices, and maps become optional/non-null when present. Raw
// JSON and custom marshalers remain nullable because a non-zero value can itself
// encode the null token; interfaces can likewise hold a non-nil dynamic value
// whose representation is null.
func outputMayEmitNull(wire payloadWire, omitOpt string, t gotypes.Type) bool {
	if wire == wireForm {
		return false
	}
	if omitOpt == "" {
		return zeroMarshalsNull(t) || customMarshalerMayEmitNull(t)
	}
	if isJSONRawMessage(t) || customMarshalerMayEmitNull(t) {
		return true
	}
	unaliased := gotypes.Unalias(t)
	if _, isTypeParam := unaliased.(*gotypes.TypeParam); isTypeParam {
		// A type parameter's underlying type is its CONSTRAINT, so `T any` would read
		// as an interface below and claim every generic field emits null. What a `T`
		// writes depends on the instantiation, and unknown is not null — the same
		// answer zeroMarshalsNull gives on the no-option path, so an omission option
		// never widens nullability.
		return false
	}
	switch underlying := unaliased.Underlying().(type) {
	case *gotypes.Interface:
		return true
	case *gotypes.Pointer:
		return zeroMarshalsNull(underlying.Elem()) || customMarshalerMayEmitNull(underlying.Elem())
	default:
		return false
	}
}

// validationRejectsNull reports whether a required validator rejects the value
// left by decoding null. Ordinary scalars, arrays, pointers, slices, maps, and
// interfaces are zero/nil and fail `required`; a non-pointer struct's required
// rule is not enforced by the validator version Gin uses. RawMessage retains the
// non-empty bytes `null`, while a custom unmarshaler owns the resulting value, so
// neither carries an inferred rejection fact.
func validationRejectsNull(t gotypes.Type) bool {
	unaliased := gotypes.Unalias(t)
	if _, isPointer := unaliased.(*gotypes.Pointer); isPointer {
		// encoding/json stops at the settable field pointer when decoding null and
		// sets it to nil, without invoking the pointee's UnmarshalJSON method.
		return true
	}
	if isNamedType(t, timePkgPath, "Time") {
		return true
	}
	if isJSONRawMessage(t) || hasJSONUnmarshaler(t) {
		return false
	}
	if _, isTypeParam := unaliased.(*gotypes.TypeParam); isTypeParam {
		return false
	}
	_, isStruct := unaliased.Underlying().(*gotypes.Struct)
	return !isStruct
}

func isJSONRawMessage(t gotypes.Type) bool {
	// Walk the declared alias chain before using Underlying or Unalias. Starting
	// in Go 1.27, Unalias(json.RawMessage) is jsontext.Value, which has lost the
	// public name that states the source contract. Alias.Rhs retains that chain,
	// including through a user alias of json.RawMessage.
	for {
		switch current := t.(type) {
		case *gotypes.Pointer:
			t = current.Elem()
		case *gotypes.Alias:
			if typeNameMatches(current.Obj(), jsonPkgPath, "RawMessage") {
				return true
			}
			t = current.Rhs()
		case *gotypes.Named:
			return typeNameMatches(current.Obj(), jsonPkgPath, "RawMessage")
		default:
			return false
		}
	}
}

func hasJSONUnmarshaler(t gotypes.Type) bool {
	return hasJSONMethod(t, "UnmarshalJSON")
}

func hasJSONMarshaler(t gotypes.Type) bool {
	return hasJSONMethod(t, "MarshalJSON")
}

// customMarshalerMayEmitNull is conservative for user-defined MarshalJSON
// implementations: the method owns the wire bytes, so null is possible. The
// standard-library time.Time implementation is a closed, known exception and
// always emits a quoted timestamp.
func customMarshalerMayEmitNull(t gotypes.Type) bool {
	return hasJSONMarshaler(t) && !isNamedType(t, timePkgPath, "Time")
}

func isNamedType(t gotypes.Type, pkgPath string, name string) bool {
	unaliased := gotypes.Unalias(t)
	if pointer, ok := unaliased.(*gotypes.Pointer); ok {
		unaliased = gotypes.Unalias(pointer.Elem())
	}
	named, ok := unaliased.(*gotypes.Named)
	return ok && typeNameMatches(named.Obj(), pkgPath, name)
}

func typeNameMatches(obj *gotypes.TypeName, pkgPath string, name string) bool {
	return obj.Pkg() != nil && obj.Pkg().Path() == pkgPath && obj.Name() == name
}

func hasJSONMethod(t gotypes.Type, name string) bool {
	if methodSetHas(t, name) {
		return true
	}
	unaliased := gotypes.Unalias(t)
	if _, ok := unaliased.(*gotypes.Pointer); ok {
		return false
	}
	return methodSetHas(gotypes.NewPointer(t), name)
}

func methodSetHas(t gotypes.Type, name string) bool {
	methodSet := gotypes.NewMethodSet(t)
	for index := 0; index < methodSet.Len(); index++ {
		method, ok := methodSet.At(index).Obj().(*gotypes.Func)
		if ok && method.Name() == name && hasJSONMethodSignature(method, name) {
			return true
		}
	}
	return false
}

func hasJSONMethodSignature(method *gotypes.Func, name string) bool {
	signature, ok := method.Type().(*gotypes.Signature)
	if !ok {
		return false
	}
	bytesType := gotypes.NewSlice(gotypes.Typ[gotypes.Byte])
	errorType := gotypes.Universe.Lookup("error").Type()
	switch name {
	case "MarshalJSON":
		return signature.Params().Len() == 0 &&
			signature.Results().Len() == 2 &&
			gotypes.Identical(signature.Results().At(0).Type(), bytesType) &&
			gotypes.Identical(signature.Results().At(1).Type(), errorType)
	case "UnmarshalJSON":
		return signature.Params().Len() == 1 &&
			signature.Results().Len() == 1 &&
			gotypes.Identical(signature.Params().At(0).Type(), bytesType) &&
			gotypes.Identical(signature.Results().At(0).Type(), errorType)
	default:
		return false
	}
}

func spanOf(fset *token.FileSet, pos token.Pos) facts.SourceSpan {
	file, line := positionOf(fset, pos)
	return facts.SourceSpan{File: file, StartLine: line, EndLine: line}
}

func positionOf(fset *token.FileSet, pos token.Pos) (string, uint32) {
	if fset == nil || !pos.IsValid() {
		return "", 0
	}
	p := fset.Position(pos)
	return p.Filename, uint32(p.Line)
}

func optString(s string) *string {
	if s == "" {
		return nil
	}
	v := s
	return &v
}

func typeString(t gotypes.Type) string {
	// Render map[string]any as written; gotypes.TypeString renders interface{} as
	// "any" under go 1.18+ aliasing rules (the normalization is done by TypeString
	// itself). Keep it qualified-free for stability. Return the string directly —
	// it is already a string, so wrapping it in fmt.Sprintf("%s", ...) is a no-op
	// allocation that go vet's simplify (S1025) flags.
	return gotypes.TypeString(t, func(p *gotypes.Package) string { return p.Name() })
}
