// Package handlers analyzes each route's handler function body to infer the
// request body, responses (by numeric HTTP status), and path/query parameters
// from the supported typed Gin patterns (GO-05), and to emit diagnostics for the
// lossy cases (untyped query params, dynamic/unresolvable responses) rather than
// guessing or silently dropping (GO-06 / D-05).
//
// Recognized gin-context calls (gated on the resolved receiver package path
// `github.com/gin-gonic/gin`, never the alias text — shared with the route
// recognizer via routes.GinMethod):
//
//	c.ShouldBindJSON(&x)        -> request body type = TypeOf(x)
//	c.ShouldBind(&x)            -> form/multipart body when the bound DTO uses form tags
//	c.FormFile("name")          -> multipart file field on a synthesized request body
//	c.Request.FormFile("name")  -> the same multipart file field through net/http
//	c.PostForm("name")          -> form string field on a synthesized request body
//	c.JSON(http.StatusXxx, y)   -> responses[status] = TypeOf(y); status via go/constant
//	c.Param("name")             -> path param (string, required)
//	c.Query("name")             -> query param (string); control flow may prove required/optional
//	c.DefaultQuery("n", "d")    -> optional query param (string) + default
//	c.GetQuery("name")          -> optional query param (string)
//
// Status numbers come from `go/constant` on the resolved `*types.Const` (e.g.
// http.StatusCreated -> 201), never a hardcoded name->number map.
package handlers

import (
	"go/ast"
	"go/constant"
	"go/token"
	gotypes "go/types"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/docs"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/routes"
	"github.com/gnr8/goextract/internal/tags"
	"golang.org/x/tools/go/packages"
)

const maxContextHelperDepth = 32

// CodeFacts is the code-inferred contract for one handler: the request body, the
// responses keyed by status, and the params. This is the ONLY source of these
// facts — there is no annotation/fallback path (CLAUDE.md rules 1 & 3).
//
// Summary/Description are the handler's own doc comment read as PLAIN PROSE (rule 0.1
// category 2) — never a directive, never structural. They are carried here rather than
// read at the call site so a route and its prose come from the same resolved decl.
type CodeFacts struct {
	RequestBody            *facts.TypeRef
	RequestBodyRequired    bool
	RequestBodyContentType string
	RequestBodyVariants    []facts.RequestBodyVariantFact
	Responses              []facts.ResponseFact
	Params                 []facts.ParamFact
	Schemas                []facts.SchemaFact
	Summary                string
	Description            string
}

// handlerDecl bundles a handler FuncDecl with its owning package's type info and
// the shared FileSet, so the analyzer can resolve types and positions. pkgPath is
// the owning package's import path, used to disambiguate same-named handlers
// across packages (WR-02).
type handlerDecl struct {
	decl    *ast.FuncDecl
	info    *gotypes.Info
	fset    *token.FileSet
	pkgPath string
}

// identityKey is the fully-qualified, position-stable identity of a handler decl
// ("<pkgPath>.<receiver>.<name>@<file>:<line>"), used as a deterministic
// tie-break when two handlers share a bare name (WR-02). It is independent of
// package/file load order, so the chosen survivor is reproducible (GRAPH-02).
func (h handlerDecl) identityKey() string {
	name := ""
	if h.decl != nil && h.decl.Name != nil {
		name = h.decl.Name.Name
	}
	recv := receiverTypeName(h.decl)
	file, line := positionOf(h.fset, declPos(h.decl))
	return h.pkgPath + "." + recv + "." + name + "@" + file + ":" + strconv.FormatUint(uint64(line), 10)
}

// receiverTypeName renders the method receiver's base type name (e.g. "Handler"
// for `func (h *Handler) Foo()`), or "" for a plain function, so two methods of
// the same name on different receivers have distinct identities (WR-02).
func receiverTypeName(fn *ast.FuncDecl) string {
	if fn == nil || fn.Recv == nil || len(fn.Recv.List) == 0 {
		return ""
	}
	t := fn.Recv.List[0].Type
	if star, ok := t.(*ast.StarExpr); ok {
		t = star.X
	}
	if id, ok := t.(*ast.Ident); ok {
		return id.Name
	}
	return ""
}

// declPos returns the declaration's position, or token.NoPos when absent.
func declPos(fn *ast.FuncDecl) token.Pos {
	if fn == nil {
		return token.NoPos
	}
	return fn.Pos()
}

// Index maps handler symbol name -> its declaration across all target packages.
// Built once from the loaded packages and reused for every route.
type Index map[string]handlerDecl

// Analyzer carries the per-invocation context the handler analysis needs — the
// module prefix used to derive module-relative schema ids — alongside the handler
// Index. Threading this state through a struct (instead of package-level globals)
// makes the helper reentrant: two analyses over different modules in one process
// no longer clobber each other's prefix (WR-03), and the setup ordering is
// enforced by construction rather than by call discipline.
type Analyzer struct {
	idx           Index
	declsByObject map[string]handlerDecl
	modulePrefix  string
	collisions    []handlerCollision
}

type handlerCollision struct {
	name    string
	message string
	file    string
	line    uint32
}

type helperBinding struct {
	stringValue *string
	literal     *facts.LiteralValue
	typeValue   gotypes.Type
	ginContext  bool
}

type helperFrame struct {
	decl     handlerDecl
	bindings map[gotypes.Object]helperBinding
}

type parameterHint struct {
	schema        *facts.Type
	schemaKnown   bool
	required      bool
	requiredKnown bool
	defaultValue  *facts.LiteralValue
}

type queryPresenceProof uint8

const (
	queryPresenceUnresolved queryPresenceProof = iota
	queryPresenceOptional
	queryPresenceRequired
)

type queryAccessValue uint8

const (
	queryAccessUnknown queryAccessValue = iota
	queryAccessExact
	queryAccessNormalized
)

type queryResponseState uint8

const (
	queryResponseNone queryResponseState = iota
	queryResponseSuccess
	queryResponseClientError
	queryResponseUncertain
)

type queryPathState struct {
	values   map[gotypes.Object]queryAccessValue
	escaped  map[gotypes.Object]bool
	response queryResponseState
	checked  bool
}

// queryPathOutcome is how one explored path ended. queryPathOpen is the
// deliberate fourth value: a path that wrote no response gnr8 recognizes has
// stated nothing, which is weaker evidence than a path that answered 2xx and is
// not interchangeable with it.
type queryPathOutcome uint8

const (
	queryPathSuccess queryPathOutcome = iota
	queryPathClientError
	queryPathOpen
	queryPathUncertain
)

type queryTerminal struct {
	outcome queryPathOutcome
	checked bool
}

type queryFlow struct {
	next      []queryPathState
	terminals []queryTerminal
}

type untypedQueryRead struct {
	name string
	file string
	line uint32
}

type contextTraversal struct {
	route                  routes.Route
	cf                     *CodeFacts
	seenParam              map[string]bool
	resolvedParam          map[string]bool
	untypedQueryReads      *[]untypedQueryRead
	formFields             map[string]facts.FieldFact
	boundFormRefs          map[string]bool
	manualFormFields       map[string]bool
	formHasFile            *bool
	hasBodyBind            *bool
	allBodyBindsOptional   *bool
	diagnostics            *diag.Accumulator
	stack                  map[string]bool
	reportedUnresolvedCall map[token.Pos]bool
}

// NewAnalyzer builds an Analyzer from the loaded packages and the target module
// path. The module prefix qualifies handler-inferred schema refs into the 02-01
// module-relative format. Duplicate-name collisions are retained so callers can
// report only the ones that recognized routes actually reference (WR-02).
func NewAnalyzer(res *load.Result, module string, diags *diag.Accumulator) *Analyzer {
	idx, collisions := buildIndex(res)
	return &Analyzer{
		idx:           idx,
		declsByObject: buildDeclObjectIndex(res, module),
		modulePrefix:  module,
		collisions:    collisions,
	}
}

// Index exposes the underlying handler index (for callers that look up docs or
// build their own per-route flow).
func (a *Analyzer) Index() Index { return a.idx }

// ReportRouteHandlerCollisions emits duplicate-name diagnostics only for names
// that recognized routes actually reference. Helper/helper collisions that never
// affect route extraction are not actionable and stay silent.
func (a *Analyzer) ReportRouteHandlerCollisions(recognized []routes.Route, diags *diag.Accumulator) {
	if diags == nil || len(a.collisions) == 0 {
		return
	}
	routeHandlers := map[string]bool{}
	for _, route := range recognized {
		if route.HandlerKey == "" {
			routeHandlers[route.Handler] = true
		}
	}
	for _, collision := range a.collisions {
		if routeHandlers[collision.name] {
			diags.SourceHandlerAmbiguous(collision.name, collision.message, collision.file, collision.line)
		}
	}
}

// BuildIndex collects every function/method declaration in the target packages,
// keyed by its name, so routes can look up their handler by symbol.
//
// A bare function name is not unique across packages/receivers, so two handlers
// named the same in different packages would otherwise resolve last-write-wins by
// load order — both a correctness bug (wrong body/responses attached to a route)
// and a GRAPH-02 determinism hazard. To remove the load-order dependency, the
// surviving decl for a colliding name is chosen deterministically by its
// fully-qualified identity (package path, then receiver + file:line position), and
// the collision is surfaced as a diagnostic so it is never silent (WR-02).
func BuildIndex(res *load.Result, diags *diag.Accumulator) Index {
	idx, collisions := buildIndex(res)
	if diags != nil {
		for _, collision := range collisions {
			diags.SourceHandlerAmbiguous(collision.name, collision.message, collision.file, collision.line)
		}
	}
	return idx
}

func buildIndex(res *load.Result) (Index, []handlerCollision) {
	idx := make(Index)
	var collisions []handlerCollision
	for _, pkg := range res.Packages {
		if pkg.TypesInfo == nil {
			continue
		}
		for _, file := range pkg.Syntax {
			for _, d := range file.Decls {
				fn, ok := d.(*ast.FuncDecl)
				if !ok || fn.Name == nil {
					continue
				}
				cand := handlerDecl{decl: fn, info: pkg.TypesInfo, fset: res.Fset, pkgPath: pkg.PkgPath}
				existing, dup := idx[fn.Name.Name]
				if !dup {
					idx[fn.Name.Name] = cand
					continue
				}
				// Deterministic tie-break: keep the candidate whose fully-qualified
				// identity sorts first, independent of package/file iteration order.
				winner, loser := existing, cand
				if cand.identityKey() < existing.identityKey() {
					winner, loser = cand, existing
				}
				idx[fn.Name.Name] = winner
				file, line := positionOf(loser.fset, declPos(loser.decl))
				collisions = append(collisions, handlerCollision{
					name: fn.Name.Name,
					message: "duplicate handler name '" + fn.Name.Name + "': also declared at " +
						loser.identityKey() + "; route lookups by bare name are ambiguous, " +
						"keeping " + winner.identityKey() + " deterministically — qualify the " +
						"handler or disambiguate the route (WR-02)",
					file: file,
					line: line,
				})
			}
		}
	}
	return idx, collisions
}

func buildDeclObjectIndex(res *load.Result, module string) map[string]handlerDecl {
	out := make(map[string]handlerDecl)
	if res == nil {
		return out
	}
	for _, pkg := range modulePackages(res, module) {
		if pkg.TypesInfo == nil {
			continue
		}
		for _, file := range pkg.Syntax {
			for _, d := range file.Decls {
				fn, ok := d.(*ast.FuncDecl)
				if !ok || fn.Name == nil {
					continue
				}
				decl := handlerDecl{decl: fn, info: pkg.TypesInfo, fset: res.Fset, pkgPath: pkg.PkgPath}
				key := ""
				if obj, ok := pkg.TypesInfo.Defs[fn.Name].(*gotypes.Func); ok {
					key = routes.FuncObjectKey(obj)
				}
				if key == "" {
					key = funcDeclObjectKey(pkg.PkgPath, fn.Name.Pos())
				}
				out[key] = decl
			}
		}
	}
	return out
}

// modulePackages includes imported packages that belong to the target module,
// even when route loading was scoped to a narrower package pattern. Third-party
// dependencies are deliberately excluded: traversing their source would make
// extraction depend on dependency internals rather than the application's
// contract source.
func modulePackages(res *load.Result, module string) []*packages.Package {
	if res == nil {
		return nil
	}
	seen := map[string]bool{}
	out := []*packages.Package{}
	var visit func(*packages.Package)
	visit = func(pkg *packages.Package) {
		if pkg == nil || seen[pkg.ID] {
			return
		}
		seen[pkg.ID] = true
		if moduleOwnsPackage(module, pkg.PkgPath) {
			out = append(out, pkg)
		}
		for _, imported := range pkg.Imports {
			visit(imported)
		}
	}
	for _, pkg := range res.Packages {
		visit(pkg)
	}
	return out
}

func moduleOwnsPackage(module, pkgPath string) bool {
	if module == "" || pkgPath == "" {
		return false
	}
	return pkgPath == module || strings.HasPrefix(pkgPath, module+"/")
}

func funcDeclObjectKey(pkgPath string, pos token.Pos) string {
	if pkgPath == "" || !pos.IsValid() {
		return ""
	}
	return pkgPath + "@" + strconv.FormatInt(int64(pos), 10)
}

func funcObjectKey(fn *gotypes.Func) string {
	return routes.FuncObjectKey(fn)
}

func (a *Analyzer) moduleOwnedCallee(fn *gotypes.Func) (handlerDecl, bool) {
	if fn == nil || fn.Pkg() == nil || !moduleOwnsPackage(a.modulePrefix, fn.Pkg().Path()) {
		return handlerDecl{}, false
	}
	callee, ok := a.declsByObject[funcObjectKey(fn)]
	if !ok || callee.decl == nil || callee.decl.Body == nil {
		return handlerDecl{}, false
	}
	return callee, true
}

func (a *Analyzer) analyzeContextHelperCall(
	caller helperFrame,
	call *ast.CallExpr,
	inherited parameterHint,
	depth int,
	traversal *contextTraversal,
) {
	if traversal == nil || !frameCallPassesGinContext(caller, call) {
		return
	}
	fn := calledFuncObject(caller.decl.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		reason := "callee source is not available in the loaded module"
		if fn != nil && fn.Pkg() != nil && !moduleOwnsPackage(a.modulePrefix, fn.Pkg().Path()) {
			reason = "callee belongs to external package " + fn.Pkg().Path()
		}
		a.reportContextTraversalStop(caller, call, traversal, reason)
		return
	}
	if depth >= maxContextHelperDepth {
		a.reportContextTraversalStop(caller, call, traversal, "helper traversal exceeded the deterministic depth limit of 32")
		return
	}
	key := callee.identityKey()
	if traversal.stack[key] {
		a.reportContextTraversalStop(caller, call, traversal, "cycle detected while traversing module-owned helpers")
		return
	}

	next := helperFrame{
		decl:     callee,
		bindings: helperCallBindings(caller, call, fn),
	}
	hint := helperCallHint(caller, call, inherited)
	traversal.stack[key] = true
	defer delete(traversal.stack, key)

	optionalBindPositions := collectOptionalBindPositions(callee)
	for name, field := range a.multipartFileMapFieldsInFrame(next, traversal.route, traversal.diagnostics) {
		traversal.formFields[name] = field
		traversal.manualFormFields[name] = true
		*traversal.formHasFile = true
	}
	ast.Inspect(callee.decl.Body, func(node ast.Node) bool {
		nested, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ginCall := routes.GinMethod(callee.info, nested)
		if ginCall && recvPkg == routes.GinPkgPath {
			a.analyzeTraversedGinCall(next, nested, name, hint, optionalBindPositions, traversal)
			return true
		}
		if pname, matched, resolved := requestHeaderGetInFrame(next, nested); matched {
			if resolved {
				a.addTraversedParameter(traversal, requestParameter(
					pname,
					"header",
					requestAccessRequired(callee, nested, "GetHeader"),
					facts.PrimitiveType(facts.StringPrim()),
					callee.fset,
					nested.Pos(),
				), true)
			} else {
				a.reportDynamicParameterName(next, nested, traversal, "Request.Header.Get")
			}
		}
		if name, matched, resolved := requestFormFileInFrame(next, nested); matched {
			if resolved {
				addTraversedMultipartFileField(name, traversal)
			} else {
				a.reportDynamicBodyFieldName(next, nested, traversal, "Request.FormFile")
			}
		}
		if frameCallPassesGinContext(next, nested) {
			a.analyzeContextHelperCall(next, nested, hint, depth+1, traversal)
		}
		return true
	})
}

func (a *Analyzer) reportContextTraversalStop(
	caller helperFrame,
	call *ast.CallExpr,
	traversal *contextTraversal,
	reason string,
) {
	if traversal.diagnostics == nil || traversal.reportedUnresolvedCall[call.Pos()] {
		return
	}
	traversal.reportedUnresolvedCall[call.Pos()] = true
	subject := selectorName(call.Fun)
	if subject == "" {
		subject = "dynamic helper"
	}
	file, line := positionOf(caller.decl.fset, call.Pos())
	traversal.diagnostics.RequestParameterUnresolved(
		subject,
		traversal.route.Method,
		untypedRouteLabel(traversal.route),
		reason,
		file,
		line,
	)
}

func helperCallBindings(caller helperFrame, call *ast.CallExpr, fn *gotypes.Func) map[gotypes.Object]helperBinding {
	out := map[gotypes.Object]helperBinding{}
	if fn == nil {
		return out
	}
	sig, ok := gotypes.Unalias(fn.Type()).(*gotypes.Signature)
	if !ok || sig.Params() == nil {
		return out
	}
	for index, arg := range call.Args {
		if index >= sig.Params().Len() {
			break
		}
		out[sig.Params().At(index)] = helperBindingFromExpr(caller, arg)
	}
	return out
}

func helperBindingFromExpr(frame helperFrame, expr ast.Expr) helperBinding {
	binding := helperBinding{typeValue: frameTypeOf(frame, expr)}
	if id, ok := expr.(*ast.Ident); ok {
		if inherited, exists := frame.bindings[frame.decl.info.ObjectOf(id)]; exists {
			binding = inherited
			if binding.typeValue == nil {
				binding.typeValue = frame.decl.info.TypeOf(expr)
			}
		}
	}
	if value := frameLiteralValue(frame, expr); value != nil {
		binding.literal = value
		if stringValue, ok := literalStringValue(value); ok {
			binding.stringValue = &stringValue
		}
	}
	binding.ginContext = binding.ginContext || isGinContextType(binding.typeValue)
	return binding
}

func frameCallPassesGinContext(frame helperFrame, call *ast.CallExpr) bool {
	if call == nil || frame.decl.info == nil {
		return false
	}
	for _, arg := range call.Args {
		if isGinContextType(frameTypeOf(frame, arg)) {
			return true
		}
		if id, ok := arg.(*ast.Ident); ok {
			if binding, exists := frame.bindings[frame.decl.info.ObjectOf(id)]; exists && binding.ginContext {
				return true
			}
		}
	}
	return false
}

func frameTypeOf(frame helperFrame, expr ast.Expr) gotypes.Type {
	if expr == nil || frame.decl.info == nil {
		return nil
	}
	if id, ok := expr.(*ast.Ident); ok {
		if binding, exists := frame.bindings[frame.decl.info.ObjectOf(id)]; exists && binding.typeValue != nil {
			return binding.typeValue
		}
	}
	return frame.decl.info.TypeOf(expr)
}

func frameLiteralValue(frame helperFrame, expr ast.Expr) *facts.LiteralValue {
	if expr == nil || frame.decl.info == nil {
		return nil
	}
	if id, ok := expr.(*ast.Ident); ok {
		if binding, exists := frame.bindings[frame.decl.info.ObjectOf(id)]; exists && binding.literal != nil {
			return binding.literal
		}
	}
	return literalValue(frame.decl.info, expr)
}

func frameStringValue(frame helperFrame, expr ast.Expr) (string, bool) {
	if id, ok := expr.(*ast.Ident); ok && frame.decl.info != nil {
		if binding, exists := frame.bindings[frame.decl.info.ObjectOf(id)]; exists && binding.stringValue != nil {
			return *binding.stringValue, true
		}
	}
	return literalStringValue(frameLiteralValue(frame, expr))
}

func literalStringValue(value *facts.LiteralValue) (string, bool) {
	if value == nil || value.Type != "string" {
		return "", false
	}
	text, ok := value.Value.(string)
	return text, ok
}

func helperCallHint(frame helperFrame, call *ast.CallExpr, inherited parameterHint) parameterHint {
	hint := inherited
	if !hint.schemaKnown {
		if schema, ok := queryHelperSchema(frame.decl.info.TypeOf(call), call); ok {
			hint.schema = &schema
			hint.schemaKnown = true
		}
	}
	if hint.defaultValue == nil {
		hint.defaultValue = queryHelperDefaultFromArgs(frame.decl.info, call, signatureOf(frame.decl.info, call.Fun))
	}
	if !hint.requiredKnown {
		name := strings.ToLower(selectorName(call.Fun))
		switch {
		case strings.Contains(name, "optional") || hint.defaultValue != nil:
			hint.requiredKnown = true
			hint.required = false
		case strings.Contains(name, "required") || helperReturnsError(frame.decl.info, call):
			hint.requiredKnown = true
			hint.required = true
		}
	}
	return hint
}

func (a *Analyzer) analyzeTraversedGinCall(
	frame helperFrame,
	call *ast.CallExpr,
	method string,
	hint parameterHint,
	optionalBindPositions map[token.Pos]bool,
	traversal *contextTraversal,
) {
	switch method {
	case "Param":
		if name, ok := frameCallStringArg(frame, call, 0); ok {
			schema := facts.PrimitiveType(facts.StringPrim())
			if hint.schemaKnown && hint.schema != nil {
				schema = *hint.schema
			}
			a.addTraversedParameter(traversal, requestParameter(name, "path", true, schema, frame.decl.fset, call.Pos()), true)
		} else {
			a.reportDynamicParameterName(frame, call, traversal, method)
		}
	case "Query", "DefaultQuery", "GetQuery", "QueryArray", "GetQueryArray", "QueryMap":
		name, ok := frameCallStringArg(frame, call, 0)
		if !ok {
			a.reportDynamicParameterName(frame, call, traversal, method)
			return
		}
		if method == "Query" && !hint.requiredKnown {
			switch queryRequiredness(frame, name) {
			case queryPresenceOptional:
				hint.requiredKnown = true
				hint.required = false
			case queryPresenceRequired:
				hint.requiredKnown = true
				hint.required = true
			}
		}
		param := parameterFromGinAccess(frame.decl.info, method, name, frame.decl.fset, call, hint)
		resolved := method != "Query" || hint.schemaKnown || hint.requiredKnown || hint.defaultValue != nil
		a.addTraversedParameter(traversal, param, resolved)
		if method == "Query" && !hint.requiredKnown && traversal.diagnostics != nil {
			file, line := positionOf(frame.decl.fset, call.Pos())
			*traversal.untypedQueryReads = append(*traversal.untypedQueryReads, untypedQueryRead{name: name, file: file, line: line})
		}
	case "GetHeader":
		if name, ok := frameCallStringArg(frame, call, 0); ok {
			schema := facts.PrimitiveType(facts.StringPrim())
			if hint.schemaKnown && hint.schema != nil {
				schema = *hint.schema
			}
			required := requestAccessRequired(frame.decl, call, method)
			if hint.requiredKnown {
				required = hint.required
			}
			a.addTraversedParameter(traversal, requestParameter(name, "header", required, schema, frame.decl.fset, call.Pos()), true)
		} else {
			a.reportDynamicParameterName(frame, call, traversal, method)
		}
	case "Cookie":
		if name, ok := frameCallStringArg(frame, call, 0); ok {
			schema := facts.PrimitiveType(facts.StringPrim())
			if hint.schemaKnown && hint.schema != nil {
				schema = *hint.schema
			}
			required := requestAccessRequired(frame.decl, call, method)
			if hint.requiredKnown {
				required = hint.required
			}
			a.addTraversedParameter(traversal, requestParameter(name, "cookie", required, schema, frame.decl.fset, call.Pos()), true)
		} else {
			a.reportDynamicParameterName(frame, call, traversal, method)
		}
	case "ShouldBindQuery", "BindQuery":
		a.addBoundParameters(frame, call, "query", traversal.cf, traversal.seenParam, traversal.resolvedParam, traversal.route, traversal.diagnostics)
	case "ShouldBindHeader", "BindHeader":
		a.addBoundParameters(frame, call, "header", traversal.cf, traversal.seenParam, traversal.resolvedParam, traversal.route, traversal.diagnostics)
	case "ShouldBindJSON", "BindJSON":
		a.setTraversedRequestBody(frame, call, "application/json", optionalBindPositions, traversal)
	case "ShouldBind", "Bind", "ShouldBindWith", "BindWith":
		bound := boundTypeFromCall(frame, call)
		contentType := bindContentType(method, frame.decl.info, call, bound)
		if isFormContentType(contentType) {
			refID, ok := a.addBoundFormFields(
				frame,
				call,
				traversal.formFields,
				traversal.route,
				traversal.diagnostics,
			)
			if !ok {
				if traversal.diagnostics != nil {
					file, line := positionOf(frame.decl.fset, call.Pos())
					traversal.diagnostics.RequestBodyUnresolved(
						method,
						traversal.route.Method,
						untypedRouteLabel(traversal.route),
						"binding target does not resolve to a form object",
						file,
						line,
					)
				}
				return
			}
			if refID != "" {
				traversal.boundFormRefs[refID] = true
			}
			if contentType == "multipart/form-data" {
				*traversal.formHasFile = true
			}
			*traversal.hasBodyBind = true
			if !optionalBindPositions[call.Pos()] {
				*traversal.allBodyBindsOptional = false
			}
			return
		}
		a.setTraversedRequestBody(frame, call, contentType, optionalBindPositions, traversal)
	case "FormFile":
		if name, ok := frameCallStringArg(frame, call, 0); ok {
			addTraversedMultipartFileField(name, traversal)
		} else {
			a.reportDynamicBodyFieldName(frame, call, traversal, method)
		}
	case "PostForm", "DefaultPostForm", "GetPostForm":
		if name, ok := frameCallStringArg(frame, call, 0); ok {
			traversal.manualFormFields[name] = true
			if _, exists := traversal.formFields[name]; !exists {
				field := formField(
					name,
					facts.PrimitiveType(facts.StringPrim()),
					requestAccessRequired(frame.decl, call, method),
				)
				if method == "DefaultPostForm" && len(call.Args) > 1 {
					field.Meta = &facts.FieldMeta{Default: frameLiteralValue(frame, call.Args[1])}
				}
				traversal.formFields[name] = field
			}
		} else {
			a.reportDynamicBodyFieldName(frame, call, traversal, method)
		}
	}
}

