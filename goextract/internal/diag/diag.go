// Package diag accumulates analysis diagnostics (severity + message + file:line)
// so that unsupported or lossy patterns surface as a diagnostic rather than a
// panic or a silent drop (GO-06 / D-10).
//
// The messages here keep a machine-stable rule + field identity (e.g.
// "CreateGoalInput.TargetValue (*float64)"). The canonical rendered wording that
// must reconcile with fixtures/goalservice/expected/diagnostics.txt is finalized
// on the Rust side in 02-03 — this package only needs to be stable and to carry
// the rule + identity + position.
package diag

import "github.com/gnr8/goextract/internal/facts"

const severityWarn = "WARN"
const severityError = "ERROR"
const severityInfo = "INFO"

const (
	categorySource           = "source"
	categoryRequestParameter = "request_parameter"
	categoryRequestBody      = "request_body"
	categoryResponse         = "response"
	categorySchema           = "schema"
	categorySecurity         = "security"
)

// Accumulator collects DiagnosticFact values during extraction.
type Accumulator struct {
	items []facts.DiagnosticFact
}

// AuthorizationSecurityMissing records an implementation read of the
// Authorization header. Typed source cannot state the corresponding security
// scheme, so the final graph resolves this warning only when user code applies
// an Authorization-carrying scheme to the operation.
func (a *Accumulator) AuthorizationSecurityMissing(method, route, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "security.requirement.missing",
		Severity: severityWarn,
		Category: categorySecurity,
		Message: "Authorization is read by " + method + " " + route +
			" but typed source cannot express its security scheme; configure an Authorization-carrying ApplySecurity transform in .gnr8",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   "Authorization",
	})
}

// New returns an empty accumulator.
func New() *Accumulator {
	return &Accumulator{items: []facts.DiagnosticFact{}}
}

// FreeFormMap records the free-form map diagnostic for a struct field
// (TARGET-API.md §5.1): map[string]any lowers to additionalProperties: true.
func (a *Accumulator) FreeFormMap(structName, fieldName, goType, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "schema.free_form_map",
		Severity: severityInfo,
		Category: categorySchema,
		Message: "free-form map field: " + structName + "." + fieldName +
			" (" + goType + ") lowers to additionalProperties: true (TARGET-API.md §5.1)",
		File:    file,
		Line:    line,
		EndLine: line,
		Schema:  structName,
		Subject: fieldName,
	})
}

// UntypedQueryParam records the untyped-query-param warning (TARGET-API.md §5.4):
// a c.Query("name") read with no binding struct, so the param's type/required-ness
// is under-specified by code alone. method + route identify the operation; the
// param name + rule are the machine-stable identity (the canonical rendered
// wording — which differs per param in expected/diagnostics.txt — is reconciled
// on the Rust side in 02-03).
func (a *Accumulator) UntypedQueryParam(name, method, route, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "request.parameter.unresolved",
		Severity: severityWarn,
		Category: categoryRequestParameter,
		Message: "untyped query param '" + name + "' on " + method + " " + route +
			": read via c.Query with no binding struct; param type/required-ness " +
			"under-specified, type inferred as string only (TARGET-API.md §5.4)",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   name,
	})
}

// RequestParameterUnresolved records a helper boundary where a Gin context was
// passed but the module-owned implementation could not be traversed. Keeping
// this distinct from an untyped direct Query read lets CI deny incomplete call
// graph analysis with the same stable request.parameter.unresolved identity.
func (a *Accumulator) RequestParameterUnresolved(subject, method, route, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "request.parameter.unresolved",
		Severity: severityWarn,
		Category: categoryRequestParameter,
		Message: "request parameter analysis stopped at " + subject + " on " + method + " " + route +
			": " + reason + "; add an explicit parameter override or keep the helper within the loaded module",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   subject,
	})
}

// CookieRequirednessUnresolved records a helper-wrapped cookie whose name and
// type are known but whose operation-specific absence behavior is not. The
// parameter remains optional because requiredness needs a positive proof.
func (a *Accumulator) CookieRequirednessUnresolved(name, method, route, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "request.parameter.unresolved",
		Severity: severityWarn,
		Category: categoryRequestParameter,
		Message: "cookie parameter '" + name + "' on " + method + " " + route +
			": requiredness cannot be proven from the operation caller's absence control flow; keep the absence branch explicit or add a typed parameter override",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   name,
	})
}