func addTraversedMultipartFileField(name string, traversal *contextTraversal) {
	if traversal == nil {
		return
	}
	addMultipartFileField(
		traversal.formFields,
		traversal.manualFormFields,
		traversal.formHasFile,
		name,
	)
}

func (a *Analyzer) setTraversedRequestBody(
	frame helperFrame,
	call *ast.CallExpr,
	contentType string,
	optionalBindPositions map[token.Pos]bool,
	traversal *contextTraversal,
) {
	bound := boundTypeFromCall(frame, call)
	id, ok := a.namedTypeID(bound)
	if !ok {
		if !isTypeParameter(bound) && traversal.diagnostics != nil {
			file, line := positionOf(frame.decl.fset, call.Pos())
			traversal.diagnostics.RequestBodyUnresolved(
				selectorName(call.Fun),
				traversal.route.Method,
				untypedRouteLabel(traversal.route),
				"binding target does not resolve to a named schema",
				file,
				line,
			)
		}
		return
	}
	if contentType == "" && traversal.diagnostics != nil {
		file, line := positionOf(frame.decl.fset, call.Pos())
		traversal.diagnostics.RequestBodyUnresolved(
			selectorName(call.Fun),
			traversal.route.Method,
			untypedRouteLabel(traversal.route),
			"binding media type is selected dynamically or is unsupported",
			file,
			line,
		)
	}
	a.setRequestBodyFact(
		traversal.cf,
		&facts.TypeRef{RefID: id},
		contentType,
		traversal.route,
		traversal.diagnostics,
		frame.decl.fset,
		call.Pos(),
		selectorName(call.Fun),
	)
	*traversal.hasBodyBind = true
	if !optionalBindPositions[call.Pos()] {
		*traversal.allBodyBindsOptional = false
	}
}

func isTypeParameter(t gotypes.Type) bool {
	if t == nil {
		return false
	}
	if pointer, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = pointer.Elem()
	}
	_, ok := gotypes.Unalias(t).(*gotypes.TypeParam)
	return ok
}

func boundTypeFromCall(frame helperFrame, call *ast.CallExpr) gotypes.Type {
	if call == nil || len(call.Args) == 0 {
		return nil
	}
	expr := call.Args[0]
	if unary, ok := expr.(*ast.UnaryExpr); ok && unary.Op == token.AND {
		expr = unary.X
	}
	t := frameTypeOf(frame, expr)
	if t == nil {
		return nil
	}
	if pointer, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		return pointer.Elem()
	}
	return t
}

func frameCallStringArg(frame helperFrame, call *ast.CallExpr, index int) (string, bool) {
	if call == nil || index >= len(call.Args) {
		return "", false
	}
	return frameStringValue(frame, call.Args[index])
}

func (a *Analyzer) addTraversedParameter(traversal *contextTraversal, param facts.ParamFact, resolved bool) {
	a.addExtractedParameter(
		traversal.cf,
		traversal.seenParam,
		traversal.resolvedParam,
		param,
		resolved,
		traversal.route,
		traversal.diagnostics,
	)
}

func (a *Analyzer) reportDynamicParameterName(
	frame helperFrame,
	call *ast.CallExpr,
	traversal *contextTraversal,
	method string,
) {
	if traversal.diagnostics == nil {
		return
	}
	file, line := positionOf(frame.decl.fset, call.Pos())
	traversal.diagnostics.RequestParameterUnresolved(
		method,
		traversal.route.Method,
		untypedRouteLabel(traversal.route),
		"parameter name is dynamic",
		file,
		line,
	)
}

func (a *Analyzer) reportDynamicBodyFieldName(
	frame helperFrame,
	call *ast.CallExpr,
	traversal *contextTraversal,
	method string,
) {
	if traversal.diagnostics == nil {
		return
	}
	file, line := positionOf(frame.decl.fset, call.Pos())
	reason := "form field name is dynamic"
	if method == "FormFile" || method == "Request.FormFile" {
		reason = "multipart field name is dynamic"
	}
	traversal.diagnostics.RequestBodyUnresolved(
		method,
		traversal.route.Method,
		untypedRouteLabel(traversal.route),
		reason,
		file,
		line,
	)
}

// multipartFileMapFields recognizes literal keys read from the File map of a
// multipart.Form returned by Gin. A literal key is a bounded repeated-file
// contract; a computed key can select an unbounded set and is diagnosed.
func (a *Analyzer) multipartFileMapFields(
	h handlerDecl,
	route routes.Route,
	diags *diag.Accumulator,
) map[string]facts.FieldFact {
	return a.multipartFileMapFieldsInFrame(
		helperFrame{decl: h, bindings: map[gotypes.Object]helperBinding{}},
		route,
		diags,
	)
}

func (a *Analyzer) multipartFileMapFieldsInFrame(
	frame helperFrame,
	route routes.Route,
	diags *diag.Accumulator,
) map[string]facts.FieldFact {
	fields := map[string]facts.FieldFact{}
	formVars := multipartFormResultVars(frame.decl)
	if len(formVars) == 0 || frame.decl.decl == nil || frame.decl.decl.Body == nil {
		return fields
	}
	ast.Inspect(frame.decl.decl.Body, func(node ast.Node) bool {
		index, ok := node.(*ast.IndexExpr)
		if !ok {
			return true
		}
		selector, ok := index.X.(*ast.SelectorExpr)
		if !ok || selector.Sel == nil || selector.Sel.Name != "File" {
			return true
		}
		base, ok := selector.X.(*ast.Ident)
		if !ok || !formVars[frame.decl.info.ObjectOf(base)] {
			return true
		}
		name, resolved := frameStringValue(frame, index.Index)
		if !resolved {
			if diags != nil {
				file, line := positionOf(frame.decl.fset, index.Pos())
				diags.RequestBodyUnresolved(
					"multipart file map",
					route.Method,
					untypedRouteLabel(route),
					"multipart file field name is dynamic",
					file,
					line,
				)
			}
			return true
		}
		fields[name] = formField(
			name,
			facts.ArrayType(facts.PrimitiveType(facts.BytesPrim())),
			false,
		)
		return true
	})
	return fields
}

func multipartFormResultVars(h handlerDecl) map[gotypes.Object]bool {
	vars := map[gotypes.Object]bool{}
	if h.decl == nil || h.decl.Body == nil || h.info == nil {
		return vars
	}
	ast.Inspect(h.decl.Body, func(node ast.Node) bool {
		assign, ok := node.(*ast.AssignStmt)
		if !ok || len(assign.Rhs) != 1 || len(assign.Lhs) == 0 {
			return true
		}
		call, ok := assign.Rhs[0].(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if !ok || recvPkg != routes.GinPkgPath || name != "MultipartForm" {
			return true
		}
		if ident, ok := assign.Lhs[0].(*ast.Ident); ok {
			if object := h.info.ObjectOf(ident); object != nil {
				vars[object] = true
			}
		}
		return true
	})
	return vars
}

// queryRequiredness proves whether a direct Query read is required or optional
// by following the function's native control flow twice: once with the value
// absent and once with it present. Unknown conditions are explored on both
// branches. A proof is retained only when every absent path agrees; mixed,
// unsupported, or helper-controlled paths deliberately remain unresolved.
//
// Requiredness needs every absent path to answer 4xx. Optionality needs every
// absent path to answer a status gnr8 read, not merely to reach the end of the
// function: a path may answer through a writer this analysis does not follow
// (c.Writer, a gin method gnr8 does not name), and silence there is not evidence
// of success. Both proofs additionally need one present path that does not reject
// the value, so a handler that fails either way proves nothing.
func queryRequiredness(frame helperFrame, name string) queryPresenceProof {
	if frame.decl.decl == nil || frame.decl.decl.Body == nil || frame.decl.info == nil ||
		!frameReadsDirectQuery(frame, name) {
		return queryPresenceUnresolved
	}
	escaped := escapedQueryValues(frame.decl)
	absent := analyzeQueryStatements(frame, name, false, frame.decl.decl.Body.List, []queryPathState{newQueryPathState(escaped)})
	present := analyzeQueryStatements(frame, name, true, frame.decl.decl.Body.List, []queryPathState{newQueryPathState(escaped)})
	absent.terminals = append(absent.terminals, terminalQueryStates(absent.next)...)
	present.terminals = append(present.terminals, terminalQueryStates(present.next)...)

	if allQueryTerminals(absent.terminals, queryPathClientError, false) &&
		hasQueryContinuation(present.terminals) {
		return queryPresenceRequired
	}
	if allQueryTerminals(absent.terminals, queryPathSuccess, true) &&
		hasQueryContinuation(present.terminals) {
		return queryPresenceOptional
	}
	return queryPresenceUnresolved
}

func newQueryPathState(escaped map[gotypes.Object]bool) queryPathState {
	return queryPathState{values: map[gotypes.Object]queryAccessValue{}, escaped: escaped}
}

// escapedQueryValues collects the locals whose value can change through a write
// this analysis never sees: the address is taken, or a function literal captures
// and assigns them. Their tracked access value would otherwise survive that write,
// so a value a helper fills in still reads as the absent one — which turns an
// unreachable rejection branch into a proof that the parameter is required.
func escapedQueryValues(h handlerDecl) map[gotypes.Object]bool {
	escaped := map[gotypes.Object]bool{}
	ast.Inspect(h.decl.Body, func(node ast.Node) bool {
		switch current := node.(type) {
		case *ast.UnaryExpr:
			if current.Op == token.AND {
				markEscapedQueryValue(h.info, current.X, escaped)
			}
		case *ast.FuncLit:
			if current.Body == nil {
				return true
			}
			ast.Inspect(current.Body, func(inner ast.Node) bool {
				switch assign := inner.(type) {
				case *ast.AssignStmt:
					for _, lhs := range assign.Lhs {
						markEscapedQueryValue(h.info, lhs, escaped)
					}
				case *ast.IncDecStmt:
					markEscapedQueryValue(h.info, assign.X, escaped)
				}
				return true
			})
		}
		return true
	})
	return escaped
}

func markEscapedQueryValue(info *gotypes.Info, expr ast.Expr, escaped map[gotypes.Object]bool) {
	ident, ok := expr.(*ast.Ident)
	if !ok || info == nil {
		return
	}
	if object := info.ObjectOf(ident); object != nil {
		escaped[object] = true
	}
}

func frameReadsDirectQuery(frame helperFrame, name string) bool {
	found := false
	ast.Inspect(frame.decl.decl.Body, func(node ast.Node) bool {
		if found {
			return false
		}
		if _, ok := node.(*ast.FuncLit); ok {
			return false
		}
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		method, recvPkg, ok := routes.GinMethod(frame.decl.info, call)
		if ok && recvPkg == routes.GinPkgPath && method == "Query" {
			queryName, resolved := frameCallStringArg(frame, call, 0)
			found = resolved && queryName == name
		}
		return !found
	})
	return found
}

const maxQueryFlowPaths = 128

func analyzeQueryStatements(
	frame helperFrame,
	name string,
	present bool,
	statements []ast.Stmt,
	initial []queryPathState,
) queryFlow {
	flow := queryFlow{next: initial}
	for _, statement := range statements {
		if len(flow.next) == 0 {
			break
		}
		next := make([]queryPathState, 0, len(flow.next))
		for _, state := range flow.next {
			step := analyzeQueryStatement(frame, name, present, statement, state)
			next = append(next, step.next...)
			flow.terminals = append(flow.terminals, step.terminals...)
		}
		if len(next) > maxQueryFlowPaths {
			flow.terminals = append(flow.terminals, queryTerminal{outcome: queryPathUncertain})
			next = next[:maxQueryFlowPaths]
		}
		flow.next = next
	}
	return flow
}

func analyzeQueryStatement(
	frame helperFrame,
	name string,
	present bool,
	statement ast.Stmt,
	state queryPathState,
) queryFlow {
	switch node := statement.(type) {
	case *ast.BlockStmt:
		return analyzeQueryStatements(frame, name, present, node.List, []queryPathState{state})
	case *ast.AssignStmt:
		updateQueryResponse(frame, node, &state)
		assignQueryValues(frame, name, node, &state)
		return queryFlow{next: []queryPathState{state}}
	case *ast.DeclStmt:
		updateQueryResponse(frame, node, &state)
		declareQueryValues(frame, name, node, &state)
		return queryFlow{next: []queryPathState{state}}
	case *ast.ExprStmt:
		updateQueryResponse(frame, node.X, &state)
		if call, ok := node.X.(*ast.CallExpr); ok && isBuiltinCall(frame.decl.info, call, "panic") {
			return queryFlow{terminals: []queryTerminal{{outcome: queryPathUncertain, checked: state.checked}}}
		}
		return queryFlow{next: []queryPathState{state}}
	case *ast.ReturnStmt:
		updateQueryResponse(frame, node, &state)
		return queryFlow{terminals: []queryTerminal{terminalQueryState(state)}}
	case *ast.IfStmt:
		if node.Init != nil {
			initFlow := analyzeQueryStatement(frame, name, present, node.Init, state)
			if len(initFlow.next) != 1 || len(initFlow.terminals) != 0 {
				return queryFlow{terminals: []queryTerminal{{outcome: queryPathUncertain, checked: state.checked}}}
			}
			state = initFlow.next[0]
		}
		updateQueryResponse(frame, node.Cond, &state)
		truth, checked := queryConditionValue(frame, name, present, node.Cond, state)
		state.checked = state.checked || checked
		flow := queryFlow{}
		if truth != queryBoolFalse {
			branch := analyzeQueryStatements(frame, name, present, node.Body.List, []queryPathState{cloneQueryPathState(state)})
			flow.next = append(flow.next, branch.next...)
			flow.terminals = append(flow.terminals, branch.terminals...)
		}
		if truth != queryBoolTrue {
			branch := analyzeQueryElse(frame, name, present, node.Else, cloneQueryPathState(state))
			flow.next = append(flow.next, branch.next...)
			flow.terminals = append(flow.terminals, branch.terminals...)
		}
		return flow
	case *ast.LabeledStmt:
		return analyzeQueryStatement(frame, name, present, node.Stmt, state)
	case *ast.EmptyStmt:
		return queryFlow{next: []queryPathState{state}}
	case *ast.IncDecStmt:
		updateQueryResponse(frame, node, &state)
		forgetQueryValue(frame.decl.info, node.X, &state)
		return queryFlow{next: []queryPathState{state}}
	case *ast.GoStmt, *ast.DeferStmt:
		updateQueryResponse(frame, node, &state)
		return queryFlow{next: []queryPathState{state}}
	case *ast.ForStmt, *ast.RangeStmt, *ast.SwitchStmt, *ast.TypeSwitchStmt, *ast.SelectStmt, *ast.BranchStmt:
		// Loops, switches, selects, and jumps need a full inter-block fixed-point
		// proof. Until one exists, keep both a possible continuation and an
		// uncertain terminal so neither required nor optional can be fabricated.
		return queryFlow{
			next:      []queryPathState{state},
			terminals: []queryTerminal{{outcome: queryPathUncertain, checked: state.checked}},
		}
	default:
		updateQueryResponse(frame, node, &state)
		return queryFlow{
			next:      []queryPathState{state},
			terminals: []queryTerminal{{outcome: queryPathUncertain, checked: state.checked}},
		}
	}
}

func analyzeQueryElse(
	frame helperFrame,
	name string,
	present bool,
	statement ast.Stmt,
	state queryPathState,
) queryFlow {
	if statement == nil {
		return queryFlow{next: []queryPathState{state}}
	}
	return analyzeQueryStatement(frame, name, present, statement, state)
}

func cloneQueryPathState(state queryPathState) queryPathState {
	clone := state
	clone.values = make(map[gotypes.Object]queryAccessValue, len(state.values))
	for object, value := range state.values {
		clone.values[object] = value
	}
	return clone
}

func terminalQueryStates(states []queryPathState) []queryTerminal {
	terminals := make([]queryTerminal, 0, len(states))
	for _, state := range states {
		terminals = append(terminals, terminalQueryState(state))
	}
	return terminals
}

func terminalQueryState(state queryPathState) queryTerminal {
	outcome := queryPathOpen
	switch state.response {
	case queryResponseSuccess:
		outcome = queryPathSuccess
	case queryResponseClientError:
		outcome = queryPathClientError
	case queryResponseUncertain:
		outcome = queryPathUncertain
	}
	return queryTerminal{outcome: outcome, checked: state.checked}
}

func allQueryTerminals(terminals []queryTerminal, outcome queryPathOutcome, requireChecked bool) bool {
	if len(terminals) == 0 {
		return false
	}
	for _, terminal := range terminals {
		if terminal.outcome != outcome || (requireChecked && !terminal.checked) {
			return false
		}
	}
	return true
}

// hasQueryContinuation reports whether the value, once supplied, can still be
// answered without a client error. A path that ended open qualifies: it did not
// reject the request, which is all either proof asks of the present run.
func hasQueryContinuation(terminals []queryTerminal) bool {
	for _, terminal := range terminals {
		if terminal.outcome == queryPathSuccess || terminal.outcome == queryPathOpen {
			return true
		}
	}
	return false
}

func assignQueryValues(frame helperFrame, name string, assign *ast.AssignStmt, state *queryPathState) {
	if assign == nil || state == nil {
		return
	}
	values := make([]queryAccessValue, len(assign.Lhs))
	if len(assign.Rhs) == len(assign.Lhs) {
		for index, expr := range assign.Rhs {
			values[index] = queryAccessFromExpr(frame, name, expr, *state)
		}
	}
	for index, lhs := range assign.Lhs {
		ident, ok := lhs.(*ast.Ident)
		if !ok || ident.Name == "_" {
			continue
		}
		object := frame.decl.info.ObjectOf(ident)
		if object == nil {
			continue
		}
		if (assign.Tok != token.ASSIGN && assign.Tok != token.DEFINE) || state.escaped[object] {
			state.values[object] = queryAccessUnknown
			continue
		}
		state.values[object] = values[index]
	}
}

func declareQueryValues(frame helperFrame, name string, decl *ast.DeclStmt, state *queryPathState) {
	if decl == nil || state == nil {
		return
	}
	gen, ok := decl.Decl.(*ast.GenDecl)
	if !ok || gen.Tok != token.VAR {
		return
	}
	for _, spec := range gen.Specs {
		valueSpec, ok := spec.(*ast.ValueSpec)
		if !ok {
			continue
		}
		values := make([]queryAccessValue, len(valueSpec.Names))
		if len(valueSpec.Values) == len(valueSpec.Names) {
			for index, expr := range valueSpec.Values {
				values[index] = queryAccessFromExpr(frame, name, expr, *state)
			}
		}
		for index, ident := range valueSpec.Names {
			if ident == nil || ident.Name == "_" {
				continue
			}
			object := frame.decl.info.ObjectOf(ident)
			if object == nil {
				continue
			}
			if state.escaped[object] {
				state.values[object] = queryAccessUnknown
				continue
			}
			state.values[object] = values[index]
		}
	}
}

func forgetQueryValue(info *gotypes.Info, expr ast.Expr, state *queryPathState) {
	ident, ok := expr.(*ast.Ident)
	if !ok || info == nil || state == nil {
		return
	}
	if object := info.ObjectOf(ident); object != nil {
		state.values[object] = queryAccessUnknown
	}
}

func queryAccessFromExpr(frame helperFrame, name string, expr ast.Expr, state queryPathState) queryAccessValue {
	switch node := expr.(type) {
	case *ast.ParenExpr:
		return queryAccessFromExpr(frame, name, node.X, state)
	case *ast.Ident:
		return state.values[frame.decl.info.ObjectOf(node)]
	case *ast.CallExpr:
		method, recvPkg, ok := routes.GinMethod(frame.decl.info, node)
		if ok && recvPkg == routes.GinPkgPath && method == "Query" {
			queryName, resolved := frameCallStringArg(frame, node, 0)
			if resolved && queryName == name {
				return queryAccessExact
			}
			return queryAccessUnknown
		}
		fn := calledFuncObject(frame.decl.info, node.Fun)
		if fn != nil && fn.Pkg() != nil && fn.Pkg().Path() == "strings" && fn.Name() == "TrimSpace" && len(node.Args) == 1 {
			value := queryAccessFromExpr(frame, name, node.Args[0], state)
			if value == queryAccessExact || value == queryAccessNormalized {
				return queryAccessNormalized
			}
		}
	}
	return queryAccessUnknown
}

type queryBool uint8

const (
	queryBoolUnknown queryBool = iota
	queryBoolFalse
	queryBoolTrue
)

func queryConditionValue(
	frame helperFrame,
	name string,
	present bool,
	expr ast.Expr,
	state queryPathState,
) (queryBool, bool) {
	switch node := expr.(type) {
	case *ast.ParenExpr:
		return queryConditionValue(frame, name, present, node.X, state)
	case *ast.UnaryExpr:
		if node.Op != token.NOT {
			return queryBoolUnknown, false
		}
		value, checked := queryConditionValue(frame, name, present, node.X, state)
		return invertQueryBool(value), checked
	case *ast.BinaryExpr:
		switch node.Op {
		case token.LAND, token.LOR:
			left, leftChecked := queryConditionValue(frame, name, present, node.X, state)
			right, rightChecked := queryConditionValue(frame, name, present, node.Y, state)
			if node.Op == token.LAND {
				return andQueryBool(left, right), leftChecked || rightChecked
			}
			return orQueryBool(left, right), leftChecked || rightChecked
		case token.EQL, token.NEQ:
			access := queryAccessFromExpr(frame, name, node.X, state)
			if access != queryAccessUnknown && isEmptyStringLiteral(node.Y) {
				return compareQueryAccess(access, present, node.Op), true
			}
			access = queryAccessFromExpr(frame, name, node.Y, state)
			if access != queryAccessUnknown && isEmptyStringLiteral(node.X) {
				return compareQueryAccess(access, present, node.Op), true
			}
			access = queryLengthAccess(frame, name, node.X, state)
			if access != queryAccessUnknown && isZeroInteger(node.Y) {
				return compareQueryAccess(access, present, node.Op), true
			}
			access = queryLengthAccess(frame, name, node.Y, state)
			if access != queryAccessUnknown && isZeroInteger(node.X) {
				return compareQueryAccess(access, present, node.Op), true
			}
		}
	}
	if value := frame.decl.info.Types[expr].Value; value != nil && value.Kind() == constant.Bool {
		if constant.BoolVal(value) {
			return queryBoolTrue, false
		}
		return queryBoolFalse, false
	}
	return queryBoolUnknown, false
}

func queryLengthAccess(frame helperFrame, name string, expr ast.Expr, state queryPathState) queryAccessValue {
	call, ok := expr.(*ast.CallExpr)
	if !ok || len(call.Args) != 1 || !isBuiltinCall(frame.decl.info, call, "len") {
		return queryAccessUnknown
	}
	return queryAccessFromExpr(frame, name, call.Args[0], state)
}

func isBuiltinCall(info *gotypes.Info, call *ast.CallExpr, name string) bool {
	if info == nil || call == nil {
		return false
	}
	ident, ok := call.Fun.(*ast.Ident)
	if !ok || ident.Name != name {
		return false
	}
	object, ok := info.ObjectOf(ident).(*gotypes.Builtin)
	return ok && object.Name() == name
}

func compareQueryAccess(access queryAccessValue, present bool, op token.Token) queryBool {
	if present && access == queryAccessNormalized {
		return queryBoolUnknown
	}
	empty := !present
	if op == token.EQL {
		if empty {
			return queryBoolTrue
		}
		return queryBoolFalse
	}
	if empty {
		return queryBoolFalse
	}
	return queryBoolTrue
}

func invertQueryBool(value queryBool) queryBool {
	switch value {
	case queryBoolFalse:
		return queryBoolTrue
	case queryBoolTrue:
		return queryBoolFalse
	default:
		return queryBoolUnknown
	}
}

func andQueryBool(left, right queryBool) queryBool {
	if left == queryBoolFalse || right == queryBoolFalse {
		return queryBoolFalse
	}
	if left == queryBoolTrue && right == queryBoolTrue {
		return queryBoolTrue
	}
	return queryBoolUnknown
}

func orQueryBool(left, right queryBool) queryBool {
	if left == queryBoolTrue || right == queryBoolTrue {
		return queryBoolTrue
	}
	if left == queryBoolFalse && right == queryBoolFalse {
		return queryBoolFalse
	}
	return queryBoolUnknown
}

func updateQueryResponse(frame helperFrame, node ast.Node, state *queryPathState) {
	if node == nil || state == nil {
		return
	}
	ast.Inspect(node, func(current ast.Node) bool {
		if current == nil {
			return false
		}
		if _, ok := current.(*ast.FuncLit); ok {
			return false
		}
		call, ok := current.(*ast.CallExpr)
		if !ok {
			return true
		}
		method, recvPkg, ginCall := routes.GinMethod(frame.decl.info, call)
		if ginCall && recvPkg == routes.GinPkgPath {
			mergeQueryResponse(state, ginResponseOutcome(frame.decl.info, method, call))
			return true
		}
		if frameCallPassesGinContext(frame, call) {
			mergeQueryResponse(state, queryResponseUncertain)
		}
		return true
	})
}

// ginResponseOutcome classifies what one gin.Context call contributes to a path's
// response state. A method that does not answer the request contributes nothing;
// one that answers with a status gnr8 cannot read as a constant is uncertain.
// Naming the response surface completely matters: an answer gnr8 does not
// recognize leaves the path open rather than looking like a success, so the cost
// of an omission here is a lost proof, never a wrong one.
func ginResponseOutcome(info *gotypes.Info, method string, call *ast.CallExpr) queryResponseState {
	if ginFixedSuccessResponse(method) {
		return queryResponseSuccess
	}
	if !ginStatusArgumentResponse(method) {
		return queryResponseNone
	}
	if len(call.Args) == 0 {
		return queryResponseUncertain
	}
	status, known := statusOf(info, call.Args[0])
	switch {
	case !known:
		return queryResponseUncertain
	case status >= 400 && status < 500:
		return queryResponseClientError
	case status >= 200 && status < 400:
		return queryResponseSuccess
	}
	return queryResponseUncertain
}

// ginFixedSuccessResponse names the gin.Context methods that answer with a status
// the method itself fixes at 200.
func ginFixedSuccessResponse(method string) bool {
	switch method {
	case "File", "FileFromFS", "FileAttachment", "SSEvent", "Stream":
		return true
	}
	return false
}

// ginStatusArgumentResponse names the gin.Context methods that answer with the
// status passed as their first argument. Abort is deliberately absent: on its own
// it states no status, so it leaves the path open instead of claiming success.
func ginStatusArgumentResponse(method string) bool {
	switch method {
	case "JSON", "IndentedJSON", "SecureJSON", "PureJSON", "AsciiJSON", "JSONP",
		"XML", "YAML", "TOML", "ProtoBuf", "String", "HTML", "Render", "Negotiate",
		"Data", "DataFromReader", "Redirect", "Status",
		"AbortWithStatus", "AbortWithStatusJSON", "AbortWithError":
		return true
	}
	return false
}

func mergeQueryResponse(state *queryPathState, incoming queryResponseState) {
	if incoming == queryResponseNone || state == nil {
		return
	}
	if state.response == queryResponseNone || state.response == incoming {
		state.response = incoming
		return
	}
	state.response = queryResponseUncertain
}

// requestAccessRequired answers requiredness from the handler's behavior. A
// read is observational by default. It becomes required only when the handler
// proves absence and rejects that branch with a declared 4xx response.
func requestAccessRequired(h handlerDecl, target *ast.CallExpr, method string) bool {
	if h.decl == nil || h.decl.Body == nil || h.info == nil || target == nil {
		return false
	}
	switch method {
	case "GetHeader", "PostForm":
		values := callResultVars(h, target, 0)
		return anyMatchingIf(h, func(ifStmt *ast.IfStmt) bool {
			if !exprChecksMissingStringAccess(h.info, ifStmt.Cond, target, values) {
				return false
			}
			return blockRejectsRequest(h, ifStmt.Body) || blockReturnsNonNilError(h, ifStmt.Body)
		})
	case "Cookie":
		errors := callResultVars(h, target, 1)
		return len(errors) > 0 && anyMatchingIf(h, func(ifStmt *ast.IfStmt) bool {
			return exprChecksNonNil(h.info, ifStmt.Cond, errors) &&
				blockRejectsRequest(h, ifStmt.Body)
		})
	default:
		return false
	}
}

func callResultVars(h handlerDecl, target *ast.CallExpr, resultIndex int) map[gotypes.Object]bool {
	vars := map[gotypes.Object]bool{}
	ast.Inspect(h.decl.Body, func(node ast.Node) bool {
		assign, ok := node.(*ast.AssignStmt)
		if !ok || len(assign.Rhs) != 1 || resultIndex >= len(assign.Lhs) {
			return true
		}
		if !exprContainsCall(assign.Rhs[0], target) {
			return true
		}
		if ident, ok := assign.Lhs[resultIndex].(*ast.Ident); ok && ident.Name != "_" {
			if object := h.info.ObjectOf(ident); object != nil {
				vars[object] = true
			}
		}
		return true
	})
	return vars
}

func anyMatchingIf(h handlerDecl, condition func(*ast.IfStmt) bool) bool {
	required := false
	ast.Inspect(h.decl.Body, func(node ast.Node) bool {
		if required {
			return false
		}
		ifStmt, ok := node.(*ast.IfStmt)
		if !ok || !condition(ifStmt) {
			return true
		}
		required = true
		return false
	})
	return required
}

func blockReturnsNonNilError(h handlerDecl, block *ast.BlockStmt) bool {
	if block == nil || h.info == nil {
		return false
	}
	errorType := gotypes.Universe.Lookup("error").Type()
	returnsError := false
	ast.Inspect(block, func(node ast.Node) bool {
		if returnsError {
			return false
		}
		ret, ok := node.(*ast.ReturnStmt)
		if !ok {
			return true
		}
		for _, result := range ret.Results {
			resultType := h.info.TypeOf(result)
			if resultType != nil && gotypes.AssignableTo(resultType, errorType) && !isNilIdent(result) {
				returnsError = true
				return false
			}
		}
		return true
	})
	return returnsError
}

func exprChecksMissingStringAccess(
	info *gotypes.Info,
	expr ast.Expr,
	target *ast.CallExpr,
	values map[gotypes.Object]bool,
) bool {
	switch node := expr.(type) {
	case *ast.BinaryExpr:
		if node.Op == token.EQL {
			return (exprIsAccessValue(info, node.X, target, values) && isEmptyStringLiteral(node.Y)) ||
				(isEmptyStringLiteral(node.X) && exprIsAccessValue(info, node.Y, target, values)) ||
				(exprIsAccessLength(info, node.X, target, values) && isZeroInteger(node.Y)) ||
				(isZeroInteger(node.X) && exprIsAccessLength(info, node.Y, target, values))
		}
		return false
	case *ast.ParenExpr:
		return exprChecksMissingStringAccess(info, node.X, target, values)
	}
	return false
}

func exprIsAccessValue(
	info *gotypes.Info,
	expr ast.Expr,
	target *ast.CallExpr,
	values map[gotypes.Object]bool,
) bool {
	if call, ok := expr.(*ast.CallExpr); ok {
		if call == target {
			return true
		}
		if selectorName(call.Fun) == "TrimSpace" && len(call.Args) == 1 {
			return exprIsAccessValue(info, call.Args[0], target, values)
		}
	}
	ident, ok := expr.(*ast.Ident)
	return ok && values[info.ObjectOf(ident)]
}

func exprIsAccessLength(
	info *gotypes.Info,
	expr ast.Expr,
	target *ast.CallExpr,
	values map[gotypes.Object]bool,
) bool {
	call, ok := expr.(*ast.CallExpr)
	if !ok || len(call.Args) != 1 {
		return false
	}
	ident, ok := call.Fun.(*ast.Ident)
	return ok && ident.Name == "len" && exprIsAccessValue(info, call.Args[0], target, values)
}

func isZeroInteger(expr ast.Expr) bool {
	literal, ok := expr.(*ast.BasicLit)
	return ok && literal.Kind == token.INT && literal.Value == "0"
}

func exprChecksNonNil(info *gotypes.Info, expr ast.Expr, values map[gotypes.Object]bool) bool {
	binary, ok := expr.(*ast.BinaryExpr)
	if !ok || binary.Op != token.NEQ {
		return false
	}
	return (exprIsObject(info, binary.X, values) && isNilIdent(binary.Y)) ||
		(isNilIdent(binary.X) && exprIsObject(info, binary.Y, values))
}

func exprIsObject(info *gotypes.Info, expr ast.Expr, values map[gotypes.Object]bool) bool {
	ident, ok := expr.(*ast.Ident)
	return ok && values[info.ObjectOf(ident)]
}

func blockRejectsRequest(h handlerDecl, block *ast.BlockStmt) bool {
	if block == nil {
		return false
	}
	rejects := false
	ast.Inspect(block, func(node ast.Node) bool {
		if rejects {
			return false
		}
		call, ok := node.(*ast.CallExpr)
		if !ok || len(call.Args) == 0 {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if !ok || recvPkg != routes.GinPkgPath {
			return true
		}
		switch name {
		case "JSON", "Status", "AbortWithStatus":
			status, known := statusOf(h.info, call.Args[0])
			rejects = known && status >= 400 && status < 500
		}
		return !rejects
	})
	return rejects
}

// requestHeaderGet matches the same call shape as requestHeaderGetInFrame but
// resolves the header name with the handler-scoped resolver, so a name that
// c.GetHeader resolves in a handler body resolves identically through
// c.Request.Header.Get. A helper frame has its own resolver (bindings from the
// call site), which is why the two entry points differ in that one step.
func (a *Analyzer) requestHeaderGet(h handlerDecl, call *ast.CallExpr) (string, bool, bool) {
	if !isRequestHeaderGetCall(helperFrame{decl: h}, call) {
		return "", false, false
	}
	name, resolved := a.callStringArg(h, call, 0)
	return name, true, resolved
}

// requestFormFile matches the same call shape as requestFormFileInFrame but
// resolves the field name with the handler-scoped resolver, so a name that
// c.FormFile resolves in a handler body resolves identically through
// c.Request.FormFile. A helper frame has its own resolver (bindings from the
// call site), which is why the two entry points differ in that one step.
func (a *Analyzer) requestFormFile(h handlerDecl, call *ast.CallExpr) (string, bool, bool) {
	if !isRequestFormFileCall(helperFrame{decl: h}, call) {
		return "", false, false
	}
	name, resolved := a.callStringArg(h, call, 0)
	return name, true, resolved
}

func isRequestHeaderGetCall(frame helperFrame, call *ast.CallExpr) bool {
	if call == nil || frame.decl.info == nil || len(call.Args) == 0 {
		return false
	}
	method, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || method.Sel == nil || method.Sel.Name != "Get" || !isNamedType(frame.decl.info.TypeOf(method.X), "net/http", "Header") {
		return false
	}
	header, ok := method.X.(*ast.SelectorExpr)
	if !ok || header.Sel == nil || header.Sel.Name != "Header" {
		return false
	}
	request, ok := header.X.(*ast.SelectorExpr)
	return ok && request.Sel != nil && request.Sel.Name == "Request" &&
		isGinContextType(frameTypeOf(frame, request.X))
}

func requestHeaderGetInFrame(frame helperFrame, call *ast.CallExpr) (string, bool, bool) {
	if !isRequestHeaderGetCall(frame, call) {
		return "", false, false
	}
	name, resolved := frameCallStringArg(frame, call, 0)
	return name, true, resolved
}

// isRequestFormFileCall recognizes net/http's Request.FormFile only when the
// request is the first-class Request field of a typed Gin context. A local
// *http.Request has the same method but is not evidence about the routed
// request, so receiver provenance is part of the match.
func isRequestFormFileCall(frame helperFrame, call *ast.CallExpr) bool {
	if call == nil || frame.decl.info == nil || len(call.Args) == 0 {
		return false
	}
	method, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || method.Sel == nil || method.Sel.Name != "FormFile" ||
		!isNamedType(frame.decl.info.TypeOf(method.X), "net/http", "Request") {
		return false
	}
	request, ok := method.X.(*ast.SelectorExpr)
	return ok && request.Sel != nil && request.Sel.Name == "Request" &&
		isGinContextType(frameTypeOf(frame, request.X))
}

func requestFormFileInFrame(frame helperFrame, call *ast.CallExpr) (string, bool, bool) {
	if !isRequestFormFileCall(frame, call) {
		return "", false, false
	}
	name, resolved := frameCallStringArg(frame, call, 0)
	return name, true, resolved
}

// Analyze infers the request/response/param facts for one route's handler. The
// route carries the method + normalized path so untyped-query diagnostics can name
// the operation. Unknown handlers (no matching decl) yield empty facts plus an
// ERROR diagnostic, not a panic or a false-complete contract (GO-06). The module prefix used to qualify schema refs is read
// from the Analyzer's per-invocation context (WR-03), not a package global.
// handlerProse reads the handler declaration's own doc comment as plain prose and
// splits it into a summary sentence and the remaining description (CLAUDE.md rule 0.1
// category 2). Only a ROUTED handler ever reaches here, so an internal helper's doc
// comment can never leak into the API surface.
//
// `(*ast.CommentGroup).Text` strips the comment markers AND drops `//go:`-style
// directive lines, so build/generate directives never pollute prose — the text handed
// to the splitter is exactly what a human wrote for humans. Nothing here matches on
// comment CONTENT; that is what keeps this a doc convention rather than a dialect.
func handlerProse(h handlerDecl) (summary, description string) {
	if h.decl == nil || h.decl.Doc == nil {
		return "", ""
	}
	summary, description = docs.Split(h.decl.Doc.Text())
	if h.decl.Name != nil {
		summary = docs.StripSymbolName(summary, h.decl.Name.Name)
	}
	return summary, description
}

func (a *Analyzer) Analyze(route routes.Route, diags *diag.Accumulator) CodeFacts {
	h, ok := a.handlerForRoute(route)
	if !ok || h.decl == nil || h.decl.Body == nil {
		if diags != nil {
			diags.UnknownHandler(
				route.Handler,
				route.Method,
				route.Path,
				route.Span.File,
				route.Span.StartLine,
			)
		}
		return CodeFacts{RequestBodyRequired: true, Responses: []facts.ResponseFact{}, Params: []facts.ParamFact{}}
	}

	cf := CodeFacts{RequestBodyRequired: true, Responses: []facts.ResponseFact{}, Params: []facts.ParamFact{}}
	cf.Summary, cf.Description = handlerProse(h)
	seenParam := map[string]bool{}
	resolvedParam := map[string]bool{}
	untypedQueryReads := []untypedQueryRead{}
	seenStatus := map[uint16]bool{}
	provisionalStatus := map[uint16]bool{}
	formFields := map[string]facts.FieldFact{}
	boundFormRefs := map[string]bool{}
	manualFormFields := map[string]bool{}
	formHasFile := false
	for name, field := range a.multipartFileMapFields(h, route, diags) {
		formFields[name] = field
		manualFormFields[name] = true
		formHasFile = true
	}
	contentTypeHint := ""
	optionalBindPositions := collectOptionalBindPositions(h)
	hasBodyBind := false
	allBodyBindsOptional := true
	delegatedResponseSeen := map[string]bool{}
	responseHeaders := a.collectResponseHeadersByStatus(h, route, diags)

	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if !ok || recvPkg != routes.GinPkgPath {
			if isHTTPRedirectCall(h.info, call) {
				a.analyzeRedirect(h, call, 3, route, &cf, seenStatus, provisionalStatus, diags)
			}
			if pname, matched, resolved := a.requestHeaderGet(h, call); matched {
				if resolved {
					a.addExtractedParameter(
						&cf,
						seenParam,
						resolvedParam,
						requestParameter(pname, "header", requestAccessRequired(h, call, "GetHeader"), facts.PrimitiveType(facts.StringPrim()), h.fset, call.Pos()),
						true,
						route,
						diags,
					)
				} else {
					reportDirectDynamicParameter(diags, h, route, call, "Request.Header.Get")
				}
			}
			if fname, matched, resolved := a.requestFormFile(h, call); matched {
				if resolved {
					addMultipartFileField(formFields, manualFormFields, &formHasFile, fname)
				} else {
					reportDirectUnresolvedBody(diags, h, route, call, "Request.FormFile", "multipart field name is dynamic")
				}
			}
			if ref, schema, ok := a.bodyFromGenericJSONHelper(h, call); ok {
				a.setRequestBodyFact(&cf, ref, "application/json", route, diags, h.fset, call.Pos(), selectorName(call.Fun))
				cf.Schemas = append(cf.Schemas, schema...)
				hasBodyBind = true
				allBodyBindsOptional = false
			}
			a.analyzeContextHelperCall(
				helperFrame{decl: h, bindings: map[gotypes.Object]helperBinding{}},
				call,
				parameterHint{},
				0,
				&contextTraversal{
					route:                  route,
					cf:                     &cf,
					seenParam:              seenParam,
					resolvedParam:          resolvedParam,
					untypedQueryReads:      &untypedQueryReads,
					formFields:             formFields,
					boundFormRefs:          boundFormRefs,
					manualFormFields:       manualFormFields,
					formHasFile:            &formHasFile,
					hasBodyBind:            &hasBodyBind,
					allBodyBindsOptional:   &allBodyBindsOptional,
					diagnostics:            diags,
					stack:                  map[string]bool{},
					reportedUnresolvedCall: map[token.Pos]bool{},
				},
			)
			a.analyzeDelegatedResponses(helperFrame{decl: h, bindings: map[gotypes.Object]helperBinding{}}, call, route, &cf, seenStatus, provisionalStatus, diags, delegatedResponseSeen, contentTypeHint)
			a.analyzePathHelperCall(h, call, &cf, seenParam)
			a.analyzeQueryHelperCall(h, call, route, &cf, seenParam, resolvedParam, diags)
			return true
		}
		switch name {
		case "ShouldBindJSON", "BindJSON":
			if ref, _, ok := a.bindRequestType(h.info, call); ok {
				a.setRequestBodyFact(&cf, ref, "application/json", route, diags, h.fset, call.Pos(), name)
				hasBodyBind = true
				if !optionalBindPositions[call.Pos()] {
					allBodyBindsOptional = false
				}
			} else {
				reportDirectUnresolvedBody(diags, h, route, call, name, "binding target does not resolve to a named schema")
			}
		case "ShouldBindQuery", "BindQuery":
			a.addBoundParameters(helperFrame{decl: h}, call, "query", &cf, seenParam, resolvedParam, route, diags)
		case "ShouldBindHeader", "BindHeader":
			a.addBoundParameters(helperFrame{decl: h}, call, "header", &cf, seenParam, resolvedParam, route, diags)
		case "ShouldBind", "Bind", "ShouldBindWith", "BindWith":
			frame := helperFrame{decl: h}
			bound := boundTypeFromCall(frame, call)
			contentType := bindContentType(name, h.info, call, bound)
			if isFormContentType(contentType) {
				refID, ok := a.addBoundFormFields(frame, call, formFields, route, diags)
				if !ok {
					reportDirectUnresolvedBody(diags, h, route, call, name, "binding target does not resolve to a form object")
					break
				}
				if refID != "" {
					boundFormRefs[refID] = true
				}
				formHasFile = formHasFile || contentType == "multipart/form-data"
				hasBodyBind = true
				if !optionalBindPositions[call.Pos()] {
					allBodyBindsOptional = false
				}
			} else if ref, _, ok := a.bindRequestType(h.info, call); ok {
				if contentType == "" {
					reportDirectUnresolvedBody(diags, h, route, call, name, "binding media type is selected dynamically or is unsupported")
				}
				a.setRequestBodyFact(&cf, ref, contentType, route, diags, h.fset, call.Pos(), name)
				hasBodyBind = true
				if !optionalBindPositions[call.Pos()] {
					allBodyBindsOptional = false
				}
			} else {
				reportDirectUnresolvedBody(diags, h, route, call, name, "binding target does not resolve to a named schema")
			}
		case "JSON":
			a.analyzeJSON(h, call, route, &cf, seenStatus, provisionalStatus, diags)
		case "Status":
			a.analyzeStatus(h, call, route, &cf, seenStatus, provisionalStatus, true, diags)
		case "AbortWithStatus":
			a.analyzeStatus(h, call, route, &cf, seenStatus, provisionalStatus, false, diags)
		case "Header":
			if key, ok := a.callStringArg(h, call, 0); ok && strings.EqualFold(key, "Content-Type") {
				if value, ok := a.stringValueOf(h, call.Args[1]); ok {
					contentTypeHint = value
				}
			}
		case "File", "FileAttachment":
			a.analyzeBinaryStatus(&cf, seenStatus, provisionalStatus, 200, contentTypeHint)
		case "Data":
			a.analyzeData(h, call, route, &cf, seenStatus, provisionalStatus, diags)
		case "DataFromReader":
			a.analyzeDataFromReader(h, call, route, &cf, seenStatus, provisionalStatus, diags)
		case "Redirect":
			a.analyzeRedirect(h, call, 0, route, &cf, seenStatus, provisionalStatus, diags)
		case "SSEvent":
			a.addSSEResponse(&cf, seenStatus, provisionalStatus)
		case "Stream":
			if streamCallContainsSSEvent(h.info, call) {
				a.addSSEResponse(&cf, seenStatus, provisionalStatus)
			}
		case "Param":
			if pname, ok := a.callStringArg(h, call, 0); ok {
				a.addExtractedParameter(&cf, seenParam, resolvedParam, pathParam(pname, h.fset, call.Pos()), true, route, diags)
			} else {
				reportDirectDynamicParameter(diags, h, route, call, name)
			}
		case "Query", "DefaultQuery", "GetQuery", "QueryArray", "GetQueryArray", "QueryMap":
			if pname, ok := a.callStringArg(h, call, 0); ok {
				file, line := positionOf(h.fset, call.Pos())
				resolved := name != "Query"
				hint := parameterHint{}
				if name == "Query" {
					switch queryRequiredness(helperFrame{decl: h}, pname) {
					case queryPresenceOptional:
						resolved = true
						hint.requiredKnown = true
						hint.required = false
					case queryPresenceRequired:
						resolved = true
						hint.requiredKnown = true
						hint.required = true
					}
				}
				a.addExtractedParameter(
					&cf,
					seenParam,
					resolvedParam,
					parameterFromGinAccess(h.info, name, pname, h.fset, call, hint),
					resolved,
					route,
					diags,
				)
				if name == "Query" && !resolved {
					untypedQueryReads = append(untypedQueryReads, untypedQueryRead{name: pname, file: file, line: line})
				}
			} else {
				reportDirectDynamicParameter(diags, h, route, call, name)
			}
		case "GetHeader":
			if pname, ok := a.callStringArg(h, call, 0); ok {
				a.addExtractedParameter(&cf, seenParam, resolvedParam, requestParameter(
					pname,
					"header",
					requestAccessRequired(h, call, name),
					facts.PrimitiveType(facts.StringPrim()),
					h.fset,
					call.Pos(),
				), true, route, diags)
			} else {
				reportDirectDynamicParameter(diags, h, route, call, name)
			}
		case "Cookie":
			if pname, ok := a.callStringArg(h, call, 0); ok {
				a.addExtractedParameter(&cf, seenParam, resolvedParam, requestParameter(
					pname,
					"cookie",
					requestAccessRequired(h, call, name),
					facts.PrimitiveType(facts.StringPrim()),
					h.fset,
					call.Pos(),
				), true, route, diags)
			} else {
				reportDirectDynamicParameter(diags, h, route, call, name)
			}
		case "FormFile":
			if fname, ok := a.callStringArg(h, call, 0); ok {
				addMultipartFileField(formFields, manualFormFields, &formHasFile, fname)
			} else {
				reportDirectUnresolvedBody(diags, h, route, call, name, "multipart field name is dynamic")
			}
		case "PostForm", "DefaultPostForm", "GetPostForm":
			if fname, ok := a.callStringArg(h, call, 0); ok {
				manualFormFields[fname] = true
				if _, seen := formFields[fname]; !seen {
					field := formField(
						fname,
						facts.PrimitiveType(facts.StringPrim()),
						requestAccessRequired(h, call, name),
					)
					if name == "DefaultPostForm" && len(call.Args) > 1 {
						field.Meta = &facts.FieldMeta{Default: literalValue(h.info, call.Args[1])}
					}
					formFields[fname] = field
				}
			} else {
				reportDirectUnresolvedBody(diags, h, route, call, name, "form field name is dynamic")
			}
		case "MultipartForm":
			// Literal accesses through the returned multipart.Form are collected in one
			// typed pre-pass. The call alone does not state a field name.
		}
		return true
	})
	if len(formFields) > 0 || len(boundFormRefs) > 0 {
		var formBody *facts.TypeRef
		formContentType := "application/x-www-form-urlencoded"
		if formHasFile {
			formContentType = "multipart/form-data"
		}
		if len(boundFormRefs) == 1 && len(manualFormFields) == 0 {
			for refID := range boundFormRefs {
				formBody = &facts.TypeRef{RefID: refID}
			}
		} else {
			schemaID, schemaName := syntheticFormSchemaIdentity(route.Handler)
			fields := make([]facts.FieldFact, 0, len(formFields))
			for _, field := range formFields {
				fields = append(fields, field)
			}
			formBody = &facts.TypeRef{RefID: schemaID}
			cf.Schemas = append(cf.Schemas, facts.SchemaFact{
				ID:   schemaID,
				Name: schemaName,
				Body: facts.ObjectType(fields),
				Span: spanOf(h.fset, declPos(h.decl)),
			})
		}
		if formBody != nil {
			a.setRequestBodyFact(&cf, formBody, formContentType, route, diags, h.fset, declPos(h.decl), "form fields")
		}
	}
	if cf.RequestBody == nil {
		if schema := a.rawJSONRequestSchema(h, route.Handler); schema != nil {
			cf.RequestBody = &facts.TypeRef{RefID: schema.ID}
			cf.RequestBodyContentType = "application/json"
			cf.Schemas = append(cf.Schemas, *schema)
		}
	}
	if hasBodyBind {
		cf.RequestBodyRequired = !allBodyBindsOptional
	}
	if diags != nil {
		reported := map[string]bool{}
		for _, read := range untypedQueryReads {
			key := "query/" + read.name
			if resolvedParam[key] || reported[key] {
				continue
			}
			reported[key] = true
			diags.UntypedQueryParam(
				read.name,
				route.Method,
				untypedRouteLabel(route),
				read.file,
				read.line,
			)
		}
	}
	if len(cf.Responses) == 0 && diags != nil {
		diags.MissingResponses(
			route.Handler,
			route.Method,
			route.Path,
			route.Span.File,
			route.Span.StartLine,
		)
	}
	for index := range cf.Responses {
		cf.Responses[index].Headers = append(
			[]facts.ResponseHeaderFact(nil),
			responseHeaders[cf.Responses[index].Status]...,
		)
	}
	return cf
}