// RequestParameterAmbiguous records a bound parameter whose tags state one fact
// about it twice. This is an error rather than a warning because gnr8 read the
// source perfectly well and refused to settle it: choosing a winner would be a
// precedence rule, so the contradicted fact is dropped, and a parameter published
// without a rule its author wrote must not look like a complete extraction.
func (a *Accumulator) RequestParameterAmbiguous(subject, method, route, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "request.parameter.ambiguous",
		Severity: severityError,
		Category: categoryRequestParameter,
		Message: "contradictory tags on parameter " + subject + " on " + method + " " + route +
			": " + reason + "; gnr8 does not choose between them — delete all but one, at the scope " +
			"it belongs to (`dive` for what a collection holds)",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   subject,
	})
}

// RequestBodyUnresolved records a request-body shape or media type that Gin
// accepts at runtime but the extractor cannot represent faithfully. The stable
// code is intentionally distinct from request-parameter failures so callers can
// deny incomplete body extraction independently.
func (a *Accumulator) RequestBodyUnresolved(subject, method, route, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "request.body.unresolved",
		Severity: severityWarn,
		Category: categoryRequestBody,
		Message: "request body analysis is incomplete for " + subject + " on " + method + " " + route +
			": " + reason + "; add an explicit body override or use a statically typed binding",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   subject,
	})
}

// DynamicResponse records the dynamic/unresolvable-response warning (D-05 / GO-06):
// a c.JSON(...) whose status or body could not be resolved to a constant/named
// type. The response is diagnosed rather than guessed or silently dropped; there
// is no secondary source to recover it from (CLAUDE.md rule 3).
func (a *Accumulator) DynamicResponse(method, route, handler, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "response.schema.unresolved",
		Severity: severityWarn,
		Category: categoryResponse,
		Message: "dynamic response in handler " + handler + ": " + reason +
			"; cannot infer a typed response from code (TARGET-API.md §5; D-05)",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   handler,
	})
}

// ResponseSchemaUnresolved records a response body that cannot be represented
// faithfully even though the route is known.
func (a *Accumulator) ResponseSchemaUnresolved(method, route, subject, message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:      "response.schema.unresolved",
		Severity:  severityWarn,
		Category:  categoryResponse,
		Message:   message,
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   subject,
	})
}

// ResponseMediaTypeUnresolved records a response whose status and body kind are
// known but whose runtime media type is dynamic. The operation identity allows
// a checked response override to retire exactly the diagnostic it resolves.
func (a *Accumulator) ResponseMediaTypeUnresolved(method, route, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:      "response.media_type.unresolved",
		Severity:  severityWarn,
		Category:  categoryResponse,
		Message:   reason,
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
	})
}

// ResponseHeaderUnresolved records a response header the handler writes under a
// name typed source cannot state. The header is omitted rather than guessed, so
// the operation identity lets a response override declare it explicitly.
func (a *Accumulator) ResponseHeaderUnresolved(method, route, reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:      "response.header.unresolved",
		Severity:  severityWarn,
		Category:  categoryResponse,
		Message:   reason,
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
	})
}

// UnsupportedRoutePattern records a Gin route registration shape that cannot be
// lowered faithfully. The route is skipped rather than guessed so migration
// review can fix the source pattern or add an explicit custom Source/Transform.
func (a *Accumulator) UnsupportedRoutePattern(reason, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.route.unresolved",
		Severity: severityWarn,
		Category: categorySource,
		Message: "unsupported Gin route pattern: " + reason +
			"; route skipped rather than guessed (GO-04)",
		File:    file,
		Line:    line,
		EndLine: line,
	})
}

// SourceRouteUnresolved records a lossy route/group pattern when the caller has
// already described whether the route was skipped or emitted relatively.
func (a *Accumulator) SourceRouteUnresolved(message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.route.unresolved",
		Severity: severityWarn,
		Category: categorySource,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
	})
}

// SourceLoadUnresolved records a source package/file that could not be loaded.
func (a *Accumulator) SourceLoadUnresolved(message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.load.unresolved",
		Severity: severityWarn,
		Category: categorySource,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
	})
}

// SourceHandlerAmbiguous records a handler lookup collision that prevents an
// unambiguous association between a recognized route and its source function.
func (a *Accumulator) SourceHandlerAmbiguous(subject, message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.handler.ambiguous",
		Severity: severityWarn,
		Category: categorySource,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
		Subject:  subject,
	})
}