func reportDirectDynamicParameter(
	diags *diag.Accumulator,
	h handlerDecl,
	route routes.Route,
	call *ast.CallExpr,
	subject string,
) {
	if diags == nil {
		return
	}
	file, line := positionOf(h.fset, call.Pos())
	diags.RequestParameterUnresolved(
		subject,
		route.Method,
		untypedRouteLabel(route),
		"parameter name is dynamic",
		file,
		line,
	)
}

func reportDirectUnresolvedBody(
	diags *diag.Accumulator,
	h handlerDecl,
	route routes.Route,
	call *ast.CallExpr,
	subject string,
	reason string,
) {
	if diags == nil {
		return
	}
	file, line := positionOf(h.fset, call.Pos())
	diags.RequestBodyUnresolved(subject, route.Method, untypedRouteLabel(route), reason, file, line)
}

func (a *Analyzer) setRequestBodyFact(
	cf *CodeFacts,
	ref *facts.TypeRef,
	contentType string,
	route routes.Route,
	diags *diag.Accumulator,
	fset *token.FileSet,
	pos token.Pos,
	subject string,
) {
	if cf == nil || ref == nil {
		return
	}
	if cf.RequestBody == nil {
		cf.RequestBody = ref
		cf.RequestBodyContentType = contentType
		return
	}
	if cf.RequestBodyContentType != "" && contentType != "" && cf.RequestBodyContentType != contentType {
		for _, variant := range cf.RequestBodyVariants {
			if variant.ContentType != contentType {
				continue
			}
			if variant.Body.RefID != ref.RefID && diags != nil {
				file, line := positionOf(fset, pos)
				diags.RequestBodyUnresolved(
					subject,
					route.Method,
					untypedRouteLabel(route),
					"conflicting body evidence for "+contentType+" ("+variant.Body.RefID+" versus "+ref.RefID+")",
					file,
					line,
				)
			}
			return
		}
		cf.RequestBodyVariants = append(cf.RequestBodyVariants, facts.RequestBodyVariantFact{
			Body:        *ref,
			ContentType: contentType,
		})
		return
	}
	if cf.RequestBody.RefID != ref.RefID {
		if diags != nil {
			file, line := positionOf(fset, pos)
			diags.RequestBodyUnresolved(
				subject,
				route.Method,
				untypedRouteLabel(route),
				"conflicting body evidence for "+contentType+" ("+cf.RequestBody.RefID+" versus "+ref.RefID+")",
				file,
				line,
			)
		}
		return
	}
	if cf.RequestBodyContentType == "" {
		cf.RequestBodyContentType = contentType
	}
}

func (a *Analyzer) handlerForRoute(route routes.Route) (handlerDecl, bool) {
	if route.HandlerKey != "" {
		h, ok := a.declsByObject[route.HandlerKey]
		return h, ok
	}
	h, ok := a.idx[route.Handler]
	return h, ok
}

func collectOptionalBindPositions(h handlerDecl) map[token.Pos]bool {
	out := map[token.Pos]bool{}
	if h.decl == nil || h.decl.Body == nil {
		return out
	}
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		ifStmt, ok := n.(*ast.IfStmt)
		if !ok || !isOptionalBodyGuard(h.info, ifStmt.Cond) {
			return true
		}
		ast.Inspect(ifStmt.Body, func(child ast.Node) bool {
			call, ok := child.(*ast.CallExpr)
			if !ok || !isGinBindCall(h.info, call) {
				return true
			}
			out[call.Pos()] = true
			return true
		})
		return true
	})
	return out
}

func isOptionalBodyGuard(info *gotypes.Info, expr ast.Expr) bool {
	return isBodyPresentPredicate(info, expr)
}

func isBodyPresentPredicate(info *gotypes.Info, expr ast.Expr) bool {
	switch node := expr.(type) {
	case *ast.BinaryExpr:
		switch node.Op {
		case token.LOR:
			return isBodyPresentPredicate(info, node.X) && isBodyPresentPredicate(info, node.Y)
		case token.LAND:
			return isBodyPresentPredicate(info, node.X) || isBodyPresentPredicate(info, node.Y)
		case token.GTR:
			return isContentLengthExpr(info, node.X) && isZeroLiteral(node.Y)
		case token.LSS:
			return isZeroLiteral(node.X) && isContentLengthExpr(info, node.Y)
		case token.NEQ:
			return (isContentLengthExpr(info, node.X) && isZeroLiteral(node.Y)) ||
				(isZeroLiteral(node.X) && isContentLengthExpr(info, node.Y)) ||
				(isContentLengthHeaderCall(info, node.X) && isEmptyStringLiteral(node.Y)) ||
				(isEmptyStringLiteral(node.X) && isContentLengthHeaderCall(info, node.Y))
		}
	case *ast.ParenExpr:
		return isBodyPresentPredicate(info, node.X)
	}
	return false
}

func isContentLengthExpr(info *gotypes.Info, expr ast.Expr) bool {
	switch node := expr.(type) {
	case *ast.SelectorExpr:
		return isGinRequestContentLength(info, node)
	case *ast.CallExpr:
		if selectorName(node.Fun) == "len" && len(node.Args) == 1 {
			return isContentLengthHeaderValueExpr(info, node.Args[0])
		}
	}
	return false
}

func isGinRequestContentLength(info *gotypes.Info, selector *ast.SelectorExpr) bool {
	if info == nil {
		return false
	}
	if selector == nil || selector.Sel == nil || selector.Sel.Name != "ContentLength" {
		return false
	}
	requestSelector, ok := selector.X.(*ast.SelectorExpr)
	if !ok || requestSelector.Sel == nil || requestSelector.Sel.Name != "Request" {
		return false
	}
	return isGinContextType(info.TypeOf(requestSelector.X)) &&
		isNamedType(info.TypeOf(requestSelector), "net/http", "Request")
}

func mentionsContentLengthHeader(info *gotypes.Info, expr ast.Expr) bool {
	if info == nil {
		return false
	}
	found := false
	ast.Inspect(expr, func(n ast.Node) bool {
		call, ok := n.(*ast.CallExpr)
		if !ok || !isContentLengthHeaderCall(info, call) {
			return true
		}
		found = true
		return false
	})
	return found
}

func isContentLengthHeaderCall(info *gotypes.Info, expr ast.Expr) bool {
	if info == nil {
		return false
	}
	call, ok := expr.(*ast.CallExpr)
	if !ok {
		return false
	}
	name, recvPkg, ok := routes.GinMethod(info, call)
	if !ok || recvPkg != routes.GinPkgPath || name != "GetHeader" {
		return false
	}
	value, ok := stringArg(call, 0)
	return ok && strings.EqualFold(value, "Content-Length")
}

func isContentLengthHeaderValueExpr(info *gotypes.Info, expr ast.Expr) bool {
	if isContentLengthHeaderCall(info, expr) {
		return true
	}
	call, ok := expr.(*ast.CallExpr)
	if !ok || len(call.Args) != 1 {
		return false
	}
	fn := calledFuncObject(info, call.Fun)
	return fn != nil && fn.Pkg() != nil && fn.Pkg().Path() == "strings" && fn.Name() == "TrimSpace" &&
		isContentLengthHeaderValueExpr(info, call.Args[0])
}

func isGinContextType(t gotypes.Type) bool {
	return isNamedType(t, routes.GinPkgPath, "Context")
}

func isNamedType(t gotypes.Type, pkgPath, name string) bool {
	if t == nil {
		return false
	}
	if ptr, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = ptr.Elem()
	}
	named, ok := gotypes.Unalias(t).(*gotypes.Named)
	if !ok || named.Obj() == nil || named.Obj().Pkg() == nil {
		return false
	}
	return named.Obj().Pkg().Path() == pkgPath && named.Obj().Name() == name
}

func isZeroLiteral(expr ast.Expr) bool {
	lit, ok := expr.(*ast.BasicLit)
	return ok && lit.Kind == token.INT && lit.Value == "0"
}

func isEmptyStringLiteral(expr ast.Expr) bool {
	lit, ok := expr.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return false
	}
	value, err := strconv.Unquote(lit.Value)
	return err == nil && value == ""
}

func isGinBindCall(info *gotypes.Info, call *ast.CallExpr) bool {
	name, recvPkg, ok := routes.GinMethod(info, call)
	if !ok || recvPkg != routes.GinPkgPath {
		return false
	}
	switch name {
	case "ShouldBindJSON", "BindJSON", "ShouldBind", "Bind", "ShouldBindWith", "BindWith":
		return true
	default:
		return false
	}
}

func (a *Analyzer) analyzeQueryHelperCall(
	h handlerDecl,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenParam map[string]bool,
	resolvedParam map[string]bool,
	diags *diag.Accumulator,
) {
	// Both handoffs below go through addExtractedParameter rather than bailing when
	// the parameter is already resolved: a helper read still states the schema the
	// raw read cannot, and skipping it made the emitted type depend on whether the
	// guard or the parser came first in the handler.
	if param, ok := a.queryParamFromModuleHelper(h, call); ok {
		a.addExtractedParameter(cf, seenParam, resolvedParam, param, true, route, diags)
		return
	}
	query, ok := firstQueryCall(h.info, call)
	if !ok || query == call {
		return
	}
	pname, ok := a.callStringArg(h, query, 0)
	if !ok {
		return
	}
	if nestedQueryHelperOutranks(h.info, call, query) {
		return
	}
	param, requiredKnown, ok := a.queryParamFromHelper(h, call, query, pname)
	if !ok {
		return
	}
	a.addExtractedParameter(cf, seenParam, resolvedParam, param, requiredKnown, route, diags)
}

func (a *Analyzer) queryParamFromHelper(
	h handlerDecl,
	helper *ast.CallExpr,
	query *ast.CallExpr,
	name string,
) (facts.ParamFact, bool, bool) {
	schema, ok := queryHelperSchema(h.info.TypeOf(helper), helper)
	if !ok {
		return facts.ParamFact{}, false, false
	}
	required := false
	requiredKnown := false
	if fn := calledFuncObject(h.info, helper.Fun); fn != nil {
		if callee, ok := a.moduleOwnedCallee(fn); ok {
			required = queryHelperRequiredFromDirectCallee(h.info, helper, callee, query)
			requiredKnown = true
		}
	}
	return facts.ParamFact{
		Name:     name,
		Location: "query",
		Required: required,
		Schema:   schema,
		Default:  firstLiteral(queryDefaultValue(h.info, query), queryHelperDefault(h.info, helper, query)),
		Span:     spanOf(h.fset, query.Pos()),
	}, requiredKnown, true
}

func (a *Analyzer) queryParamFromModuleHelper(h handlerDecl, call *ast.CallExpr) (facts.ParamFact, bool) {
	if !callPassesGinContext(h.info, call) {
		return facts.ParamFact{}, false
	}
	fn := calledFuncObject(h.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		return facts.ParamFact{}, false
	}
	sig, ok := gotypes.Unalias(h.info.TypeOf(call.Fun)).(*gotypes.Signature)
	if !ok || sig.Params() == nil {
		return facts.ParamFact{}, false
	}
	schema, ok := queryHelperSchema(h.info.TypeOf(call), call)
	if !ok {
		return facts.ParamFact{}, false
	}
	for i := 0; i < len(call.Args) && i < sig.Params().Len(); i++ {
		pname, ok := a.stringValueOf(h, call.Args[i])
		if !ok {
			continue
		}
		query, ok := helperQueryCallWithVar(callee, sig.Params().At(i))
		if !ok {
			continue
		}
		return facts.ParamFact{
			Name:     pname,
			Location: "query",
			Required: queryHelperRequiredFromCallee(h.info, call, callee, query),
			Schema:   schema,
			Default:  firstLiteral(queryDefaultValue(callee.info, query), queryHelperDefaultFromArgs(h.info, call, sig)),
			Span:     spanOf(h.fset, call.Pos()),
		}, true
	}
	return facts.ParamFact{}, false
}

func helperQueryCallWithVar(h handlerDecl, keyVar *gotypes.Var) (*ast.CallExpr, bool) {
	if h.decl == nil || h.decl.Body == nil || keyVar == nil {
		return nil, false
	}
	var found *ast.CallExpr
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found != nil {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if !ok || recvPkg != routes.GinPkgPath || !isGinQueryMethod(name) || len(call.Args) == 0 {
			return true
		}
		id, ok := call.Args[0].(*ast.Ident)
		if ok && h.info.ObjectOf(id) == keyVar {
			found = call
			return false
		}
		return true
	})
	return found, found != nil
}

func (a *Analyzer) analyzePathHelperCall(
	h handlerDecl,
	call *ast.CallExpr,
	cf *CodeFacts,
	seenParam map[string]bool,
) {
	pname, schema, ok := a.pathParamFromHelper(h, call)
	if !ok || seenParam["path/"+pname] {
		return
	}
	seenParam["path/"+pname] = true
	cf.Params = append(cf.Params, facts.ParamFact{
		Name:     pname,
		Location: "path",
		Required: true,
		Schema:   schema,
		Span:     spanOf(h.fset, call.Pos()),
	})
}

func (a *Analyzer) pathParamFromHelper(
	h handlerDecl,
	call *ast.CallExpr,
) (string, facts.Type, bool) {
	fn := calledFuncObject(h.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		return "", facts.Type{}, false
	}
	sig, ok := gotypes.Unalias(h.info.TypeOf(call.Fun)).(*gotypes.Signature)
	if !ok || sig.Params() == nil {
		return "", facts.Type{}, false
	}
	schema, ok := queryHelperSchema(h.info.TypeOf(call), call)
	if !ok {
		return "", facts.Type{}, false
	}
	for i := 0; i < len(call.Args) && i < sig.Params().Len(); i++ {
		name, ok := stringArg(call, i)
		if !ok {
			continue
		}
		if helperReadsGinParamWithVar(callee, sig.Params().At(i)) {
			return name, schema, true
		}
	}
	return "", facts.Type{}, false
}

func helperReadsGinParamWithVar(h handlerDecl, keyVar *gotypes.Var) bool {
	if h.decl == nil || h.decl.Body == nil || keyVar == nil {
		return false
	}
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if !ok || recvPkg != routes.GinPkgPath || name != "Param" || len(call.Args) == 0 {
			return true
		}
		id, ok := call.Args[0].(*ast.Ident)
		if ok && h.info.ObjectOf(id) == keyVar {
			found = true
			return false
		}
		return true
	})
	return found
}

func (a *Analyzer) analyzeDelegatedResponses(
	caller helperFrame,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
	seenHelpers map[string]bool,
	inheritedContentTypeHint string,
) {
	if !frameCallPassesGinContext(caller, call) {
		return
	}
	fn := calledFuncObject(caller.decl.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		return
	}
	key := caller.decl.identityKey() + "@" + strconv.FormatInt(int64(call.Pos()), 10) + "->" + callee.identityKey()
	if seenHelpers[key] {
		return
	}
	seenHelpers[key] = true
	frame := helperFrame{decl: callee, bindings: helperCallBindings(caller, call, fn)}

	contentTypeHint := inheritedContentTypeHint
	ast.Inspect(callee.decl.Body, func(n ast.Node) bool {
		nested, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(callee.info, nested)
		if !ok || recvPkg != routes.GinPkgPath {
			if isHTTPRedirectCall(callee.info, nested) {
				a.analyzeRedirectInFrame(frame, nested, 3, route, cf, seenStatus, provisionalStatus, diags)
			}
			a.analyzeDelegatedResponses(frame, nested, route, cf, seenStatus, provisionalStatus, diags, seenHelpers, contentTypeHint)
			return true
		}
		switch name {
		case "JSON":
			a.analyzeJSON(callee, nested, route, cf, seenStatus, provisionalStatus, diags)
		case "Status":
			a.analyzeStatus(callee, nested, route, cf, seenStatus, provisionalStatus, true, diags)
		case "AbortWithStatus":
			a.analyzeStatus(callee, nested, route, cf, seenStatus, provisionalStatus, false, diags)
		case "Header":
			if key, ok := stringArg(nested, 0); ok && strings.EqualFold(key, "Content-Type") {
				if value, ok := frameStringValue(frame, nested.Args[1]); ok {
					contentTypeHint = value
				}
			}
		case "File", "FileAttachment":
			a.analyzeBinaryStatus(cf, seenStatus, provisionalStatus, 200, contentTypeHint)
		case "Data":
			a.analyzeData(callee, nested, route, cf, seenStatus, provisionalStatus, diags)
		case "DataFromReader":
			a.analyzeDataFromReader(callee, nested, route, cf, seenStatus, provisionalStatus, diags)
		case "Redirect":
			a.analyzeRedirectInFrame(frame, nested, 0, route, cf, seenStatus, provisionalStatus, diags)
		case "SSEvent":
			a.addSSEResponse(cf, seenStatus, provisionalStatus)
		case "Stream":
			if streamCallContainsSSEvent(callee.info, nested) {
				a.addSSEResponse(cf, seenStatus, provisionalStatus)
			}
		}
		return true
	})
}

func (a *Analyzer) analyzeDelegatedRequestBodies(
	h handlerDecl,
	call *ast.CallExpr,
	cf *CodeFacts,
	seenHelpers map[string]bool,
	hasBodyBind *bool,
	allBodyBindsOptional *bool,
) {
	if callUsesTypeArgs(call) {
		return
	}
	callee, ok := a.delegatedGinContextHelper(h, call)
	if !ok {
		return
	}
	key := callee.identityKey()
	if seenHelpers[key] {
		return
	}
	seenHelpers[key] = true

	optionalBindPositions := collectOptionalBindPositions(callee)
	ast.Inspect(callee.decl.Body, func(n ast.Node) bool {
		nested, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(callee.info, nested)
		if !ok || recvPkg != routes.GinPkgPath {
			a.analyzeDelegatedRequestBodies(callee, nested, cf, seenHelpers, hasBodyBind, allBodyBindsOptional)
			return true
		}
		switch name {
		case "ShouldBindJSON", "BindJSON":
			if ref, _, ok := a.bindRequestType(callee.info, nested); ok {
				cf.RequestBody = ref
				cf.RequestBodyContentType = "application/json"
				*hasBodyBind = true
				if !optionalBindPositions[nested.Pos()] {
					*allBodyBindsOptional = false
				}
			}
		case "ShouldBind", "Bind", "ShouldBindWith", "BindWith":
			if ref, bound, ok := a.bindRequestType(callee.info, nested); ok {
				cf.RequestBody = ref
				cf.RequestBodyContentType = bindContentType(name, callee.info, nested, bound)
				*hasBodyBind = true
				if !optionalBindPositions[nested.Pos()] {
					*allBodyBindsOptional = false
				}
			}
		}
		return true
	})
}

func callUsesTypeArgs(call *ast.CallExpr) bool {
	if call == nil {
		return false
	}
	switch call.Fun.(type) {
	case *ast.IndexExpr, *ast.IndexListExpr:
		return true
	default:
		return false
	}
}

func (a *Analyzer) delegatedGinContextHelper(h handlerDecl, call *ast.CallExpr) (handlerDecl, bool) {
	if !callPassesGinContext(h.info, call) {
		return handlerDecl{}, false
	}
	fn := calledFuncObject(h.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		return handlerDecl{}, false
	}
	return callee, true
}

func (a *Analyzer) bodyFromGenericJSONHelper(
	h handlerDecl,
	call *ast.CallExpr,
) (*facts.TypeRef, []facts.SchemaFact, bool) {
	if !callPassesGinContext(h.info, call) {
		return nil, nil, false
	}
	fn := calledFuncObject(h.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok || !helperBindsJSONTypeParam(callee) {
		return nil, nil, false
	}
	typeArgs := callTypeArgs(h.info, call)
	if len(typeArgs) != 1 {
		return nil, nil, false
	}
	id, ok := a.namedTypeID(typeArgs[0])
	if !ok {
		return nil, nil, false
	}
	return &facts.TypeRef{RefID: id}, nil, true
}

func helperBindsJSONTypeParam(h handlerDecl) bool {
	if h.decl == nil || h.decl.Body == nil {
		return false
	}
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(h.info, call)
		if ok && recvPkg == routes.GinPkgPath && (name == "ShouldBindJSON" || name == "BindJSON") && bindArgIsTypeParam(h.info, call) {
			found = true
			return false
		}
		return true
	})
	return found
}

func bindArgIsTypeParam(info *gotypes.Info, call *ast.CallExpr) bool {
	if info == nil || call == nil || len(call.Args) == 0 {
		return false
	}
	unary, ok := call.Args[0].(*ast.UnaryExpr)
	if !ok || unary.Op != token.AND {
		return false
	}
	_, ok = gotypes.Unalias(info.TypeOf(unary.X)).(*gotypes.TypeParam)
	return ok
}

func callTypeArgs(info *gotypes.Info, call *ast.CallExpr) []gotypes.Type {
	if info == nil || call == nil {
		return nil
	}
	var exprs []ast.Expr
	switch fun := call.Fun.(type) {
	case *ast.IndexExpr:
		exprs = []ast.Expr{fun.Index}
	case *ast.IndexListExpr:
		exprs = fun.Indices
	default:
		return nil
	}
	out := make([]gotypes.Type, 0, len(exprs))
	for _, expr := range exprs {
		t := info.TypeOf(expr)
		if t == nil {
			return nil
		}
		out = append(out, t)
	}
	return out
}

func callPassesGinContext(info *gotypes.Info, call *ast.CallExpr) bool {
	for _, arg := range call.Args {
		if isGinContextType(info.TypeOf(arg)) {
			return true
		}
	}
	return false
}

func nestedQueryHelperOutranks(info *gotypes.Info, helper *ast.CallExpr, query *ast.CallExpr) bool {
	found := false
	currentPriority := queryHelperPriority(info, helper, query)
	ast.Inspect(helper, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok || call == helper || call == query {
			return true
		}
		if nestedQuery, ok := firstQueryCall(info, call); ok && nestedQuery == query {
			if name, recvPkg, ok := routes.GinMethod(info, call); ok && recvPkg == routes.GinPkgPath && isGinQueryMethod(name) {
				return true
			}
			if queryHelperPriority(info, call, query) > currentPriority {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

func queryHelperPriority(info *gotypes.Info, helper *ast.CallExpr, query *ast.CallExpr) int {
	if info == nil || helper == nil {
		return 0
	}
	schema, ok := queryHelperSchema(info.TypeOf(helper), helper)
	if !ok {
		return 0
	}
	if !isPrimitiveStringType(schema) {
		return 4
	}
	if queryDefaultValue(info, query) != nil || queryHelperDefault(info, helper, query) != nil || helperReturnsError(info, helper) {
		return 3
	}
	name := strings.ToLower(selectorName(helper.Fun))
	if name != "trimspace" && (strings.Contains(name, "optional") || strings.Contains(name, "required")) {
		return 2
	}
	return 1
}

func isPrimitiveStringType(schema facts.Type) bool {
	if schema.Type != facts.TypePrimitive {
		return false
	}
	prim, ok := schema.Of.(*facts.Prim)
	return ok && prim.Prim == facts.PrimString
}

func helperReturnsError(info *gotypes.Info, helper *ast.CallExpr) bool {
	if info == nil || helper == nil {
		return false
	}
	t := info.TypeOf(helper)
	if tuple, ok := gotypes.Unalias(t).(*gotypes.Tuple); ok && tuple.Len() > 1 {
		last := tuple.At(tuple.Len() - 1)
		return last != nil && isErrorType(last.Type())
	}
	return false
}

func firstQueryCall(info *gotypes.Info, root ast.Expr) (*ast.CallExpr, bool) {
	var found *ast.CallExpr
	ast.Inspect(root, func(n ast.Node) bool {
		if found != nil {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(info, call)
		if ok && recvPkg == routes.GinPkgPath && isGinQueryMethod(name) {
			found = call
			return false
		}
		return true
	})
	return found, found != nil
}

func isGinQueryMethod(name string) bool {
	switch name {
	case "Query", "DefaultQuery", "GetQuery", "QueryArray", "GetQueryArray", "QueryMap":
		return true
	default:
		return false
	}
}

func queryDefaultValue(info *gotypes.Info, query *ast.CallExpr) *facts.LiteralValue {
	if query == nil || selectorName(query.Fun) != "DefaultQuery" || len(query.Args) < 2 {
		return nil
	}
	return literalValue(info, query.Args[1])
}

func queryHelperSchema(t gotypes.Type, helper *ast.CallExpr) (facts.Type, bool) {
	if t == nil {
		if selectorName(helper.Fun) == "TrimSpace" {
			return facts.PrimitiveType(facts.StringPrim()), true
		}
		return facts.Type{}, false
	}
	if tuple, ok := gotypes.Unalias(t).(*gotypes.Tuple); ok && tuple.Len() > 0 {
		t = tuple.At(0).Type()
	}
	if ptr, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = ptr.Elem()
	}
	if slice, ok := gotypes.Unalias(t).(*gotypes.Slice); ok {
		elem, ok := queryHelperSchema(slice.Elem(), helper)
		if !ok {
			return facts.Type{}, false
		}
		return facts.ArrayType(elem), true
	}
	if array, ok := gotypes.Unalias(t).(*gotypes.Array); ok {
		elem, ok := queryHelperSchema(array.Elem(), helper)
		if !ok {
			return facts.Type{}, false
		}
		return facts.ArrayType(elem), true
	}
	if mapped, ok := gotypes.Unalias(t).(*gotypes.Map); ok {
		key, keyOK := queryHelperSchema(mapped.Key(), helper)
		value, valueOK := queryHelperSchema(mapped.Elem(), helper)
		if !keyOK || !valueOK {
			return facts.Type{}, false
		}
		return facts.MapTypeOf(key, value), true
	}
	if named, ok := gotypes.Unalias(t).(*gotypes.Named); ok {
		if obj := named.Obj(); obj != nil && obj.Pkg() != nil {
			if obj.Pkg().Path() == "github.com/google/uuid" && obj.Name() == "UUID" {
				return facts.WellKnownType(facts.WellKnownUUID), true
			}
			if obj.Pkg().Path() == "time" && obj.Name() == "Time" {
				return facts.WellKnownType(facts.WellKnownDateTime), true
			}
		}
		t = named.Underlying()
	}
	if basic, ok := gotypes.Unalias(t).(*gotypes.Basic); ok {
		switch basic.Kind() {
		case gotypes.String:
			return facts.PrimitiveType(facts.StringPrim()), true
		case gotypes.Bool:
			return facts.PrimitiveType(facts.BoolPrim()), true
		case gotypes.Int, gotypes.Int8, gotypes.Int16, gotypes.Int32, gotypes.Int64:
			return facts.PrimitiveType(facts.IntPrim(64, true)), true
		case gotypes.Uint, gotypes.Uint8, gotypes.Uint16, gotypes.Uint32, gotypes.Uint64:
			return facts.PrimitiveType(facts.IntPrim(64, false)), true
		case gotypes.Float32:
			return facts.PrimitiveType(facts.FloatPrim(32)), true
		case gotypes.Float64:
			return facts.PrimitiveType(facts.FloatPrim(64)), true
		}
	}
	if selectorName(helper.Fun) == "TrimSpace" {
		return facts.PrimitiveType(facts.StringPrim()), true
	}
	return facts.Type{}, false
}

func queryHelperRequiredFromCallee(info *gotypes.Info, helper *ast.CallExpr, callee handlerDecl, query *ast.CallExpr) bool {
	lowerName := strings.ToLower(selectorName(helper.Fun))
	if strings.Contains(lowerName, "optional") {
		return false
	}
	if queryHelperDefaultFromArgs(info, helper, signatureOf(info, helper.Fun)) != nil {
		return false
	}
	if queryDefaultValue(callee.info, query) != nil {
		return false
	}
	if selectorName(query.Fun) == "DefaultQuery" || selectorName(query.Fun) == "GetQuery" {
		return false
	}
	if strings.Contains(lowerName, "required") {
		return true
	}
	if helperEmptyStringTolerant(callee, query) {
		return false
	}
	return helperReturnsError(info, helper)
}

func queryHelperRequiredFromDirectCallee(info *gotypes.Info, helper *ast.CallExpr, callee handlerDecl, query *ast.CallExpr) bool {
	lowerName := strings.ToLower(selectorName(helper.Fun))
	if strings.Contains(lowerName, "optional") {
		return false
	}
	if queryHelperDefault(info, helper, query) != nil {
		return false
	}
	if queryDefaultValue(info, query) != nil {
		return false
	}
	if selectorName(query.Fun) == "DefaultQuery" || selectorName(query.Fun) == "GetQuery" {
		return false
	}
	if strings.Contains(lowerName, "required") {
		return true
	}
	sig := signatureOf(info, helper.Fun)
	if sig != nil && sig.Params() != nil {
		for i, arg := range helper.Args {
			if i >= sig.Params().Len() || !exprContainsCall(arg, query) {
				continue
			}
			if helperParamEmptyStringTolerant(callee, sig.Params().At(i)) {
				return false
			}
		}
	}
	return helperReturnsError(info, helper)
}

func signatureOf(info *gotypes.Info, expr ast.Expr) *gotypes.Signature {
	sig, _ := gotypes.Unalias(info.TypeOf(expr)).(*gotypes.Signature)
	return sig
}

func queryHelperDefault(info *gotypes.Info, helper *ast.CallExpr, query *ast.CallExpr) *facts.LiteralValue {
	sig, ok := gotypes.Unalias(info.TypeOf(helper.Fun)).(*gotypes.Signature)
	if !ok || sig.Params() == nil {
		return nil
	}
	afterQuery := query == nil
	for i, arg := range helper.Args {
		if !afterQuery {
			if arg == query || exprContainsCall(arg, query) {
				afterQuery = true
			}
			continue
		}
		if i >= sig.Params().Len() || !isDefaultParamName(sig.Params().At(i).Name()) {
			continue
		}
		if value := literalValue(info, arg); value != nil {
			return value
		}
	}
	return nil
}

func queryHelperDefaultFromArgs(info *gotypes.Info, helper *ast.CallExpr, sig *gotypes.Signature) *facts.LiteralValue {
	if sig == nil || sig.Params() == nil {
		return nil
	}
	for i, arg := range helper.Args {
		if i >= sig.Params().Len() || !isDefaultParamName(sig.Params().At(i).Name()) {
			continue
		}
		if value := literalValue(info, arg); value != nil {
			return value
		}
	}
	return nil
}

func helperEmptyStringTolerant(h handlerDecl, query *ast.CallExpr) bool {
	queryVars := queryResultVars(h, query)
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		ifStmt, ok := n.(*ast.IfStmt)
		if !ok || !exprChecksEmptyString(h.info, ifStmt.Cond, queryVars) {
			return true
		}
		for _, stmt := range ifStmt.Body.List {
			ret, ok := stmt.(*ast.ReturnStmt)
			if !ok || len(ret.Results) == 0 {
				continue
			}
			if returnTreatsEmptyStringAsOptional(ret) {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

func helperParamEmptyStringTolerant(h handlerDecl, param *gotypes.Var) bool {
	if h.decl == nil || h.decl.Body == nil || param == nil {
		return false
	}
	queryVars := map[gotypes.Object]bool{param: true}
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		ifStmt, ok := n.(*ast.IfStmt)
		if !ok || !exprChecksEmptyString(h.info, ifStmt.Cond, queryVars) {
			return true
		}
		for _, stmt := range ifStmt.Body.List {
			ret, ok := stmt.(*ast.ReturnStmt)
			if !ok || len(ret.Results) == 0 {
				continue
			}
			if returnTreatsEmptyStringAsOptional(ret) {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

func returnTreatsEmptyStringAsOptional(ret *ast.ReturnStmt) bool {
	if ret == nil || len(ret.Results) < 2 || !isNilIdent(ret.Results[len(ret.Results)-1]) {
		return false
	}
	for _, result := range ret.Results[:len(ret.Results)-1] {
		if isZeroValueExpr(result) {
			return true
		}
	}
	return false
}

func isZeroValueExpr(expr ast.Expr) bool {
	if isNilIdent(expr) {
		return true
	}
	switch value := expr.(type) {
	case *ast.BasicLit:
		switch value.Kind {
		case token.INT, token.FLOAT:
			return value.Value == "0" || value.Value == "0.0"
		case token.STRING:
			return isEmptyStringLiteral(value)
		}
	case *ast.Ident:
		return value.Name == "false"
	}
	return false
}

func queryResultVars(h handlerDecl, query *ast.CallExpr) map[gotypes.Object]bool {
	out := map[gotypes.Object]bool{}
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		assign, ok := n.(*ast.AssignStmt)
		if !ok {
			return true
		}
		for i, rhs := range assign.Rhs {
			if i >= len(assign.Lhs) || !exprContainsCall(rhs, query) {
				continue
			}
			id, ok := assign.Lhs[i].(*ast.Ident)
			if ok {
				if obj := h.info.ObjectOf(id); obj != nil {
					out[obj] = true
				}
			}
		}
		return true
	})
	return out
}

func exprChecksEmptyString(info *gotypes.Info, expr ast.Expr, queryVars map[gotypes.Object]bool) bool {
	switch node := expr.(type) {
	case *ast.BinaryExpr:
		if node.Op == token.EQL || node.Op == token.NEQ {
			return (exprIsQueryValue(info, node.X, queryVars) && isEmptyStringLiteral(node.Y)) ||
				(isEmptyStringLiteral(node.X) && exprIsQueryValue(info, node.Y, queryVars))
		}
		return exprChecksEmptyString(info, node.X, queryVars) || exprChecksEmptyString(info, node.Y, queryVars)
	case *ast.ParenExpr:
		return exprChecksEmptyString(info, node.X, queryVars)
	}
	return false
}

func exprIsQueryValue(info *gotypes.Info, expr ast.Expr, queryVars map[gotypes.Object]bool) bool {
	if call, ok := expr.(*ast.CallExpr); ok {
		if selectorName(call.Fun) == "TrimSpace" && len(call.Args) == 1 {
			return exprIsQueryValue(info, call.Args[0], queryVars)
		}
		name, recvPkg, ok := routes.GinMethod(info, call)
		return ok && recvPkg == routes.GinPkgPath && isGinQueryMethod(name)
	}
	id, ok := expr.(*ast.Ident)
	return ok && queryVars[info.ObjectOf(id)]
}

func isNilIdent(expr ast.Expr) bool {
	id, ok := expr.(*ast.Ident)
	return ok && id.Name == "nil"
}

func exprContainsCall(expr ast.Expr, target *ast.CallExpr) bool {
	if expr == nil || target == nil {
		return false
	}
	found := false
	ast.Inspect(expr, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if ok && call == target {
			found = true
			return false
		}
		return true
	})
	return found
}

func isDefaultParamName(name string) bool {
	lower := strings.ToLower(name)
	return strings.Contains(lower, "default") || strings.Contains(lower, "fallback")
}

func literalValue(info *gotypes.Info, expr ast.Expr) *facts.LiteralValue {
	if value := constValueOf(info, expr); value != nil {
		return literalValueFromConstant(value)
	}
	switch value := expr.(type) {
	case *ast.BasicLit:
		switch value.Kind {
		case token.STRING:
			unquoted, err := strconv.Unquote(value.Value)
			if err != nil {
				return nil
			}
			return &facts.LiteralValue{Type: "string", Value: unquoted}
		case token.INT, token.FLOAT:
			return &facts.LiteralValue{Type: "number", Value: value.Value}
		}
	case *ast.Ident:
		if value.Name == "true" {
			return &facts.LiteralValue{Type: "bool", Value: true}
		}
		if value.Name == "false" {
			return &facts.LiteralValue{Type: "bool", Value: false}
		}
	}
	return nil
}

func constValueOf(info *gotypes.Info, expr ast.Expr) constant.Value {
	if info == nil || expr == nil {
		return nil
	}
	if tv, ok := info.Types[expr]; ok && tv.Value != nil {
		return tv.Value
	}
	if ident := leafIdent(expr); ident != nil {
		if c, ok := info.ObjectOf(ident).(*gotypes.Const); ok {
			return c.Val()
		}
	}
	return nil
}

func literalValueFromConstant(value constant.Value) *facts.LiteralValue {
	switch value.Kind() {
	case constant.String:
		return &facts.LiteralValue{Type: "string", Value: constant.StringVal(value)}
	case constant.Bool:
		return &facts.LiteralValue{Type: "bool", Value: constant.BoolVal(value)}
	case constant.Int:
		return &facts.LiteralValue{Type: "number", Value: value.ExactString()}
	case constant.Float:
		number := value.ExactString()
		if strings.Contains(number, "/") {
			if f, ok := constant.Float64Val(value); ok {
				number = strconv.FormatFloat(f, 'g', -1, 64)
			}
		}
		return &facts.LiteralValue{Type: "number", Value: number}
	default:
		return nil
	}
}

func firstLiteral(values ...*facts.LiteralValue) *facts.LiteralValue {
	for _, value := range values {
		if value != nil {
			return value
		}
	}
	return nil
}

func isErrorType(t gotypes.Type) bool {
	named, ok := gotypes.Unalias(t).(*gotypes.Named)
	if !ok || named.Obj() == nil {
		return false
	}
	return named.Obj().Name() == "error" && named.Obj().Pkg() == nil
}

func selectorName(expr ast.Expr) string {
	switch fun := expr.(type) {
	case *ast.SelectorExpr:
		return fun.Sel.Name
	case *ast.Ident:
		return fun.Name
	default:
		return ""
	}
}

// bindRequestType resolves ShouldBindJSON(&x), ShouldBindJSON(ptr), and
// ShouldBindJSON(new(T)) to the bound named type. Gin accepts all of these
// pointer-bearing shapes; restricting extraction to the address-of syntax would
// silently lose an otherwise statically known body.
func (a *Analyzer) bindRequestType(info *gotypes.Info, call *ast.CallExpr) (*facts.TypeRef, gotypes.Type, bool) {
	if info == nil || len(call.Args) < 1 {
		return nil, nil, false
	}
	bound := info.TypeOf(call.Args[0])
	if bound == nil {
		return nil, nil, false
	}
	if pointer, ok := gotypes.Unalias(bound).(*gotypes.Pointer); ok {
		bound = pointer.Elem()
	}
	id, ok := a.namedTypeID(bound)
	if !ok {
		return nil, nil, false
	}
	return &facts.TypeRef{RefID: id}, bound, true
}

func bindContentType(method string, info *gotypes.Info, call *ast.CallExpr, bound gotypes.Type) string {
	switch explicitBindingName(info, call) {
	case "FormMultipart":
		return "multipart/form-data"
	case "Form", "FormPost":
		return "application/x-www-form-urlencoded"
	case "JSON":
		return "application/json"
	}
	if typeContainsMultipartFile(bound) {
		return "multipart/form-data"
	}
	if typeHasFormTag(bound) {
		return "application/x-www-form-urlencoded"
	}
	if method == "Bind" || method == "ShouldBind" {
		return ""
	}
	return ""
}

func explicitBindingName(info *gotypes.Info, call *ast.CallExpr) string {
	if len(call.Args) < 2 {
		return ""
	}
	switch arg := call.Args[1].(type) {
	case *ast.SelectorExpr:
		return arg.Sel.Name
	case *ast.Ident:
		if obj := info.ObjectOf(arg); obj != nil {
			return obj.Name()
		}
		return arg.Name
	default:
		return ""
	}
}

func formField(name string, schema facts.Type, required bool) facts.FieldFact {
	return facts.FieldFact{
		JSONName:                  name,
		SerializerMayOmit:         !required,
		DeserializerAcceptsAbsent: !required,
		DeserializerAcceptsNull:   false,
		SerializerMayEmitNull:     false,
		ValidatorRequiresPresence: required,
		ValidatorRejectsNull:      false,
		Schema:                    schema,
	}
}

func addMultipartFileField(
	formFields map[string]facts.FieldFact,
	manualFormFields map[string]bool,
	formHasFile *bool,
	name string,
) {
	formFields[name] = formField(name, facts.PrimitiveType(facts.BytesPrim()), true)
	manualFormFields[name] = true
	*formHasFile = true
}

func isFormContentType(contentType string) bool {
	return contentType == "multipart/form-data" || contentType == "application/x-www-form-urlencoded"
}

func (a *Analyzer) addBoundFormFields(
	frame helperFrame,
	call *ast.CallExpr,
	fields map[string]facts.FieldFact,
	route routes.Route,
	diags *diag.Accumulator,
) (string, bool) {
	bound := boundTypeFromCall(frame, call)
	params, ok := a.parametersFromBoundType(bound, "form", frame.decl.fset, route, diags, map[string]bool{})
	if !ok {
		return "", false
	}
	for _, param := range params {
		field := formField(param.Name, param.Schema, param.Required)
		if param.Default != nil {
			field.Meta = &facts.FieldMeta{Default: param.Default}
		}
		existing, exists := fields[param.Name]
		if !exists {
			fields[param.Name] = field
			continue
		}
		if !reflect.DeepEqual(existing.Schema, field.Schema) {
			existingSpecificity := parameterSchemaSpecificity(existing.Schema)
			incomingSpecificity := parameterSchemaSpecificity(field.Schema)
			switch {
			case incomingSpecificity > existingSpecificity:
				existing.Schema = field.Schema
			case incomingSpecificity == existingSpecificity:
				if diags != nil {
					diags.RequestBodyUnresolved(
						param.Name,
						route.Method,
						untypedRouteLabel(route),
						"conflicting extracted schemas for form field "+param.Name,
						param.Span.File,
						param.Span.StartLine,
					)
				}
			}
		}
		existing.ValidatorRequiresPresence = existing.ValidatorRequiresPresence || field.ValidatorRequiresPresence
		existing.DeserializerAcceptsAbsent = !existing.ValidatorRequiresPresence
		existing.SerializerMayOmit = existing.DeserializerAcceptsAbsent
		if existing.Meta == nil {
			existing.Meta = field.Meta
		}
		fields[param.Name] = existing
	}
	refID, _ := a.namedTypeID(bound)
	return refID, true
}

func syntheticFormSchemaIdentity(handler string) (id string, name string) {
	base := exportedIdentifier(handler)
	if base == "" {
		base = "Request"
	}
	name = base + "FormRequest"
	return "__synthetic." + name, name
}

func syntheticRawJSONRequestSchemaIdentity(handler string) (id string, name string) {
	base := exportedIdentifier(handler)
	if base == "" {
		base = "Request"
	}
	name = base + "RawJSONRequest"
	return "__synthetic." + name, name
}

func syntheticResponseSchemaIdentity(handler string, status uint16) (id string, name string) {
	base := exportedIdentifier(handler)
	if base == "" {
		base = "Response"
	}
	name = base + strconv.FormatUint(uint64(status), 10) + "Response"
	return "__synthetic." + name, name
}

func exportedIdentifier(name string) string {
	if name == "" {
		return ""
	}
	return strings.ToUpper(name[:1]) + name[1:]
}

func typeContainsMultipartFile(t gotypes.Type) bool {
	return typeContainsMultipartFileSeen(t, map[string]bool{})
}

func typeContainsMultipartFileSeen(t gotypes.Type, seen map[string]bool) bool {
	if t == nil {
		return false
	}
	key := t.String()
	if seen[key] {
		return false
	}
	seen[key] = true
	switch u := gotypes.Unalias(t).(type) {
	case *gotypes.Pointer:
		return typeContainsMultipartFileSeen(u.Elem(), seen)
	case *gotypes.Slice:
		return typeContainsMultipartFileSeen(u.Elem(), seen)
	case *gotypes.Array:
		return typeContainsMultipartFileSeen(u.Elem(), seen)
	case *gotypes.Named:
		obj := u.Obj()
		if obj != nil && obj.Pkg() != nil && obj.Pkg().Path() == "mime/multipart" && obj.Name() == "FileHeader" {
			return true
		}
		return typeContainsMultipartFileSeen(u.Underlying(), seen)
	case *gotypes.Struct:
		for i := 0; i < u.NumFields(); i++ {
			if typeContainsMultipartFileSeen(u.Field(i).Type(), seen) {
				return true
			}
		}
	}
	return false
}

func typeHasFormTag(t gotypes.Type) bool {
	return typeHasFormTagSeen(t, map[string]bool{})
}

func typeHasFormTagSeen(t gotypes.Type, seen map[string]bool) bool {
	if t == nil {
		return false
	}
	key := t.String()
	if seen[key] {
		return false
	}
	seen[key] = true
	switch u := gotypes.Unalias(t).(type) {
	case *gotypes.Pointer:
		return typeHasFormTagSeen(u.Elem(), seen)
	case *gotypes.Named:
		return typeHasFormTagSeen(u.Underlying(), seen)
	case *gotypes.Struct:
		for i := 0; i < u.NumFields(); i++ {
			tag := reflect.StructTag(u.Tag(i))
			if raw, ok := tag.Lookup("form"); ok && raw != "" && raw != "-" {
				return true
			}
			if u.Field(i).Embedded() && typeHasFormTagSeen(u.Field(i).Type(), seen) {
				return true
			}
		}
	}
	return false
}

// analyzeJSON resolves c.JSON(http.StatusXxx, y): status from go/constant, body
// from the named type of y. A dynamic/unresolvable body emits a WARN (D-05) and
// records the status with a nil body so the response is never silently dropped.
func (a *Analyzer) analyzeJSON(
	h handlerDecl,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
) {
	if len(call.Args) < 2 {
		return
	}
	status, ok := statusOf(h.info, call.Args[0])
	if !ok {
		file, line := positionOf(h.fset, call.Pos())
		diags.DynamicResponse(route.Method, untypedRouteLabel(route), route.Handler, "non-constant HTTP status", file, line)
		return
	}

	var body *facts.TypeRef
	if id, schema, ok := a.syntheticJSONResponse(h, call.Args[1], route.Handler, status); ok {
		body = &facts.TypeRef{RefID: id}
		cf.Schemas = append(cf.Schemas, schema)
	} else if id, ok := a.namedTypeID(h.info.TypeOf(call.Args[1])); ok {
		body = &facts.TypeRef{RefID: id}
	} else {
		file, line := positionOf(h.fset, call.Pos())
		diags.DynamicResponse(route.Method, untypedRouteLabel(route), route.Handler, "response body does not resolve to a named type", file, line)
	}
	file, line := positionOf(h.fset, call.Pos())
	a.addJSONResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:       status,
		Body:         body,
		ContentTypes: []string{"application/json"},
	}, route, diags, file, line)
}

func (a *Analyzer) analyzeStatus(
	h handlerDecl,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	provisional bool,
	diags *diag.Accumulator,
) {
	if len(call.Args) < 1 {
		return
	}
	status, ok := statusOf(h.info, call.Args[0])
	if !ok {
		file, line := positionOf(h.fset, call.Pos())
		diags.DynamicResponse(route.Method, untypedRouteLabel(route), route.Handler, "non-constant HTTP status", file, line)
		return
	}
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{Status: status}, provisional)
}

func (a *Analyzer) analyzeBinaryStatus(
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	status uint16,
	contentType string,
) {
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:       status,
		BodyKind:     "binary",
		ContentType:  responseContentType(contentType),
		ContentTypes: responseContentTypes(responseContentType(contentType)),
	}, false)
}

func (a *Analyzer) addSSEResponse(
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
) {
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:       200,
		BodyKind:     "sse",
		ContentType:  "text/event-stream",
		ContentTypes: []string{"text/event-stream"},
	}, false)
}