// UnsupportedType records that a struct field's declared type has no faithful
// neutral primitive (e.g. complex64/128, uintptr, an untyped constant kind), so
// the extractor lowers it to free-form `any` rather than guessing a concrete
// type (GO-06 / CLAUDE.md rule 3: diagnose, never fabricate). goType is the
// rendered Go type. method/route identity here is the struct.field + declared
// type, mirroring Floatf/FreeFormMap.
func (a *Accumulator) UnsupportedType(structName, fieldName, goType, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "schema.type.unresolved",
		Severity: severityWarn,
		Category: categorySchema,
		Message: "unsupported field type: " + structName + "." + fieldName +
			" (" + goType + ") has no neutral primitive; lowered to free-form any " +
			"rather than guessing a concrete type (GO-06)",
		File:    file,
		Line:    line,
		EndLine: line,
		Schema:  structName,
		Subject: fieldName,
	})
}

// IneffectiveOmitEmpty records a `json:",omitempty"` that `encoding/json` does
// not act on. The option drops only encoding/json's "empty" set, so on a struct,
// a `time.Time`, or a non-zero-length array the key is always written. gnr8
// publishes what the marshaller does, which is not what the author asked for —
// so it says which one it published and how to get the other.
func (a *Accumulator) IneffectiveOmitEmpty(structName, fieldName, goType, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "schema.omit_option.ineffective",
		Severity: severityInfo,
		Category: categorySchema,
		Message: "ineffective omission option: " + structName + "." + fieldName +
			" (" + goType + ") is tagged ',omitempty', but encoding/json omits only " +
			"false, 0, \"\", a nil pointer/interface, and a zero-length array/slice/map — " +
			"this key is always written, so the field is published as always present; " +
			"use ',omitzero' to omit the zero value",
		File:    file,
		Line:    line,
		EndLine: line,
		Schema:  structName,
		Subject: fieldName,
	})
}

// UnknownHandler records a recognized route whose handler declaration/body is
// unavailable. This is an error because emitting empty request/response facts
// would make extraction coverage look complete when it is not.
func (a *Accumulator) UnknownHandler(handler, method, route, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.handler.unresolved",
		Severity: severityError,
		Category: categorySource,
		Message: "unknown handler '" + handler + "' for " + method + " " + route +
			"; route extraction is incomplete and generation must not be treated as healthy (GO-06)",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   handler,
	})
}

// MissingResponses records a handler that was analyzed but supplied no static
// response evidence. Bodyless responses still have a status fact, so zero facts
// means extraction coverage is incomplete rather than intentionally bodyless.
func (a *Accumulator) MissingResponses(handler, method, route, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "response.missing",
		Severity: severityError,
		Category: categoryResponse,
		Message: "missing response facts for handler '" + handler + "' on " + method + " " + route +
			"; add a statically recognizable response or explicit code configuration (GO-06)",
		File:      file,
		Line:      line,
		EndLine:   line,
		Operation: method + " " + route,
		Subject:   handler,
	})
}

// Error records an actionable extraction failure that permits the helper to
// return its partial facts solely for diagnostics and review.
func (a *Accumulator) Error(message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "source.load.failed",
		Severity: severityError,
		Category: categorySource,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
	})
}

// SchemaMetadataUnresolved records a schema tag/default that could not be
// represented faithfully and was ignored or retained only as a string.
func (a *Accumulator) SchemaMetadataUnresolved(structName, fieldName, message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     "schema.metadata.unresolved",
		Severity: severityWarn,
		Category: categorySchema,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
		Schema:   structName,
		Subject:  fieldName,
	})
}

// Warn records a backwards-compatible generic warning for callers that do not
// yet have a dedicated structured helper.
func (a *Accumulator) Warn(message, file string, line uint32) {
	a.WarnCode("source.unresolved", categorySource, message, file, line)
}

// WarnCode records a warning with an explicit stable code and category.
func (a *Accumulator) WarnCode(code, category, message, file string, line uint32) {
	a.items = append(a.items, facts.DiagnosticFact{
		Code:     code,
		Severity: severityWarn,
		Category: category,
		Message:  message,
		File:     file,
		Line:     line,
		EndLine:  line,
	})
}

// Items returns the accumulated diagnostics. The caller (facts.Marshal) sorts
// them into a stable order before emitting.
func (a *Accumulator) Items() []facts.DiagnosticFact {
	return a.items
}