func streamCallContainsSSEvent(info *gotypes.Info, stream *ast.CallExpr) bool {
	found := false
	for _, arg := range stream.Args {
		ast.Inspect(arg, func(n ast.Node) bool {
			if found {
				return false
			}
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			name, recvPkg, ok := routes.GinMethod(info, call)
			if ok && recvPkg == routes.GinPkgPath && name == "SSEvent" {
				found = true
				return false
			}
			return true
		})
	}
	return found
}

func (a *Analyzer) analyzeData(
	h handlerDecl,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
) {
	if len(call.Args) < 3 {
		return
	}
	status, ok := statusOf(h.info, call.Args[0])
	if !ok {
		file, line := positionOf(h.fset, call.Pos())
		diags.DynamicResponse(route.Method, untypedRouteLabel(route), route.Handler, "non-constant HTTP status", file, line)
		return
	}
	if !isByteSlice(h.info.TypeOf(call.Args[2])) {
		file, line := positionOf(h.fset, call.Pos())
		diags.ResponseSchemaUnresolved(route.Method, untypedRouteLabel(route), route.Handler, "unsupported binary response pattern: Gin Data payload is not []byte (GO-05)", file, line)
		return
	}
	contentType := "application/octet-stream"
	if value, ok := a.stringValueOf(h, call.Args[1]); ok {
		contentType = responseContentType(value)
	} else {
		file, line := positionOf(h.fset, call.Pos())
		diags.ResponseMediaTypeUnresolved(route.Method, untypedRouteLabel(route), "unsupported binary response pattern: Gin Data content type is dynamic; defaulting to application/octet-stream (GO-05)", file, line)
	}
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:       status,
		BodyKind:     "binary",
		ContentType:  contentType,
		ContentTypes: responseContentTypes(contentType),
	}, false)
}

func (a *Analyzer) analyzeDataFromReader(
	h handlerDecl,
	call *ast.CallExpr,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
) {
	if len(call.Args) < 3 {
		return
	}
	status, ok := statusOf(h.info, call.Args[0])
	if !ok {
		file, line := positionOf(h.fset, call.Pos())
		diags.DynamicResponse(route.Method, untypedRouteLabel(route), route.Handler, "non-constant HTTP status", file, line)
		return
	}
	contentType := "application/octet-stream"
	if value, ok := a.stringValueOf(h, call.Args[2]); ok {
		contentType = responseContentType(value)
	} else {
		file, line := positionOf(h.fset, call.Pos())
		diags.ResponseMediaTypeUnresolved(route.Method, untypedRouteLabel(route), "unsupported binary response pattern: Gin DataFromReader content type is dynamic; defaulting to application/octet-stream (GO-05)", file, line)
	}
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:       status,
		BodyKind:     "binary",
		ContentType:  contentType,
		ContentTypes: responseContentTypes(contentType),
	}, false)
}

func (a *Analyzer) analyzeRedirect(
	h handlerDecl,
	call *ast.CallExpr,
	statusIndex int,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
) {
	if call == nil || statusIndex >= len(call.Args) {
		return
	}
	status, ok := statusOf(h.info, call.Args[statusIndex])
	if !ok || status < 300 || status >= 400 {
		if diags != nil {
			file, line := positionOf(h.fset, call.Pos())
			diags.DynamicResponse(
				route.Method,
				untypedRouteLabel(route),
				route.Handler,
				"redirect status must be a constant 3xx HTTP status",
				file,
				line,
			)
		}
		return
	}
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:   status,
		BodyKind: "empty",
	}, false)
}

func (a *Analyzer) analyzeRedirectInFrame(
	frame helperFrame,
	call *ast.CallExpr,
	statusIndex int,
	route routes.Route,
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	diags *diag.Accumulator,
) {
	if call == nil || statusIndex >= len(call.Args) {
		return
	}
	status, ok := statusOf(frame.decl.info, call.Args[statusIndex])
	if !ok {
		status, ok = statusFromLiteral(frameLiteralValue(frame, call.Args[statusIndex]))
	}
	if !ok || status < 300 || status >= 400 {
		if diags != nil {
			file, line := positionOf(frame.decl.fset, call.Pos())
			diags.DynamicResponse(
				route.Method,
				untypedRouteLabel(route),
				route.Handler,
				"redirect status must be a constant 3xx HTTP status",
				file,
				line,
			)
		}
		return
	}
	a.addResponse(cf, seenStatus, provisionalStatus, facts.ResponseFact{
		Status:   status,
		BodyKind: "empty",
	}, false)
}

func statusFromLiteral(value *facts.LiteralValue) (uint16, bool) {
	if value == nil || value.Type != "number" {
		return 0, false
	}
	text, ok := value.Value.(string)
	if !ok {
		return 0, false
	}
	integer, err := strconv.ParseInt(text, 10, 64)
	if err != nil {
		return 0, false
	}
	return httpStatusInRange(integer)
}

func isHTTPRedirectCall(info *gotypes.Info, call *ast.CallExpr) bool {
	function := calledFuncObject(info, call.Fun)
	return function != nil && function.Pkg() != nil &&
		function.Pkg().Path() == "net/http" && function.Name() == "Redirect"
}

type responseHeaderSet map[string]facts.ResponseHeaderFact

type responseHeaderFlow struct {
	next       responseHeaderSet
	continues  bool
	returns    responseHeaderSet
	hasReturns bool
}

type responseHeaderCollector struct {
	analyzer    *Analyzer
	byStatus    map[uint16]responseHeaderSet
	stack       map[string]bool
	route       routes.Route
	diagnostics *diag.Accumulator
	// A helper reached from two call sites is walked twice, so an unresolved name is
	// reported once per source position rather than once per walk.
	reported map[token.Pos]bool
}

func (a *Analyzer) collectResponseHeadersByStatus(
	h handlerDecl,
	route routes.Route,
	diags *diag.Accumulator,
) map[uint16][]facts.ResponseHeaderFact {
	collector := responseHeaderCollector{
		analyzer:    a,
		byStatus:    map[uint16]responseHeaderSet{},
		stack:       map[string]bool{},
		route:       route,
		diagnostics: diags,
		reported:    map[token.Pos]bool{},
	}
	collector.analyzeFunction(
		helperFrame{decl: h, bindings: map[gotypes.Object]helperBinding{}},
		responseHeaderSet{},
		0,
	)
	out := make(map[uint16][]facts.ResponseHeaderFact, len(collector.byStatus))
	for status, byName := range collector.byStatus {
		headers := make([]facts.ResponseHeaderFact, 0, len(byName))
		for _, header := range byName {
			headers = append(headers, header)
		}
		sort.Slice(headers, func(i, j int) bool { return headers[i].Name < headers[j].Name })
		out[status] = headers
	}
	return out
}

func (c *responseHeaderCollector) analyzeFunction(
	frame helperFrame,
	inherited responseHeaderSet,
	depth int,
) responseHeaderSet {
	if frame.decl.decl == nil || frame.decl.decl.Body == nil || frame.decl.info == nil || depth >= maxContextHelperDepth {
		return cloneResponseHeaderSet(inherited)
	}
	key := frame.decl.identityKey()
	if c.stack[key] {
		return cloneResponseHeaderSet(inherited)
	}
	c.stack[key] = true
	defer delete(c.stack, key)
	flow := c.analyzeStatements(frame, frame.decl.decl.Body.List, inherited, depth)
	exits := responseHeaderSet{}
	if flow.continues {
		mergeResponseHeaderSets(exits, flow.next)
	}
	if flow.hasReturns {
		mergeResponseHeaderSets(exits, flow.returns)
	}
	return exits
}

func (c *responseHeaderCollector) analyzeStatements(
	frame helperFrame,
	statements []ast.Stmt,
	inherited responseHeaderSet,
	depth int,
) responseHeaderFlow {
	current := cloneResponseHeaderSet(inherited)
	flow := responseHeaderFlow{next: current, continues: true, returns: responseHeaderSet{}}
	for _, statement := range statements {
		if !flow.continues {
			break
		}
		step := c.analyzeStatement(frame, statement, current, depth)
		if step.hasReturns {
			mergeResponseHeaderSets(flow.returns, step.returns)
			flow.hasReturns = true
		}
		flow.continues = step.continues
		current = step.next
		flow.next = current
	}
	return flow
}

func (c *responseHeaderCollector) analyzeStatement(
	frame helperFrame,
	statement ast.Stmt,
	inherited responseHeaderSet,
	depth int,
) responseHeaderFlow {
	current := cloneResponseHeaderSet(inherited)
	flow := responseHeaderFlow{next: current, continues: true, returns: responseHeaderSet{}}
	switch stmt := statement.(type) {
	case *ast.BlockStmt:
		return c.analyzeStatements(frame, stmt.List, current, depth)
	case *ast.ReturnStmt:
		for _, result := range stmt.Results {
			c.analyzeNodeCalls(frame, result, current, depth)
		}
		flow.continues = false
		flow.returns = current
		flow.hasReturns = true
		return flow
	case *ast.IfStmt:
		if stmt.Init != nil {
			c.analyzeNodeCalls(frame, stmt.Init, current, depth)
		}
		c.analyzeNodeCalls(frame, stmt.Cond, current, depth)
		body := c.analyzeStatements(frame, stmt.Body.List, current, depth)
		other := responseHeaderFlow{
			next:      cloneResponseHeaderSet(current),
			continues: true,
			returns:   responseHeaderSet{},
		}
		if stmt.Else != nil {
			other = c.analyzeElse(frame, stmt.Else, current, depth)
		}
		flow.next = responseHeaderSet{}
		flow.continues = body.continues || other.continues
		if body.continues {
			mergeResponseHeaderSets(flow.next, body.next)
		}
		if other.continues {
			mergeResponseHeaderSets(flow.next, other.next)
		}
		flow.returns = responseHeaderSet{}
		flow.hasReturns = body.hasReturns || other.hasReturns
		if body.hasReturns {
			mergeResponseHeaderSets(flow.returns, body.returns)
		}
		if other.hasReturns {
			mergeResponseHeaderSets(flow.returns, other.returns)
		}
		return flow
	case *ast.ForStmt:
		if stmt.Init != nil {
			c.analyzeNodeCalls(frame, stmt.Init, current, depth)
		}
		if stmt.Cond != nil {
			c.analyzeNodeCalls(frame, stmt.Cond, current, depth)
		}
		body := c.analyzeStatements(frame, stmt.Body.List, current, depth)
		if stmt.Post != nil && body.continues {
			c.analyzeNodeCalls(frame, stmt.Post, body.next, depth)
		}
		if body.continues {
			mergeResponseHeaderSets(current, body.next)
		}
		flow.next = current
		flow.returns = body.returns
		flow.hasReturns = body.hasReturns
		return flow
	case *ast.RangeStmt:
		c.analyzeNodeCalls(frame, stmt.X, current, depth)
		body := c.analyzeStatements(frame, stmt.Body.List, current, depth)
		if body.continues {
			mergeResponseHeaderSets(current, body.next)
		}
		flow.next = current
		flow.returns = body.returns
		flow.hasReturns = body.hasReturns
		return flow
	case *ast.SwitchStmt:
		if stmt.Init != nil {
			c.analyzeNodeCalls(frame, stmt.Init, current, depth)
		}
		if stmt.Tag != nil {
			c.analyzeNodeCalls(frame, stmt.Tag, current, depth)
		}
		return c.analyzeCaseClauses(frame, stmt.Body.List, current, depth)
	case *ast.TypeSwitchStmt:
		if stmt.Init != nil {
			c.analyzeNodeCalls(frame, stmt.Init, current, depth)
		}
		if stmt.Assign != nil {
			c.analyzeNodeCalls(frame, stmt.Assign, current, depth)
		}
		return c.analyzeCaseClauses(frame, stmt.Body.List, current, depth)
	default:
		c.analyzeNodeCalls(frame, statement, current, depth)
		return flow
	}
}

func (c *responseHeaderCollector) analyzeElse(
	frame helperFrame,
	node ast.Stmt,
	inherited responseHeaderSet,
	depth int,
) responseHeaderFlow {
	if block, ok := node.(*ast.BlockStmt); ok {
		return c.analyzeStatements(frame, block.List, inherited, depth)
	}
	return c.analyzeStatement(frame, node, inherited, depth)
}

func (c *responseHeaderCollector) analyzeCaseClauses(
	frame helperFrame,
	clauses []ast.Stmt,
	inherited responseHeaderSet,
	depth int,
) responseHeaderFlow {
	flow := responseHeaderFlow{next: responseHeaderSet{}, returns: responseHeaderSet{}}
	hasDefault := false
	for _, statement := range clauses {
		clause, ok := statement.(*ast.CaseClause)
		if !ok {
			continue
		}
		if len(clause.List) == 0 {
			hasDefault = true
		}
		for _, expression := range clause.List {
			c.analyzeNodeCalls(frame, expression, inherited, depth)
		}
		branch := c.analyzeStatements(frame, clause.Body, inherited, depth)
		if branch.continues {
			flow.continues = true
			mergeResponseHeaderSets(flow.next, branch.next)
		}
		if branch.hasReturns {
			flow.hasReturns = true
			mergeResponseHeaderSets(flow.returns, branch.returns)
		}
	}
	if !hasDefault {
		flow.continues = true
		mergeResponseHeaderSets(flow.next, inherited)
	}
	return flow
}

func (c *responseHeaderCollector) analyzeNodeCalls(
	frame helperFrame,
	node ast.Node,
	headers responseHeaderSet,
	depth int,
) {
	if node == nil {
		return
	}
	ast.Inspect(node, func(child ast.Node) bool {
		call, ok := child.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ginCall := routes.GinMethod(frame.decl.info, call)
		if ginCall && recvPkg == routes.GinPkgPath {
			if name == "Header" {
				if header, ok := frameCallStringArg(frame, call, 0); ok {
					addResponseHeader(headers, header)
				} else {
					c.reportUnresolvedHeaderName(frame, call.Pos(), "Header")
				}
				return true
			}
			c.recordGinResponse(frame, call, name, headers)
			return true
		}
		if isHTTPRedirectCall(frame.decl.info, call) {
			if status, ok := responseStatusInFrame(frame, call, 3); ok && status >= 300 && status < 400 {
				responseHeaders := cloneResponseHeaderSet(headers)
				addResponseHeader(responseHeaders, "Location")
				c.record(status, responseHeaders)
			}
			return true
		}
		if responseHeaderMutation(frame, call) {
			if header, ok := frameCallStringArg(frame, call, 0); ok {
				addResponseHeader(headers, header)
			} else {
				c.reportUnresolvedHeaderName(frame, call.Pos(), selectorName(call.Fun))
			}
			return true
		}
		fn := calledFuncObject(frame.decl.info, call.Fun)
		callee, ok := c.analyzer.moduleOwnedCallee(fn)
		if !ok || !frameCallPassesResponseContext(frame, call) {
			return true
		}
		next := helperFrame{decl: callee, bindings: helperCallBindings(frame, call, fn)}
		mergeResponseHeaderSets(headers, c.analyzeFunction(next, headers, depth+1))
		return true
	})
}

func (c *responseHeaderCollector) recordGinResponse(
	frame helperFrame,
	call *ast.CallExpr,
	name string,
	headers responseHeaderSet,
) {
	status := uint16(0)
	ok := false
	switch name {
	case "JSON", "Status", "AbortWithStatus", "Data", "DataFromReader", "Redirect":
		status, ok = responseStatusInFrame(frame, call, 0)
	case "File", "FileAttachment", "SSEvent":
		status, ok = 200, true
	case "Stream":
		status, ok = 200, streamCallContainsSSEvent(frame.decl.info, call)
	}
	if !ok {
		return
	}
	responseHeaders := cloneResponseHeaderSet(headers)
	switch name {
	case "Redirect":
		if status < 300 || status >= 400 {
			return
		}
		addResponseHeader(responseHeaders, "Location")
	case "Data", "File":
		addResponseHeader(responseHeaders, "Content-Type")
	case "FileAttachment":
		addResponseHeader(responseHeaders, "Content-Type")
		addResponseHeader(responseHeaders, "Content-Disposition")
	case "DataFromReader":
		addResponseHeader(responseHeaders, "Content-Type")
		if len(call.Args) > 1 && nonNegativeIntegerConstant(frame.decl.info, call.Args[1]) {
			addResponseHeader(responseHeaders, "Content-Length")
		}
		if len(call.Args) > 4 {
			extra, resolved := stringMapLiteralKeys(frame, call.Args[4])
			for _, header := range extra {
				addResponseHeader(responseHeaders, header)
			}
			if !resolved {
				c.reportUnresolvedHeaderName(frame, call.Args[4].Pos(), "DataFromReader")
			}
		}
	}
	c.record(status, responseHeaders)
}

// reportUnresolvedHeaderName records a response header write whose name is not a
// constant. The header is dropped rather than guessed (rule 3), so the omission
// has to be visible or the generated contract silently loses it.
func (c *responseHeaderCollector) reportUnresolvedHeaderName(
	frame helperFrame,
	pos token.Pos,
	subject string,
) {
	if c.diagnostics == nil || c.reported[pos] {
		return
	}
	c.reported[pos] = true
	file, line := positionOf(frame.decl.fset, pos)
	c.diagnostics.ResponseHeaderUnresolved(
		c.route.Method,
		untypedRouteLabel(c.route),
		subject+" writes a response header under a name that is not a constant",
		file,
		line,
	)
}

func (c *responseHeaderCollector) record(status uint16, headers responseHeaderSet) {
	if len(headers) == 0 {
		return
	}
	if c.byStatus[status] == nil {
		c.byStatus[status] = responseHeaderSet{}
	}
	mergeResponseHeaderSets(c.byStatus[status], headers)
}

func responseStatusInFrame(frame helperFrame, call *ast.CallExpr, index int) (uint16, bool) {
	if call == nil || index >= len(call.Args) {
		return 0, false
	}
	if status, ok := statusOf(frame.decl.info, call.Args[index]); ok {
		return status, true
	}
	return statusFromLiteral(frameLiteralValue(frame, call.Args[index]))
}

func addResponseHeader(headers responseHeaderSet, name string) {
	if name == "" {
		return
	}
	schema := facts.PrimitiveType(facts.StringPrim())
	if strings.EqualFold(name, "Content-Length") {
		schema = facts.PrimitiveType(facts.IntPrim(64, false))
	}
	headers[strings.ToLower(name)] = facts.ResponseHeaderFact{Name: name, Schema: schema}
}

func cloneResponseHeaderSet(headers responseHeaderSet) responseHeaderSet {
	clone := make(responseHeaderSet, len(headers))
	mergeResponseHeaderSets(clone, headers)
	return clone
}

func mergeResponseHeaderSets(target, source responseHeaderSet) {
	for name, header := range source {
		target[name] = header
	}
}

func nonNegativeIntegerConstant(info *gotypes.Info, expr ast.Expr) bool {
	value, ok := info.Types[expr]
	if !ok || value.Value == nil || value.Value.Kind() != constant.Int {
		return false
	}
	integer, exact := constant.Int64Val(value.Value)
	return exact && integer >= 0
}

// stringMapLiteralKeys reads the keys of a literal string map. The second result
// reports whether the whole map was readable: an explicit nil states "no extra
// headers", a literal whose keys evaluate to constants states exactly those, and
// anything else is a name typed source cannot state.
//
// Keys go through the frame's constant evaluator rather than a literal-only read,
// so a named constant is as statically known as the string it was declared from.
func stringMapLiteralKeys(frame helperFrame, expr ast.Expr) ([]string, bool) {
	if isNilIdent(expr) {
		return nil, true
	}
	literal, ok := expr.(*ast.CompositeLit)
	if !ok {
		return nil, false
	}
	keys := make([]string, 0, len(literal.Elts))
	resolved := true
	for _, element := range literal.Elts {
		pair, ok := element.(*ast.KeyValueExpr)
		if !ok {
			resolved = false
			continue
		}
		if key, ok := frameStringValue(frame, pair.Key); ok {
			keys = append(keys, key)
		} else {
			resolved = false
		}
	}
	return keys, resolved
}

// responseHeaderMutation reports whether call writes a header on the map the
// response is actually sent with.
//
// The receiver's type is a necessary condition, never a sufficient one:
// `c.Request.Header` and a local `http.Header` have exactly the same type as
// `c.Writer.Header()`, so classifying by type alone states inbound and scratch
// headers as part of the outbound contract. The map has to be provably the
// writer's own.
func responseHeaderMutation(frame helperFrame, call *ast.CallExpr) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || selector.Sel == nil || (selector.Sel.Name != "Set" && selector.Sel.Name != "Add") {
		return false
	}
	if !isNamedType(frameTypeOf(frame, selector.X), "net/http", "Header") {
		return false
	}
	return responseWriterHeaderExpr(frame, selector.X)
}

// responseWriterHeaderExpr reports whether expr is a response writer's own header
// map: either `<writer>.Header()` directly, or a variable bound to one.
func responseWriterHeaderExpr(frame helperFrame, expr ast.Expr) bool {
	switch node := expr.(type) {
	case *ast.ParenExpr:
		return responseWriterHeaderExpr(frame, node.X)
	case *ast.CallExpr:
		return isResponseWriterHeaderCall(frame, node)
	case *ast.Ident:
		if frame.decl.info == nil {
			return false
		}
		object := frame.decl.info.ObjectOf(node)
		return object != nil && responseWriterHeaderVars(frame)[object]
	}
	return false
}

func isResponseWriterHeaderCall(frame helperFrame, call *ast.CallExpr) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || selector.Sel == nil || selector.Sel.Name != "Header" || len(call.Args) != 0 {
		return false
	}
	writer := frameTypeOf(frame, selector.X)
	return isNamedType(writer, "net/http", "ResponseWriter") ||
		isNamedType(writer, routes.GinPkgPath, "ResponseWriter")
}

// responseWriterHeaderVars collects the variables bound to a response writer's
// header map, so the `h := c.Writer.Header(); h.Set(…)` idiom stays a response
// fact while an unrelated `http.Header` value does not.
func responseWriterHeaderVars(frame helperFrame) map[gotypes.Object]bool {
	vars := map[gotypes.Object]bool{}
	if frame.decl.decl == nil || frame.decl.decl.Body == nil || frame.decl.info == nil {
		return vars
	}
	ast.Inspect(frame.decl.decl.Body, func(node ast.Node) bool {
		assign, ok := node.(*ast.AssignStmt)
		if !ok || len(assign.Rhs) != 1 || len(assign.Lhs) != 1 {
			return true
		}
		call, ok := assign.Rhs[0].(*ast.CallExpr)
		if !ok || !isResponseWriterHeaderCall(frame, call) {
			return true
		}
		if ident, ok := assign.Lhs[0].(*ast.Ident); ok && ident.Name != "_" {
			if object := frame.decl.info.ObjectOf(ident); object != nil {
				vars[object] = true
			}
		}
		return true
	})
	return vars
}

func frameCallPassesResponseContext(frame helperFrame, call *ast.CallExpr) bool {
	if frameCallPassesGinContext(frame, call) {
		return true
	}
	for _, argument := range call.Args {
		argumentType := frameTypeOf(frame, argument)
		if isNamedType(argumentType, "net/http", "Request") ||
			isNamedType(argumentType, "net/http", "ResponseWriter") {
			return true
		}
	}
	return false
}

func (a *Analyzer) addResponse(
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	response facts.ResponseFact,
	provisional bool,
) {
	if seenStatus[response.Status] {
		if provisionalStatus[response.Status] && !provisional {
			for i := range cf.Responses {
				if cf.Responses[i].Status == response.Status {
					cf.Responses[i] = response
					break
				}
			}
			provisionalStatus[response.Status] = false
		}
		return
	}
	seenStatus[response.Status] = true
	provisionalStatus[response.Status] = provisional
	cf.Responses = append(cf.Responses, response)
}

func (a *Analyzer) addJSONResponse(
	cf *CodeFacts,
	seenStatus map[uint16]bool,
	provisionalStatus map[uint16]bool,
	response facts.ResponseFact,
	route routes.Route,
	diags *diag.Accumulator,
	file string,
	line uint32,
) {
	if !seenStatus[response.Status] {
		a.addResponse(cf, seenStatus, provisionalStatus, response, false)
		return
	}
	idx := responseIndex(cf.Responses, response.Status)
	if idx < 0 {
		return
	}
	if provisionalStatus[response.Status] {
		cf.Responses[idx] = response
		provisionalStatus[response.Status] = false
		return
	}
	if response.Status < 200 || response.Status > 299 {
		return
	}
	existing := cf.Responses[idx]
	existingPriority := successResponsePriority(existing)
	newPriority := successResponsePriority(response)
	switch {
	case newPriority > existingPriority:
		cf.Responses[idx] = response
	case newPriority == existingPriority && newPriority >= 3 && response.Body != nil && existing.Body != nil && response.Body.RefID != existing.Body.RefID:
		if diags != nil {
			diags.ResponseSchemaUnresolved(route.Method, untypedRouteLabel(route), route.Handler, "conflicting typed success responses for "+route.Handler+" status "+strconv.FormatUint(uint64(response.Status), 10)+"; keeping first response body "+existing.Body.RefID+" (GO-05)", file, line)
		}
	}
}

func responseIndex(responses []facts.ResponseFact, status uint16) int {
	for i := range responses {
		if responses[i].Status == status {
			return i
		}
	}
	return -1
}

func successResponsePriority(response facts.ResponseFact) int {
	if response.Body == nil {
		return 0
	}
	if isErrorishRef(response.Body.RefID) {
		return 1
	}
	if strings.HasPrefix(response.Body.RefID, "__synthetic.") {
		return 2
	}
	return 3
}

func isErrorishRef(ref string) bool {
	parts := strings.Split(ref, ".")
	name := strings.ToLower(parts[len(parts)-1])
	return strings.Contains(name, "error") || strings.Contains(name, "problem") || strings.Contains(name, "failure")
}

func responseContentType(contentType string) string {
	if contentType == "" {
		return "application/octet-stream"
	}
	return contentType
}

func responseContentTypes(contentType string) []string {
	if contentType == "" {
		return nil
	}
	return []string{contentType}
}

func (a *Analyzer) stringValueOf(h handlerDecl, expr ast.Expr) (string, bool) {
	return a.stringValueOfSeen(h, expr, map[string]bool{})
}

func (a *Analyzer) callStringArg(h handlerDecl, call *ast.CallExpr, index int) (string, bool) {
	if call == nil || index < 0 || index >= len(call.Args) {
		return "", false
	}
	return a.stringValueOf(h, call.Args[index])
}

func (a *Analyzer) stringValueOfSeen(h handlerDecl, expr ast.Expr, seen map[string]bool) (string, bool) {
	if lit, ok := expr.(*ast.BasicLit); ok && lit.Kind == token.STRING {
		value, err := strconv.Unquote(lit.Value)
		if err == nil {
			return value, true
		}
		return strings.Trim(lit.Value, "`\""), true
	}
	if tv, ok := h.info.Types[expr]; ok && tv.Value != nil && tv.Value.Kind() == constant.String {
		return constant.StringVal(tv.Value), true
	}
	if ident := leafIdent(expr); ident != nil {
		if c, ok := h.info.ObjectOf(ident).(*gotypes.Const); ok && c.Val() != nil && c.Val().Kind() == constant.String {
			return constant.StringVal(c.Val()), true
		}
	}
	call, ok := expr.(*ast.CallExpr)
	if !ok || len(call.Args) != 0 {
		return "", false
	}
	if _, ok := call.Fun.(*ast.Ident); !ok {
		return "", false
	}
	fn := calledFuncObject(h.info, call.Fun)
	callee, ok := a.moduleOwnedCallee(fn)
	if !ok {
		return "", false
	}
	key := funcObjectKey(fn)
	if key == "" || seen[key] {
		return "", false
	}
	seen[key] = true
	if len(callee.decl.Body.List) != 1 {
		return "", false
	}
	ret, ok := callee.decl.Body.List[0].(*ast.ReturnStmt)
	if !ok || len(ret.Results) != 1 {
		return "", false
	}
	return a.stringValueOfSeen(callee, ret.Results[0], seen)
}

func calledFuncObject(info *gotypes.Info, fun ast.Expr) *gotypes.Func {
	if info == nil {
		return nil
	}
	switch f := fun.(type) {
	case *ast.IndexExpr:
		return calledFuncObject(info, f.X)
	case *ast.IndexListExpr:
		return calledFuncObject(info, f.X)
	case *ast.Ident:
		fn, _ := info.ObjectOf(f).(*gotypes.Func)
		return fn
	case *ast.SelectorExpr:
		if sel := info.Selections[f]; sel != nil {
			fn, _ := sel.Obj().(*gotypes.Func)
			return fn
		}
		fn, _ := info.ObjectOf(f.Sel).(*gotypes.Func)
		return fn
	default:
		return nil
	}
}

func (a *Analyzer) rawJSONRequestSchema(h handlerDecl, handler string) *facts.SchemaFact {
	if !handlerUsesRawJSONBody(h) {
		return nil
	}
	id, name := syntheticRawJSONRequestSchemaIdentity(handler)
	return &facts.SchemaFact{
		ID:   id,
		Name: name,
		Body: facts.AnyType(),
		Span: spanOf(h.fset, declPos(h.decl)),
	}
}

func handlerUsesRawJSONBody(h handlerDecl) bool {
	if h.decl == nil || h.decl.Body == nil {
		return false
	}
	rawVars := rawDataVars(h)
	if len(rawVars) == 0 {
		return false
	}
	if rawDataUsedByEncodingJSON(h, rawVars) {
		return true
	}
	return hasJSONContentTypeEvidence(h)
}

func rawDataVars(h handlerDecl) map[gotypes.Object]bool {
	out := map[gotypes.Object]bool{}
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		switch node := n.(type) {
		case *ast.AssignStmt:
			for i, rhs := range node.Rhs {
				if i >= len(node.Lhs) || !isGinGetRawDataCall(h.info, rhs) {
					continue
				}
				if id, ok := node.Lhs[i].(*ast.Ident); ok {
					if obj := h.info.ObjectOf(id); obj != nil {
						out[obj] = true
					}
				}
			}
		case *ast.ValueSpec:
			for i, rhs := range node.Values {
				if i >= len(node.Names) || !isGinGetRawDataCall(h.info, rhs) {
					continue
				}
				if obj := h.info.ObjectOf(node.Names[i]); obj != nil {
					out[obj] = true
				}
			}
		}
		return true
	})
	return out
}

func isGinGetRawDataCall(info *gotypes.Info, expr ast.Expr) bool {
	call, ok := expr.(*ast.CallExpr)
	if !ok {
		return false
	}
	name, recvPkg, ok := routes.GinMethod(info, call)
	return ok && recvPkg == routes.GinPkgPath && name == "GetRawData"
}

func rawDataUsedByEncodingJSON(h handlerDecl, rawVars map[gotypes.Object]bool) bool {
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		fn := calledFuncObject(h.info, call.Fun)
		if fn == nil || fn.Pkg() == nil || fn.Pkg().Path() != "encoding/json" {
			return true
		}
		switch fn.Name() {
		case "Unmarshal", "Valid":
			if len(call.Args) > 0 && exprUsesObject(h.info, call.Args[0], rawVars) {
				found = true
				return false
			}
		case "Compact", "Indent", "HTMLEscape":
			if len(call.Args) > 1 && exprUsesObject(h.info, call.Args[1], rawVars) {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

func exprUsesObject(info *gotypes.Info, expr ast.Expr, targets map[gotypes.Object]bool) bool {
	found := false
	ast.Inspect(expr, func(n ast.Node) bool {
		if found {
			return false
		}
		id, ok := n.(*ast.Ident)
		if !ok {
			return true
		}
		if targets[info.ObjectOf(id)] {
			found = true
			return false
		}
		return true
	})
	return found
}

func hasJSONContentTypeEvidence(h handlerDecl) bool {
	found := false
	ast.Inspect(h.decl.Body, func(n ast.Node) bool {
		if found {
			return false
		}
		if expr, ok := n.(*ast.BinaryExpr); ok {
			if exprMentionsGinContentType(h.info, expr) && exprHasStringLiteral(expr, "application/json") {
				found = true
				return false
			}
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		if callSubtreeHasStringLiteral(call, "application/json") && exprMentionsGinContentType(h.info, call) {
			found = true
			return false
		}
		return true
	})
	return found
}

func exprMentionsGinContentType(info *gotypes.Info, expr ast.Expr) bool {
	found := false
	ast.Inspect(expr, func(n ast.Node) bool {
		if found {
			return false
		}
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := routes.GinMethod(info, call)
		if ok && recvPkg == routes.GinPkgPath && name == "ContentType" {
			found = true
			return false
		}
		if ok && recvPkg == routes.GinPkgPath && name == "GetHeader" {
			key, ok := stringArg(call, 0)
			if ok && strings.EqualFold(key, "Content-Type") {
				found = true
				return false
			}
		}
		return true
	})
	return found
}

func exprHasStringLiteral(expr ast.Expr, value string) bool {
	return callSubtreeHasStringLiteral(expr, value)
}

func callSubtreeHasStringLiteral(root ast.Node, value string) bool {
	found := false
	ast.Inspect(root, func(n ast.Node) bool {
		if found {
			return false
		}
		lit, ok := n.(*ast.BasicLit)
		if !ok || lit.Kind != token.STRING {
			return true
		}
		unquoted, err := strconv.Unquote(lit.Value)
		if err == nil && strings.EqualFold(unquoted, value) {
			found = true
			return false
		}
		return true
	})
	return found
}

func isByteSlice(t gotypes.Type) bool {
	if t == nil {
		return false
	}
	if named, ok := gotypes.Unalias(t).(*gotypes.Named); ok {
		t = named.Underlying()
	}
	slice, ok := gotypes.Unalias(t).(*gotypes.Slice)
	if !ok {
		return false
	}
	basic, ok := gotypes.Unalias(slice.Elem()).(*gotypes.Basic)
	return ok && basic.Kind() == gotypes.Uint8
}

func (a *Analyzer) syntheticJSONResponse(
	h handlerDecl,
	expr ast.Expr,
	handler string,
	status uint16,
) (string, facts.SchemaFact, bool) {
	if schema, ok := a.arrayResponseSchema(h.info.TypeOf(expr), handler, status, h.fset, expr.Pos()); ok {
		return schema.ID, schema, true
	}
	if !isGinH(h.info.TypeOf(expr)) {
		return "", facts.SchemaFact{}, false
	}
	schema := a.ginHResponseSchema(h, expr, handler, status)
	return schema.ID, schema, true
}

func (a *Analyzer) arrayResponseSchema(
	t gotypes.Type,
	handler string,
	status uint16,
	fset *token.FileSet,
	pos token.Pos,
) (facts.SchemaFact, bool) {
	var elem gotypes.Type
	switch u := gotypes.Unalias(t).(type) {
	case *gotypes.Slice:
		elem = u.Elem()
	case *gotypes.Array:
		elem = u.Elem()
	default:
		return facts.SchemaFact{}, false
	}
	elemType, ok := a.responseType(elem)
	if !ok {
		return facts.SchemaFact{}, false
	}
	id, name := syntheticResponseSchemaIdentity(handler, status)
	return facts.SchemaFact{
		ID:   id,
		Name: name,
		Body: facts.ArrayType(elemType),
		Span: spanOf(fset, pos),
	}, true
}

func (a *Analyzer) ginHResponseSchema(
	h handlerDecl,
	expr ast.Expr,
	handler string,
	status uint16,
) facts.SchemaFact {
	id, name := syntheticResponseSchemaIdentity(handler, status)
	body := facts.MapTypeOf(facts.PrimitiveType(facts.StringPrim()), facts.AnyType())
	if lit, ok := expr.(*ast.CompositeLit); ok {
		if fields, ok := ginHLiteralFields(lit); ok {
			body = facts.ObjectType(fields)
		}
	}
	return facts.SchemaFact{
		ID:   id,
		Name: name,
		Body: body,
		Span: spanOf(h.fset, expr.Pos()),
	}
}

func ginHLiteralFields(lit *ast.CompositeLit) ([]facts.FieldFact, bool) {
	fields := make([]facts.FieldFact, 0, len(lit.Elts))
	for _, elt := range lit.Elts {
		kv, ok := elt.(*ast.KeyValueExpr)
		if !ok {
			return nil, false
		}
		key, ok := stringKey(kv.Key)
		if !ok {
			return nil, false
		}
		schema, ok := literalSchema(kv.Value)
		if !ok {
			return nil, false
		}
		fields = append(fields, facts.FieldFact{
			JSONName:                  key,
			SerializerMayOmit:         false,
			DeserializerAcceptsAbsent: false,
			DeserializerAcceptsNull:   false,
			SerializerMayEmitNull:     false,
			ValidatorRequiresPresence: false,
			ValidatorRejectsNull:      false,
			Schema:                    schema,
		})
	}
	return fields, true
}

func stringKey(expr ast.Expr) (string, bool) {
	lit, ok := expr.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", false
	}
	value, err := strconv.Unquote(lit.Value)
	if err != nil {
		return "", false
	}
	return value, true
}

func literalSchema(expr ast.Expr) (facts.Type, bool) {
	switch value := expr.(type) {
	case *ast.BasicLit:
		switch value.Kind {
		case token.STRING:
			return facts.PrimitiveType(facts.StringPrim()), true
		case token.INT:
			return facts.PrimitiveType(facts.IntPrim(64, true)), true
		case token.FLOAT:
			return facts.PrimitiveType(facts.FloatPrim(64)), true
		}
	case *ast.Ident:
		if value.Name == "true" || value.Name == "false" {
			return facts.PrimitiveType(facts.BoolPrim()), true
		}
	}
	return facts.Type{}, false
}

func (a *Analyzer) responseType(t gotypes.Type) (facts.Type, bool) {
	if t == nil {
		return facts.Type{}, false
	}
	if p, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = p.Elem()
	}
	if id, ok := a.namedTypeID(t); ok {
		return facts.NamedType(id), true
	}
	if basic, ok := gotypes.Unalias(t).(*gotypes.Basic); ok {
		switch basic.Kind() {
		case gotypes.String:
			return facts.PrimitiveType(facts.StringPrim()), true
		case gotypes.Bool:
			return facts.PrimitiveType(facts.BoolPrim()), true
		case gotypes.Int, gotypes.Int8, gotypes.Int16, gotypes.Int32, gotypes.Int64:
			return facts.PrimitiveType(facts.IntPrim(64, true)), true
		case gotypes.Uint, gotypes.Uint8, gotypes.Uint16, gotypes.Uint32, gotypes.Uint64:
			return facts.PrimitiveType(facts.IntPrim(64, false)), true
		case gotypes.Float32:
			return facts.PrimitiveType(facts.FloatPrim(32)), true
		case gotypes.Float64:
			return facts.PrimitiveType(facts.FloatPrim(64)), true
		}
	}
	return facts.Type{}, false
}

func isGinH(t gotypes.Type) bool {
	named, ok := gotypes.Unalias(t).(*gotypes.Named)
	if !ok || named.Obj() == nil || named.Obj().Pkg() == nil {
		return false
	}
	return named.Obj().Pkg().Path() == routes.GinPkgPath && named.Obj().Name() == "H"
}

// statusOf resolves an HTTP status argument to its numeric value via go/constant.
// Handles http.StatusXxx selectors and any const expression whose value is an int.
//
// The resolved value is bounds-checked to a sane HTTP status range (100..=599)
// before narrowing to uint16: a non-exact or out-of-range constant (e.g.
// c.JSON(70000, x), which would otherwise wrap silently to 4464) returns
// ok=false so the caller emits a dynamic-response diagnostic instead of emitting
// a corrupted status (GO-06).
func statusOf(info *gotypes.Info, arg ast.Expr) (uint16, bool) {
	tv, ok := info.Types[arg]
	if ok && tv.Value != nil && tv.Value.Kind() == constant.Int {
		if v, exact := constant.Int64Val(tv.Value); exact {
			return httpStatusInRange(v)
		}
	}
	// Fallback: resolve the identifier to a *types.Const and read its value.
	ident := leafIdent(arg)
	if ident == nil {
		return 0, false
	}
	c, ok := info.ObjectOf(ident).(*gotypes.Const)
	if !ok || c.Val() == nil || c.Val().Kind() != constant.Int {
		return 0, false
	}
	v, exact := constant.Int64Val(c.Val())
	if !exact {
		return 0, false
	}
	return httpStatusInRange(v)
}

// httpStatusInRange narrows a resolved int64 status to uint16 only when it is a
// valid HTTP status code (100..=599); otherwise it reports ok=false so the
// out-of-range value is diagnosed as a dynamic response rather than truncated.
func httpStatusInRange(v int64) (uint16, bool) {
	if v < 100 || v > 599 {
		return 0, false
	}
	return uint16(v), true
}

// namedTypeID returns the package-qualified schema id of a named (struct/enum)
// type, matching the schema-id format from 02-01 so the ref points at the same
// schema. Pointers/composite-literal types unwrap to their named type.
func (a *Analyzer) namedTypeID(t gotypes.Type) (string, bool) {
	if t == nil {
		return "", false
	}
	if p, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = p.Elem()
	}
	named, ok := gotypes.Unalias(t).(*gotypes.Named)
	if !ok {
		return "", false
	}
	obj := named.Obj()
	if obj == nil || obj.Pkg() == nil {
		return "", false
	}
	return a.qualifiedSchemaID(obj.Pkg().Path(), obj.Name()), true
}

// qualifiedSchemaID mirrors types.schemaID exactly so a handler-inferred request/
// response ref points at the same schema the type extractor (02-01) emitted: the
// module path prefix is trimmed to a module-relative package path, then
// ".TypeName" (e.g. "internal/common/dto.CreateGoalInput"). The module prefix is
// read from the Analyzer's per-invocation context (WR-03), not a package global.
func (a *Analyzer) qualifiedSchemaID(pkgPath, name string) string {
	rel := pkgPath
	if a.modulePrefix != "" && strings.HasPrefix(pkgPath, a.modulePrefix) {
		rel = strings.TrimPrefix(pkgPath, a.modulePrefix)
		rel = strings.TrimPrefix(rel, "/")
	}
	if rel == "" {
		return name
	}
	return rel + "." + name
}

// --- param builders ------------------------------------------------------

func pathParam(name string, fset *token.FileSet, pos token.Pos) facts.ParamFact {
	return facts.ParamFact{
		Name:     name,
		Location: "path",
		Required: true,
		Schema:   facts.PrimitiveType(facts.StringPrim()),
		Span:     spanOf(fset, pos),
	}
}

// queryParam builds a query parameter from a c.Query("name") read. Type defaults
// to string and required starts false; queryRequiredness may replace that initial
// value only when native control flow proves it (CLAUDE.md rules 1 & 3).
func queryParam(name string, fset *token.FileSet, pos token.Pos) facts.ParamFact {
	return facts.ParamFact{
		Name:     name,
		Location: "query",
		Required: false,
		Schema:   facts.PrimitiveType(facts.StringPrim()),
		Span:     spanOf(fset, pos),
	}
}

func queryParamFromGinCall(info *gotypes.Info, method, name string, fset *token.FileSet, call *ast.CallExpr) facts.ParamFact {
	param := queryParam(name, fset, call.Pos())
	if method == "DefaultQuery" {
		param.Default = queryDefaultValue(info, call)
	}
	return param
}

func requestParameter(
	name string,
	location string,
	required bool,
	schema facts.Type,
	fset *token.FileSet,
	pos token.Pos,
) facts.ParamFact {
	return facts.ParamFact{
		Name:     name,
		Location: location,
		Required: required,
		Schema:   schema,
		Span:     spanOf(fset, pos),
	}
}

func (a *Analyzer) addExtractedParameter(
	cf *CodeFacts,
	seen map[string]bool,
	resolved map[string]bool,
	param facts.ParamFact,
	isResolved bool,
	route routes.Route,
	diags *diag.Accumulator,
) {
	if param.Location == "header" && isOpenAPIRepresentationHeader(param.Name) {
		if strings.EqualFold(param.Name, "Authorization") {
			key := param.Location + "/authorization"
			if !seen[key] && diags != nil {
				seen[key] = true
				diags.AuthorizationSecurityMissing(
					route.Method,
					untypedRouteLabel(route),
					param.Span.File,
					param.Span.StartLine,
				)
			}
		}
		return
	}
	key := param.Location + "/" + param.Name
	wasResolved := resolved[key]
	if isResolved {
		resolved[key] = true
	}
	if !seen[key] {
		seen[key] = true
		cf.Params = append(cf.Params, param)
		return
	}
	index := -1
	for i := range cf.Params {
		if cf.Params[i].Location == param.Location && cf.Params[i].Name == param.Name {
			index = i
			break
		}
	}
	if index < 0 {
		cf.Params = append(cf.Params, param)
		return
	}
	existing := &cf.Params[index]
	// Resolution and schema specificity are independent facts about one parameter:
	// the read that settles requiredness may still carry the bare `string` schema
	// (a direct c.Query proved by control flow), while the read that leaves
	// requiredness open may be the one that proved the type (strconv.Atoi around
	// that same c.Query). Letting either fact overwrite the other loses one of them.
	if !wasResolved && isResolved {
		if parameterSchemaSpecificity(existing.Schema) > parameterSchemaSpecificity(param.Schema) {
			param.Schema = existing.Schema
			param.Span = existing.Span
		}
		param.Default = firstLiteral(param.Default, existing.Default)
		if param.Style == "" {
			param.Style = existing.Style
			param.Explode = existing.Explode
		}
		*existing = param
		return
	}
	if wasResolved && !isResolved {
		if parameterSchemaSpecificity(param.Schema) > parameterSchemaSpecificity(existing.Schema) {
			existing.Schema = param.Schema
			existing.Span = param.Span
		}
		if existing.Default == nil {
			existing.Default = param.Default
		}
		if existing.Style == "" {
			existing.Style = param.Style
			existing.Explode = param.Explode
		}
		return
	}
	if !reflect.DeepEqual(existing.Schema, param.Schema) {
		existingSpecificity := parameterSchemaSpecificity(existing.Schema)
		incomingSpecificity := parameterSchemaSpecificity(param.Schema)
		switch {
		case incomingSpecificity > existingSpecificity:
			existing.Schema = param.Schema
			existing.Span = param.Span
		case incomingSpecificity < existingSpecificity:
			// A raw string access is compatible evidence for a value that a
			// surrounding parser or typed binder refines more precisely: the
			// refined schema stays, and the merges below still take what the raw
			// read proved about requiredness, defaults, and serialization. Return
			// here instead and the answer depends on which read the walk reached
			// first, which is not a property of the handler.
		default:
			if diags != nil {
				diags.RequestParameterUnresolved(
					param.Name,
					route.Method,
					untypedRouteLabel(route),
					"conflicting extracted schemas for "+key,
					param.Span.File,
					param.Span.StartLine,
				)
			}
			return
		}
	}
	existing.Required = existing.Required || param.Required
	if existing.Default == nil {
		existing.Default = param.Default
	} else if param.Default != nil && !reflect.DeepEqual(existing.Default, param.Default) && diags != nil {
		diags.RequestParameterUnresolved(
			param.Name,
			route.Method,
			untypedRouteLabel(route),
			"conflicting extracted defaults for "+key,
			param.Span.File,
			param.Span.StartLine,
		)
	}
	if existing.Style == "" {
		existing.Style = param.Style
	} else if param.Style != "" && existing.Style != param.Style && diags != nil {
		diags.RequestParameterUnresolved(
			param.Name,
			route.Method,
			untypedRouteLabel(route),
			"conflicting extracted serialization styles for "+key,
			param.Span.File,
			param.Span.StartLine,
		)
	}
	if existing.Explode == nil {
		existing.Explode = param.Explode
	} else if param.Explode != nil && *existing.Explode != *param.Explode && diags != nil {
		diags.RequestParameterUnresolved(
			param.Name,
			route.Method,
			untypedRouteLabel(route),
			"conflicting extracted explode values for "+key,
			param.Span.File,
			param.Span.StartLine,
		)
	}
	existing.AllowReserved = existing.AllowReserved || param.AllowReserved
}

// OpenAPI defines Accept, Content-Type, and Authorization through response/request media types and
// security schemes. Emitting implementation reads of those headers as ordinary parameters creates a
// duplicate, contradictory public contract (and Content-Type is explicitly ignored as a Header
// Parameter by the specification).
func isOpenAPIRepresentationHeader(name string) bool {
	return strings.EqualFold(name, "Accept") ||
		strings.EqualFold(name, "Content-Type") ||
		strings.EqualFold(name, "Authorization")
}

func parameterSchemaSpecificity(schema facts.Type) int {
	if isPrimitiveStringType(schema) || schema.Type == facts.TypeAny {
		return 0
	}
	return 1
}

func parameterFromGinAccess(
	info *gotypes.Info,
	method string,
	name string,
	fset *token.FileSet,
	call *ast.CallExpr,
	hint parameterHint,
) facts.ParamFact {
	schema := facts.PrimitiveType(facts.StringPrim())
	required := false
	param := requestParameter(name, "query", required, schema, fset, call.Pos())
	switch method {
	case "DefaultQuery":
		param.Default = queryDefaultValue(info, call)
	case "QueryArray", "GetQueryArray":
		param.Schema = facts.ArrayType(facts.PrimitiveType(facts.StringPrim()))
		param.Style = "form"
		param.Explode = boolPointer(true)
	case "QueryMap":
		param.Schema = facts.MapTypeOf(
			facts.PrimitiveType(facts.StringPrim()),
			facts.PrimitiveType(facts.StringPrim()),
		)
		param.Style = "deepObject"
		param.Explode = boolPointer(true)
	}
	if hint.schemaKnown && hint.schema != nil {
		param.Schema = *hint.schema
	}
	if hint.requiredKnown {
		param.Required = hint.required
	}
	if hint.defaultValue != nil {
		param.Default = hint.defaultValue
	}
	if param.Schema.Type == facts.TypeArray && param.Style == "" {
		param.Style = "form"
		param.Explode = boolPointer(true)
	}
	return param
}

func boolPointer(value bool) *bool {
	return &value
}

func (a *Analyzer) addBoundParameters(
	frame helperFrame,
	call *ast.CallExpr,
	location string,
	cf *CodeFacts,
	seen map[string]bool,
	resolved map[string]bool,
	route routes.Route,
	diags *diag.Accumulator,
) {
	bound := boundTypeFromCall(frame, call)
	params, ok := a.parametersFromBoundType(bound, location, frame.decl.fset, route, diags, map[string]bool{})
	if !ok {
		if diags != nil {
			file, line := positionOf(frame.decl.fset, call.Pos())
			diags.RequestParameterUnresolved(
				selectorName(call.Fun),
				route.Method,
				untypedRouteLabel(route),
				"unable to resolve "+location+" binding DTO",
				file,
				line,
			)
		}
		return
	}
	for _, param := range params {
		a.addExtractedParameter(cf, seen, resolved, param, true, route, diags)
	}
}

func (a *Analyzer) parametersFromBoundType(
	t gotypes.Type,
	location string,
	fset *token.FileSet,
	route routes.Route,
	diags *diag.Accumulator,
	seen map[string]bool,
) ([]facts.ParamFact, bool) {
	if t == nil {
		return nil, false
	}
	if pointer, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = pointer.Elem()
	}
	if named, ok := gotypes.Unalias(t).(*gotypes.Named); ok {
		key := named.String()
		if seen[key] {
			return nil, true
		}
		seen[key] = true
		t = named.Underlying()
	}
	structure, ok := gotypes.Unalias(t).(*gotypes.Struct)
	if !ok {
		return nil, false
	}
	out := []facts.ParamFact{}
	for index := 0; index < structure.NumFields(); index++ {
		field := structure.Field(index)
		if field.Embedded() {
			nested, nestedOK := a.parametersFromBoundType(field.Type(), location, fset, route, diags, seen)
			if nestedOK {
				out = append(out, nested...)
			}
			continue
		}
		tag := reflect.StructTag(structure.Tag(index))
		name, options, skip := parameterWireName(tag, field.Name(), location)
		if skip {
			continue
		}
		file, line := positionOf(fset, field.Pos())
		schema, schemaOK := a.parameterType(field.Type(), tag)
		if !schemaOK {
			if diags != nil {
				if location == "form" {
					diags.RequestBodyUnresolved(
						field.Name(),
						route.Method,
						untypedRouteLabel(route),
						"unsupported form binding field type: "+field.Type().String(),
						file,
						line,
					)
				} else {
					diags.RequestParameterUnresolved(
						field.Name(),
						route.Method,
						untypedRouteLabel(route),
						"unsupported "+location+" binding field type: "+field.Type().String(),
						file,
						line,
					)
				}
			}
			continue
		}
		schema = schemaWithParameterEnums(schema, tag, name, route, diags, file, line)
		param := requestParameter(
			name,
			location,
			// Field scope only: a `required` behind `dive`/`keys` constrains the values
			// inside a repeated parameter, not whether the parameter must be sent.
			tags.HasFieldToken(tag.Get("binding"), "required") ||
				tags.HasFieldToken(tag.Get("validate"), "required"),
			schema,
			fset,
			field.Pos(),
		)
		if defaultText, exists := parameterDefault(tag, options); exists {
			if schema.Type == facts.TypeArray || schema.Type == facts.TypeMap || schema.Type == facts.TypeObject {
				if diags != nil {
					if location == "form" {
						diags.RequestBodyUnresolved(
							name,
							route.Method,
							untypedRouteLabel(route),
							"structured form field default cannot be represented losslessly",
							file,
							line,
						)
					} else {
						diags.RequestParameterUnresolved(
							name,
							route.Method,
							untypedRouteLabel(route),
							"structured parameter default cannot be represented losslessly",
							file,
							line,
						)
					}
				}
			} else {
				param.Default = literalForParameter(defaultText, schema)
			}
		}
		if reason := applyParameterSerialization(&param, tag, options); reason != "" && diags != nil {
			diags.RequestParameterUnresolved(
				name,
				route.Method,
				untypedRouteLabel(route),
				reason,
				file,
				line,
			)
		}
		out = append(out, param)
	}
	return out, true
}

func parameterWireName(tag reflect.StructTag, fallback, location string) (string, []string, bool) {
	key := "form"
	if location == "header" {
		key = "header"
	}
	raw, ok := tag.Lookup(key)
	if !ok || raw == "" {
		return fallback, nil, false
	}
	parts := strings.Split(raw, ",")
	name := strings.TrimSpace(parts[0])
	if name == "-" {
		return "", nil, true
	}
	if name == "" {
		name = fallback
	}
	return name, parts[1:], false
}

func (a *Analyzer) parameterType(t gotypes.Type, tag reflect.StructTag) (facts.Type, bool) {
	if t == nil {
		return facts.Type{}, false
	}
	switch value := gotypes.Unalias(t).(type) {
	case *gotypes.Pointer:
		return a.parameterType(value.Elem(), tag)
	case *gotypes.Slice:
		element, ok := a.parameterType(value.Elem(), reflect.StructTag(""))
		if !ok {
			return facts.Type{}, false
		}
		return facts.ArrayType(element), true
	case *gotypes.Array:
		element, ok := a.parameterType(value.Elem(), reflect.StructTag(""))
		if !ok {
			return facts.Type{}, false
		}
		return facts.ArrayType(element), true
	case *gotypes.Map:
		key, keyOK := a.parameterType(value.Key(), reflect.StructTag(""))
		item, itemOK := a.parameterType(value.Elem(), reflect.StructTag(""))
		if !keyOK || !itemOK {
			return facts.Type{}, false
		}
		return facts.MapTypeOf(key, item), true
	case *gotypes.Named:
		object := value.Obj()
		if object != nil && object.Pkg() != nil {
			switch {
			case object.Pkg().Path() == "github.com/google/uuid" && object.Name() == "UUID":
				return facts.WellKnownType(facts.WellKnownUUID), true
			case object.Pkg().Path() == "time" && object.Name() == "Time":
				if tag.Get("time_format") == "2006-01-02" {
					return facts.WellKnownType(facts.WellKnownDate), true
				}
				return facts.WellKnownType(facts.WellKnownDateTime), true
			case object.Pkg().Path() == "mime/multipart" && object.Name() == "FileHeader":
				return facts.PrimitiveType(facts.BytesPrim()), true
			}
			if _, basic := gotypes.Unalias(value.Underlying()).(*gotypes.Basic); basic {
				if members := namedStringEnumMembers(value); len(members) > 0 {
					return facts.EnumType(members), true
				}
				return a.parameterType(value.Underlying(), tag)
			}
			if _, structure := gotypes.Unalias(value.Underlying()).(*gotypes.Struct); structure {
				return facts.NamedType(a.qualifiedSchemaID(object.Pkg().Path(), object.Name())), true
			}
		}
		return a.parameterType(value.Underlying(), tag)
	case *gotypes.Basic:
		switch value.Kind() {
		case gotypes.String:
			return facts.PrimitiveType(facts.StringPrim()), true
		case gotypes.Bool:
			return facts.PrimitiveType(facts.BoolPrim()), true
		case gotypes.Int8:
			return facts.PrimitiveType(facts.IntPrim(8, true)), true
		case gotypes.Int16:
			return facts.PrimitiveType(facts.IntPrim(16, true)), true
		case gotypes.Int32:
			return facts.PrimitiveType(facts.IntPrim(32, true)), true
		case gotypes.Int, gotypes.Int64:
			return facts.PrimitiveType(facts.IntPrim(64, true)), true
		case gotypes.Uint8:
			return facts.PrimitiveType(facts.IntPrim(8, false)), true
		case gotypes.Uint16:
			return facts.PrimitiveType(facts.IntPrim(16, false)), true
		case gotypes.Uint32:
			return facts.PrimitiveType(facts.IntPrim(32, false)), true
		case gotypes.Uint, gotypes.Uint64:
			return facts.PrimitiveType(facts.IntPrim(64, false)), true
		case gotypes.Float32:
			return facts.PrimitiveType(facts.FloatPrim(32)), true
		case gotypes.Float64:
			return facts.PrimitiveType(facts.FloatPrim(64)), true
		}
	}
	return facts.Type{}, false
}

func namedStringEnumMembers(named *gotypes.Named) []string {
	if named == nil || named.Obj() == nil || named.Obj().Pkg() == nil {
		return nil
	}
	basic, ok := gotypes.Unalias(named.Underlying()).(*gotypes.Basic)
	if !ok || basic.Kind() != gotypes.String {
		return nil
	}
	out := []string{}
	for _, name := range named.Obj().Pkg().Scope().Names() {
		constantObject, ok := named.Obj().Pkg().Scope().Lookup(name).(*gotypes.Const)
		if !ok || !gotypes.Identical(constantObject.Type(), named) || constantObject.Val() == nil || constantObject.Val().Kind() != constant.String {
			continue
		}
		out = append(out, constant.StringVal(constantObject.Val()))
	}
	return out
}

// enumTarget names the one schema an enum replaces. A bound parameter carries its
// facts in `facts.ParamFact.Schema` and nowhere else — unlike a schema field there
// is no constraint object standing beside it — so stating an enum means replacing a
// schema, and a rule that cannot name which schema it replaces cannot be applied.
type enumTarget int

const (
	// enumTargetNone is a rule that reaches no schema on this parameter.
	enumTargetNone enumTarget = iota
	// enumTargetSelf is the parameter's own schema.
	enumTargetSelf
	// enumTargetElement is an array's element or a map's value.
	enumTargetElement
)

// enumTargets is iterated instead of the grouping map, whose order Go randomizes.
//
// Only one of these can hold rules for any given parameter: enumTargetOf answers
// Self only for a scalar and Element only for a container, and the schema is one or
// the other. The loop does not lean on that, so a future target — a constrained map
// key, once a document can carry one — needs a line here and nothing else.
var enumTargets = [...]enumTarget{enumTargetSelf, enumTargetElement}

func (t enumTarget) label() string {
	switch t {
	case enumTargetSelf:
		return "the parameter itself"
	case enumTargetElement:
		return "the values inside the parameter"
	}
	return "nothing"
}

// parameterEnumRule is one enum a bound parameter's tag states, paired with the
// scope it was written at. text quotes the rule the way the author wrote it, tag key
// included, so a diagnostic about two of them points at two distinct places in the
// source even when the rules themselves are spelled identically.
type parameterEnumRule struct {
	scope  tags.Scope
	values []string
	text   string
}

// enumTargetOf resolves the one schema a rule written at the given scope replaces.
// Every scope has a single destination: nothing is attempted twice and nothing is
// recovered on failure, so the tag's own shape decides where its members land.
//
// A scope that reaches no schema is dropped in silence, which is already what a
// post-`dive` rule gets on a schema field. Two scopes reach nothing here. A `dive`
// on a scalar names a value the parameter does not have. `keys`…`endkeys` names one
// it does have, but an OpenAPI object key is a string and gnr8 does not yet emit
// `propertyNames` to constrain it, so lowering rejects a map whose key is anything
// but a plain string (`lower::is_openapi_map_key`) — a well-formed tag must not
// abort generation, so the rule is read and discarded until the document can carry
// it.
//
// Field scope on a container lowers to what the container holds. That is a rule
// rather than a rescue: `facts.Type` has no room for an enum beside an array or a
// map, so the members can only be describing the values, and reading them that way
// is one deterministic answer, not a choice between two.
func enumTargetOf(schema facts.Type, scope tags.Scope) enumTarget {
	container := schema.Type == facts.TypeArray || schema.Type == facts.TypeMap
	switch scope {
	case tags.ScopeField:
		if container {
			return enumTargetElement
		}
		return enumTargetSelf
	case tags.ScopeElement:
		if container {
			return enumTargetElement
		}
	}
	return enumTargetNone
}

func schemaWithEnum(schema facts.Type, values []string, target enumTarget) facts.Type {
	switch target {
	case enumTargetSelf:
		return facts.EnumType(values)
	case enumTargetElement:
		if schema.Type == facts.TypeArray {
			return facts.ArrayType(facts.EnumType(values))
		}
		if mapped, ok := schema.Of.(*facts.MapType); ok && mapped != nil {
			return facts.MapTypeOf(mapped.Key, facts.EnumType(values))
		}
	}
	return schema
}

// schemaWithParameterEnums places each enum a bound parameter's tag states onto the
// value that tag says it constrains.
//
// Two rules that land on the same value are a contradiction the extractor refuses to
// settle. Choosing between them would be a precedence rule, and a fact stated twice
// has no winner even when both spellings agree, so both are dropped and the
// parameter is reported rather than published with a guess.
func schemaWithParameterEnums(
	schema facts.Type,
	tag reflect.StructTag,
	name string,
	route routes.Route,
	diags *diag.Accumulator,
	file string,
	line uint32,
) facts.Type {
	stated := map[enumTarget][]parameterEnumRule{}
	for _, rule := range parameterEnumRules(tag) {
		target := enumTargetOf(schema, rule.scope)
		if target == enumTargetNone {
			continue
		}
		stated[target] = append(stated[target], rule)
	}
	for _, target := range enumTargets {
		rules := stated[target]
		switch {
		case len(rules) == 0:
			continue
		case len(rules) > 1:
			if diags != nil {
				diags.RequestParameterAmbiguous(
					name,
					route.Method,
					untypedRouteLabel(route),
					"enum stated more than once for "+target.label()+" ("+renderEnumRules(rules)+")",
					file,
					line,
				)
			}
		default:
			schema = schemaWithEnum(schema, rules[0].values, target)
		}
	}
	return schema
}

// parameterEnumRules reads every enum a bound parameter's tag states. `binding` and
// `validate` are one validation vocabulary written under two keys and the `enums`
// tag states the same fact a third way, so all three are read as peers: none of them
// outranks another, and stating the enum in two of them is a contradiction rather
// than a preference.
func parameterEnumRules(tag reflect.StructTag) []parameterEnumRule {
	rules := []parameterEnumRule{}
	for _, key := range []string{"binding", "validate"} {
		for _, token := range tags.Scoped(tag.Get(key)) {
			value, ok := strings.CutPrefix(token.Text, "oneof=")
			if !ok {
				continue
			}
			if values := strings.Fields(strings.ReplaceAll(value, "|", " ")); len(values) > 0 {
				rules = append(rules, parameterEnumRule{scope: token.Scope, values: values, text: quoteTagRule(key, token.Text)})
			}
		}
	}
	for _, key := range []string{"enums", "enum"} {
		value := tag.Get(key)
		if value == "" {
			continue
		}
		values := strings.FieldsFunc(value, func(r rune) bool { return r == ',' || r == '|' || r == ' ' })
		if len(values) > 0 {
			rules = append(rules, parameterEnumRule{scope: tags.ScopeField, values: values, text: quoteTagRule(key, value)})
		}
	}
	return rules
}

// quoteTagRule renders one rule as the author wrote it — `binding:"oneof=a b"` — so
// two rules that differ only in which tag key carries them still read as two.
func quoteTagRule(key, rule string) string {
	return key + `:"` + rule + `"`
}

func renderEnumRules(rules []parameterEnumRule) string {
	spellings := make([]string, 0, len(rules))
	for _, rule := range rules {
		spellings = append(spellings, rule.text)
	}
	return strings.Join(spellings, ", ")
}

func parameterDefault(tag reflect.StructTag, options []string) (string, bool) {
	if value, ok := tag.Lookup("default"); ok {
		return value, true
	}
	for _, option := range options {
		if value, ok := strings.CutPrefix(strings.TrimSpace(option), "default="); ok {
			return value, true
		}
	}
	return "", false
}

func literalForParameter(value string, schema facts.Type) *facts.LiteralValue {
	if value == "null" {
		return &facts.LiteralValue{Type: "null"}
	}
	if schema.Type == facts.TypePrimitive {
		if primitive, ok := schema.Of.(*facts.Prim); ok && primitive != nil {
			switch primitive.Prim {
			case facts.PrimBool:
				if parsed, err := strconv.ParseBool(value); err == nil {
					return &facts.LiteralValue{Type: "bool", Value: parsed}
				}
			case facts.PrimInt, facts.PrimFloat:
				if _, err := strconv.ParseFloat(value, 64); err == nil {
					return &facts.LiteralValue{Type: "number", Value: value}
				}
			}
		}
	}
	return &facts.LiteralValue{Type: "string", Value: value}
}

func applyParameterSerialization(param *facts.ParamFact, tag reflect.StructTag, options []string) string {
	if param == nil {
		return ""
	}
	param.Style = tag.Get("style")
	if value := tag.Get("explode"); value != "" {
		if parsed, err := strconv.ParseBool(value); err == nil {
			param.Explode = boolPointer(parsed)
		}
	}
	if value := tag.Get("allowReserved"); value != "" {
		param.AllowReserved, _ = strconv.ParseBool(value)
	}
	collectionFormat := tag.Get("collection_format")
	for _, option := range options {
		if value, ok := strings.CutPrefix(strings.TrimSpace(option), "collection_format="); ok {
			collectionFormat = value
		}
	}
	if param.Schema.Type == facts.TypeArray {
		if param.Location == "query" {
			expectedStyle, expectedExplode, supported := queryCollectionSerialization(collectionFormat)
			if !supported {
				return "query array collection format " + strconv.Quote(collectionFormat) + " cannot be represented by the generated OpenAPI/SDK wire contract"
			}
			if param.Style == "" {
				param.Style = expectedStyle
			} else if collectionFormat != "" && param.Style != expectedStyle {
				return "query array style " + strconv.Quote(param.Style) + " conflicts with collection format " + strconv.Quote(collectionFormat)
			}
			if param.Explode == nil {
				param.Explode = boolPointer(expectedExplode)
			} else if collectionFormat != "" && *param.Explode != expectedExplode {
				return "query array explode value conflicts with collection format " + strconv.Quote(collectionFormat)
			}
		} else if param.Location == "header" {
			if collectionFormat != "" && collectionFormat != "csv" {
				return "header array collection format " + strconv.Quote(collectionFormat) + " cannot be represented by OpenAPI simple serialization"
			}
			if param.Style == "" {
				param.Style = "simple"
			}
			if param.Explode == nil {
				param.Explode = boolPointer(false)
			}
		}
	}
	return ""
}

func queryCollectionSerialization(collectionFormat string) (style string, explode bool, supported bool) {
	switch collectionFormat {
	case "", "multi":
		return "form", true, true
	case "csv":
		return "form", false, true
	case "ssv":
		return "spaceDelimited", false, true
	case "pipes":
		return "pipeDelimited", false, true
	default:
		return "", false, false
	}
}

// untypedRouteLabel renders the operation label (method + group-relative path)
// used in the untyped-query diagnostic. The machine-stable identity (param name +
// method + relative path) is preserved.
func untypedRouteLabel(route routes.Route) string {
	return route.Path
}

// --- expression helpers --------------------------------------------------

// stringArg returns the string value of the i-th call argument when it is a
// string literal.
func stringArg(call *ast.CallExpr, i int) (string, bool) {
	if i >= len(call.Args) {
		return "", false
	}
	lit, ok := call.Args[i].(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", false
	}
	return strings.Trim(lit.Value, "`\""), true
}

// leafIdent returns the trailing identifier of a selector/ident expression, e.g.
// `http.StatusCreated` -> the `StatusCreated` ident; `StatusOK` -> itself.
func leafIdent(e ast.Expr) *ast.Ident {
	switch x := e.(type) {
	case *ast.SelectorExpr:
		return x.Sel
	case *ast.Ident:
		return x
	}
	return nil
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
