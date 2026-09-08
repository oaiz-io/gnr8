// Package routes recognizes the Gin route table of the target module (GO-04).
//
// Recognition is SEMANTIC, not textual: for every `*ast.CallExpr` whose Fun is a
// selector (`x.METHOD(...)`), it resolves the method's identity through
// `go/types` `Info.Selections` and gates on the receiver package path
// (`github.com/gin-gonic/gin`) — so `import grouter "...gin"` aliasing or a
// different gin version is irrelevant (RESEARCH Pattern 2, Anti-Pattern: never
// string-match the import alias).
//
// The fixture registers routes as:
//
//	api := h.Router.Group("/" + basePath) // dynamic prefix -> not folded
//	api.Use(h.AuthMiddleware)             // secures the whole group (D-04/D-14)
//	api.POST("/", h.createGoal)           // group-relative "/"
//	api.GET("/list", h.listGoals)         // "/list"
//	api.PUT("/:uuid", h.updateGoal)       // "/:uuid" -> "/{uuid}"
//	api.DELETE("/:uuid", h.deleteGoal)    // "/:uuid" -> "/{uuid}"
//
// The group prefix is a non-constant `"/" + basePath`, so routes are recorded
// group-relative with Gin `:param` normalized to OpenAPI `{param}`; the concrete
// `/goal` prefix is supplied later by the Rust lowering layer (it is not scraped
// from any annotation — CLAUDE.md rule 1). No Gin terms leak into the emitted
// facts — only router-agnostic HTTP routes.
package routes

import (
	"go/ast"
	"go/constant"
	"go/token"
	gotypes "go/types"
	"strconv"
	"strings"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/load"
)

// GinPkgPath is the receiver package the recognizer gates on. Recognition keys on
// this resolved package path, never on the source identifier/alias (T-02-06).
// Exported so the handler analyzer (Task 2) shares the exact identity gate.
const GinPkgPath = "github.com/gin-gonic/gin"

// ginPkgPath is the unexported alias kept for in-package readability.
const ginPkgPath = GinPkgPath

// ginNamedHTTPMethods is the set of *gin.RouterGroup verb shortcuts.
var ginNamedHTTPMethods = map[string]bool{
	"GET": true, "POST": true, "PUT": true, "DELETE": true,
	"PATCH": true, "HEAD": true, "OPTIONS": true,
}

// openAPIOperationMethods is the standard-method set the graph can lower into
// OpenAPI Path Item operation slots. Gin has no named TRACE shortcut, but Handle
// and Match can state it exactly.
var openAPIOperationMethods = map[string]bool{
	"GET": true, "POST": true, "PUT": true, "DELETE": true,
	"PATCH": true, "HEAD": true, "OPTIONS": true, "TRACE": true,
}

// Route is one recognized HTTP route plus the handler symbol and enclosing group,
// so the handler analyzer can match a handler FuncDecl by symbol. Middleware/Secured
// record source middleware placement for later explicit config matching — pure code
// recognition, NOT an inferred security requirement: generated API security still
// comes from the user's gnr8 config (CLAUDE.md rule 4).
type Route struct {
	Method          string           // "POST"
	Path            string           // group-relative, normalized: "/", "/list", "/{uuid}"
	Handler         string           // selector name of the handler arg, e.g. "createGoal"
	HandlerKey      string           // resolved *types.Func key, e.g. "example.com/m/pkg@123"
	HandlerIdentity string           // human-readable package/receiver/name/file identity
	Group           string           // deepest static route group segment, e.g. "books"
	Middleware      []string         // source middleware symbols applied before the handler
	Secured         bool             // route/group carried middleware
	Span            facts.SourceSpan // provenance of the METHOD registration call
}

// Recognize walks every target package's syntax and returns the recognized routes
// with group security propagated. Output order is not guaranteed; callers sort
// (facts.Marshal) before emit.
func Recognize(res *load.Result) []Route {
	return RecognizeWithDiagnostics(res, nil)
}

// RecognizeWithDiagnostics is Recognize plus diagnostics for unsupported route
// registration patterns. Existing callers that only need routes can use
// Recognize; the CLI sidecar passes the accumulator so unsupported dynamic
// route paths are visible to the user.
func RecognizeWithDiagnostics(res *load.Result, diags *diag.Accumulator) []Route {
	var out []Route
	for _, pkg := range res.Packages {
		if pkg.TypesInfo == nil {
			continue
		}
		inheritedMiddleware := inferRouterGroupParameterMiddleware(pkg.Syntax, pkg.TypesInfo)
		for _, file := range pkg.Syntax {
			out = append(out, recognizeFile(file, pkg.TypesInfo, res.Fset, diags, inheritedMiddleware)...)
		}
	}
	return out
}

// groupInfo tracks the resolved static prefix for a group receiver. Middleware is
// resolved separately at each route's source position.
type groupInfo struct {
	prefix string
}

type rawGroup struct {
	parent           gotypes.Object
	parentPrefix     string
	parentMiddleware []string
	prefix           string
	middleware       []string
	createdAt        token.Pos
	static           bool
	span             facts.SourceSpan
}

type middlewareUse struct {
	pos     token.Pos
	symbols []string
}

// recognizeFile collects routes from a single file. It performs two passes so group
// declarations and middleware calls can be resolved before routes are emitted. Middleware
// follows Gin's statement-order semantics: Use only affects routes registered after it,
// and child groups snapshot their parent's middleware when the child is created.
func recognizeFile(
	file *ast.File,
	info *gotypes.Info,
	fset *token.FileSet,
	diags *diag.Accumulator,
	inheritedMiddleware map[gotypes.Object][]string,
) []Route {
	groups := map[gotypes.Object]*groupInfo{}
	rawGroups := map[gotypes.Object]rawGroup{}
	middlewareUses := map[gotypes.Object][]middlewareUse{}

	// Pass 1: find Group(...) assignments and Use(...) calls. Group prefix
	// resolution happens after the walk so nested groups are order-independent.
	ast.Inspect(file, func(n ast.Node) bool {
		recordGroupAssignment(info, rawGroups, fset, n)
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := ginMethod(info, call)
		if !ok || recvPkg != ginPkgPath || name != "Use" {
			return true
		}
		if obj := receiverObject(info, call); obj != nil {
			middlewareUses[obj] = append(middlewareUses[obj], middlewareUse{
				pos:     call.Pos(),
				symbols: middlewareSymbols(info, call.Args),
			})
		}
		return true
	})
	for obj, symbols := range inheritedMiddleware {
		if len(symbols) == 0 {
			continue
		}
		middlewareUses[obj] = append(middlewareUses[obj], middlewareUse{
			pos:     token.NoPos,
			symbols: symbols,
		})
	}
	for obj := range rawGroups {
		raw := rawGroups[obj]
		if diags != nil && !raw.static {
			diags.SourceRouteUnresolved("unsupported Gin route pattern: dynamic Gin group prefix; prefix skipped rather than guessed (GO-04)", raw.span.File, raw.span.StartLine)
		}
		g := groupOf(groups, obj)
		g.prefix = resolveGroupPrefix(obj, rawGroups, map[gotypes.Object]bool{})
	}

	// Pass 2: emit one Route per recognized METHOD(path, handler) call.
	var out []Route
	ast.Inspect(file, func(n ast.Node) bool {
		call, ok := n.(*ast.CallExpr)
		if !ok {
			return true
		}
		name, recvPkg, ok := ginMethod(info, call)
		if !ok || recvPkg != ginPkgPath {
			return true
		}
		registration, recognized := ginRouteRegistration(info, call, name, fset, diags)
		if !recognized {
			return true
		}
		if len(call.Args) <= registration.handlerIndex {
			return true // not a (path, handler) registration; skip defensively.
		}
		pathLit, ok := stringLiteral(call.Args[registration.pathIndex])
		if !ok {
			if diags != nil {
				span := spanOf(fset, call.Pos(), call.End())
				diags.UnsupportedRoutePattern("dynamic route path for "+registration.method+" registration", span.File, span.StartLine)
			}
			return true // dynamic path arg; not folded here.
		}
		handlerArg := call.Args[registration.handlerIndex]
		handler := handlerSymbol(handlerArg)
		if handler == "" {
			if diags != nil {
				span := spanOf(fset, call.Pos(), call.End())
				diags.UnsupportedRoutePattern("route handler for "+registration.method+" "+pathLit+" is not a named function or method", span.File, span.StartLine)
			}
			return true
		}
		handlerFn := HandlerFunc(info, handlerArg)

		prefix := receiverPrefix(info, groups, call)
		middleware := receiverMiddlewareAt(info, rawGroups, middlewareUses, call, call.Pos())
		if registration.middlewareStart < registration.handlerIndex {
			middleware = appendUniqueStrings(middleware, middlewareSymbols(info, call.Args[registration.middlewareStart:registration.handlerIndex])...)
		}
		secured := len(middleware) > 0
		if obj := receiverObject(info, call); obj != nil {
			if _, seen := groups[obj]; !seen && prefix == "" && receiverIsGinRouterGroup(info, call) && diags != nil {
				span := spanOf(fset, call.Pos(), call.End())
				diags.SourceRouteUnresolved("unsupported Gin route pattern: route registered on router group parameter; prefix cannot be inferred across helper calls, so the route is emitted relative (GO-04)", span.File, span.StartLine)
			}
		}

		out = append(out, Route{
			Method:          registration.method,
			Path:            joinPaths(prefix, normalizePath(pathLit)),
			Handler:         handler,
			HandlerKey:      FuncObjectKey(handlerFn),
			HandlerIdentity: FuncIdentity(handlerFn, fset),
			Group:           groupNameFromPrefix(prefix),
			Middleware:      middleware,
			Secured:         secured,
			Span:            spanOf(fset, call.Pos(), call.End()),
		})
		return true
	})
	return out
}

type routeRegistration struct {
	method          string
	pathIndex       int
	middlewareStart int
	handlerIndex    int
}

// ginRouteRegistration lowers the route shapes that state exactly one OpenAPI
// operation. The named verb methods and Handle have a single method/path/handler
// identity. Any and a multi-method Match do not: gnr8's native operation identity
// is the one routed handler symbol, so expanding either call would create several
// operations with the same identity. Those calls are diagnosed and skipped rather
// than silently disappearing or receiving invented operation names.
func ginRouteRegistration(
	info *gotypes.Info,
	call *ast.CallExpr,
	name string,
	fset *token.FileSet,
	diags *diag.Accumulator,
) (routeRegistration, bool) {
	last := len(call.Args) - 1
	if ginNamedHTTPMethods[name] {
		if len(call.Args) < 2 {
			return routeRegistration{}, false
		}
		return routeRegistration{method: name, pathIndex: 0, middlewareStart: 1, handlerIndex: last}, true
	}
	span := spanOf(fset, call.Pos(), call.End())
	switch name {
	case "Handle":
		if len(call.Args) < 3 {
			return routeRegistration{}, false
		}
		method, ok := constantString(info, call.Args[0])
		if !ok || !openAPIOperationMethods[method] {
			if diags != nil {
				reason := "dynamic HTTP method for Handle registration"
				if ok {
					reason = "HTTP method " + strconv.Quote(method) + " from Handle cannot be represented"
				}
				diags.UnsupportedRoutePattern(reason, span.File, span.StartLine)
			}
			return routeRegistration{}, false
		}
		return routeRegistration{method: method, pathIndex: 1, middlewareStart: 2, handlerIndex: last}, true
	case "Match":
		methods, ok := constantStringSlice(info, firstCallArg(call))
		if !ok || len(methods) != 1 {
			if diags != nil {
				diags.UnsupportedRoutePattern("Match registers zero or multiple operations for one handler identity", span.File, span.StartLine)
			}
			return routeRegistration{}, false
		}
		method := methods[0]
		if !openAPIOperationMethods[method] {
			if diags != nil {
				diags.UnsupportedRoutePattern("HTTP method "+strconv.Quote(method)+" from Match cannot be represented", span.File, span.StartLine)
			}
			return routeRegistration{}, false
		}
		return routeRegistration{method: method, pathIndex: 1, middlewareStart: 2, handlerIndex: last}, true
	case "Any":
		if diags != nil {
			diags.UnsupportedRoutePattern("Any registers multiple operations, including CONNECT, for one handler identity", span.File, span.StartLine)
		}
	case "StaticFile", "StaticFileFS", "Static", "StaticFS":
		if diags != nil {
			diags.UnsupportedRoutePattern(name+" registers framework-generated GET and HEAD operations without a source handler identity", span.File, span.StartLine)
		}
	}
	return routeRegistration{}, false
}

func firstCallArg(call *ast.CallExpr) ast.Expr {
	if call == nil || len(call.Args) == 0 {
		return nil
	}
	return call.Args[0]
}

func constantString(info *gotypes.Info, expr ast.Expr) (string, bool) {
	if info == nil || expr == nil {
		return "", false
	}
	if value, ok := info.Types[expr]; ok && value.Value != nil && value.Value.Kind() == constant.String {
		return constant.StringVal(value.Value), true
	}
	return stringLiteral(expr)
}

func constantStringSlice(info *gotypes.Info, expr ast.Expr) ([]string, bool) {
	literal, ok := expr.(*ast.CompositeLit)
	if !ok {
		return nil, false
	}
	out := make([]string, 0, len(literal.Elts))
	for _, element := range literal.Elts {
		value, ok := constantString(info, element)
		if !ok {
			return nil, false
		}
		out = append(out, value)
	}
	return out, true
}

// inferRouterGroupParameterMiddleware follows router-group arguments through
// package-local helper calls. Gin services commonly split route registration
// across helpers:
//
//	api.Use(h.AuthMiddleware)
//	h.registerJobs(api)
//
// The callee's `api *gin.RouterGroup` parameter is a different go/types object
// from the caller's local variable, so a file-local scan cannot otherwise see
// the middleware already attached at the call site. The fixed point also
// handles helpers that delegate to more helpers. Middleware is still derived
// exclusively from executable route setup; no documentation annotations are
// consulted.
func inferRouterGroupParameterMiddleware(files []*ast.File, info *gotypes.Info) map[gotypes.Object][]string {
	rawGroups := map[gotypes.Object]rawGroup{}
	middlewareUses := map[gotypes.Object][]middlewareUse{}

	for _, file := range files {
		ast.Inspect(file, func(n ast.Node) bool {
			recordGroupAssignment(info, rawGroups, nil, n)
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			name, recvPkg, ok := ginMethod(info, call)
			if !ok || recvPkg != ginPkgPath || name != "Use" {
				return true
			}
			if obj := receiverObject(info, call); obj != nil {
				middlewareUses[obj] = append(middlewareUses[obj], middlewareUse{
					pos:     call.Pos(),
					symbols: middlewareSymbols(info, call.Args),
				})
			}
			return true
		})
	}

	inherited := map[gotypes.Object][]string{}
	for {
		changed := false
		for _, file := range files {
			ast.Inspect(file, func(n ast.Node) bool {
				call, ok := n.(*ast.CallExpr)
				if !ok {
					return true
				}
				fn := calledFunc(info, call.Fun)
				if fn == nil {
					return true
				}
				signature, ok := gotypes.Unalias(fn.Type()).(*gotypes.Signature)
				if !ok {
					return true
				}
				limit := min(len(call.Args), signature.Params().Len())
				for i := 0; i < limit; i++ {
					param := signature.Params().At(i)
					if !isGinRouterGroupType(param.Type()) {
						continue
					}
					middleware := middlewareFromExprAt(
						info,
						rawGroups,
						middlewareUses,
						call.Args[i],
						call.Pos(),
					)
					before := len(inherited[param])
					inherited[param] = appendUniqueStrings(inherited[param], middleware...)
					if len(inherited[param]) == before {
						continue
					}
					changed = true
					middlewareUses[param] = append(middlewareUses[param], middlewareUse{
						pos:     token.NoPos,
						symbols: inherited[param],
					})
				}
				return true
			})
		}
		if !changed {
			return inherited
		}
	}
}

func calledFunc(info *gotypes.Info, expr ast.Expr) *gotypes.Func {
	switch e := expr.(type) {
	case *ast.Ident:
		fn, _ := info.ObjectOf(e).(*gotypes.Func)
		return fn
	case *ast.SelectorExpr:
		return selectedFunc(info, e)
	default:
		return nil
	}
}

func isGinRouterGroupType(t gotypes.Type) bool {
	t = gotypes.Unalias(t)
	if ptr, ok := t.(*gotypes.Pointer); ok {
		t = gotypes.Unalias(ptr.Elem())
	}
	named, ok := t.(*gotypes.Named)
	return ok &&
		named.Obj() != nil &&
		named.Obj().Pkg() != nil &&
		named.Obj().Pkg().Path() == ginPkgPath &&
		named.Obj().Name() == "RouterGroup"
}

func recordGroupAssignment(info *gotypes.Info, rawGroups map[gotypes.Object]rawGroup, fset *token.FileSet, n ast.Node) {
	switch node := n.(type) {
	case *ast.AssignStmt:
		for i, rhs := range node.Rhs {
			if i >= len(node.Lhs) {
				continue
			}
			obj := assignedIdentObject(info, node.Lhs[i])
			if obj == nil {
				continue
			}
			if group, ok := groupFromExpr(info, rhs, fset); ok {
				rawGroups[obj] = group
			}
		}
	case *ast.ValueSpec:
		for i, rhs := range node.Values {
			if i >= len(node.Names) {
				continue
			}
			obj := info.ObjectOf(node.Names[i])
			if obj == nil {
				continue
			}
			if group, ok := groupFromExpr(info, rhs, fset); ok {
				rawGroups[obj] = group
			}
		}
	}
}

func assignedIdentObject(info *gotypes.Info, expr ast.Expr) gotypes.Object {
	ident, ok := expr.(*ast.Ident)
	if !ok {
		return nil
	}
	return info.ObjectOf(ident)
}

func groupFromExpr(info *gotypes.Info, expr ast.Expr, fset *token.FileSet) (rawGroup, bool) {
	call, ok := expr.(*ast.CallExpr)
	if !ok {
		return rawGroup{}, false
	}
	name, recvPkg, ok := ginMethod(info, call)
	if !ok || recvPkg != ginPkgPath || name != "Group" {
		return rawGroup{}, false
	}
	prefix, static := "", false
	if len(call.Args) > 0 {
		prefix, static = stringLiteral(call.Args[0])
	}
	parentPrefix := ""
	var parentMiddleware []string
	if sel, ok := call.Fun.(*ast.SelectorExpr); ok {
		if parentGroup, ok := groupFromExpr(info, sel.X, fset); ok {
			parentPrefix = resolvedRawGroupPrefix(parentGroup, nil)
			parentMiddleware = appendUniqueStrings(parentMiddleware, parentGroup.parentMiddleware...)
			parentMiddleware = appendUniqueStrings(parentMiddleware, parentGroup.middleware...)
		}
	}
	var span facts.SourceSpan
	if fset != nil {
		span = spanOf(fset, call.Pos(), call.End())
	}
	return rawGroup{
		parent:           receiverObject(info, call),
		parentPrefix:     parentPrefix,
		parentMiddleware: parentMiddleware,
		prefix:           normalizePath(prefix),
		middleware:       middlewareSymbols(info, call.Args[1:]),
		createdAt:        call.Pos(),
		static:           static,
		span:             span,
	}, true
}

func resolveGroupPrefix(obj gotypes.Object, rawGroups map[gotypes.Object]rawGroup, visiting map[gotypes.Object]bool) string {
	if obj == nil || visiting[obj] {
		return ""
	}
	group, ok := rawGroups[obj]
	if !ok {
		return ""
	}
	visiting[obj] = true
	parent := resolveGroupPrefix(group.parent, rawGroups, visiting)
	delete(visiting, obj)
	return joinPaths(parent, resolvedRawGroupPrefix(group, nil))
}

func resolveGroupMiddlewareAt(
	obj gotypes.Object,
	rawGroups map[gotypes.Object]rawGroup,
	middlewareUses map[gotypes.Object][]middlewareUse,
	at token.Pos,
	visiting map[gotypes.Object]bool,
) []string {
	if obj == nil || visiting[obj] {
		return nil
	}
	raw, ok := rawGroups[obj]
	if !ok {
		var out []string
		for _, use := range middlewareUses[obj] {
			if use.pos < at {
				out = appendUniqueStrings(out, use.symbols...)
			}
		}
		return out
	}
	visiting[obj] = true
	var out []string
	out = appendUniqueStrings(out, resolveGroupMiddlewareAt(raw.parent, rawGroups, middlewareUses, raw.createdAt, visiting)...)
	out = appendUniqueStrings(out, raw.parentMiddleware...)
	out = appendUniqueStrings(out, raw.middleware...)
	for _, use := range middlewareUses[obj] {
		if use.pos < at {
			out = appendUniqueStrings(out, use.symbols...)
		}
	}
	delete(visiting, obj)
	return out
}

func receiverPrefix(info *gotypes.Info, groups map[gotypes.Object]*groupInfo, call *ast.CallExpr) string {
	if obj := receiverObject(info, call); obj != nil {
		if g, ok := groups[obj]; ok {
			return g.prefix
		}
	}
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return ""
	}
	return prefixFromExpr(info, groups, sel.X)
}

func prefixFromExpr(info *gotypes.Info, groups map[gotypes.Object]*groupInfo, expr ast.Expr) string {
	switch e := expr.(type) {
	case *ast.Ident:
		if obj := info.ObjectOf(e); obj != nil {
			if g, ok := groups[obj]; ok {
				return g.prefix
			}
		}
	case *ast.CallExpr:
		group, ok := groupFromExpr(info, e, nil)
		if !ok {
			return ""
		}
		parent := ""
		if group.parent != nil {
			if g, ok := groups[group.parent]; ok {
				parent = g.prefix
			}
		}
		return joinPaths(parent, resolvedRawGroupPrefix(group, nil))
	}
	return ""
}

func receiverMiddlewareAt(
	info *gotypes.Info,
	rawGroups map[gotypes.Object]rawGroup,
	middlewareUses map[gotypes.Object][]middlewareUse,
	call *ast.CallExpr,
	at token.Pos,
) []string {
	if obj := receiverObject(info, call); obj != nil {
		return resolveGroupMiddlewareAt(obj, rawGroups, middlewareUses, at, map[gotypes.Object]bool{})
	}
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return nil
	}
	return middlewareFromExprAt(info, rawGroups, middlewareUses, sel.X, at)
}

func middlewareFromExprAt(
	info *gotypes.Info,
	rawGroups map[gotypes.Object]rawGroup,
	middlewareUses map[gotypes.Object][]middlewareUse,
	expr ast.Expr,
	at token.Pos,
) []string {
	switch e := expr.(type) {
	case *ast.Ident:
		if obj := info.ObjectOf(e); obj != nil {
			return resolveGroupMiddlewareAt(obj, rawGroups, middlewareUses, at, map[gotypes.Object]bool{})
		}
	case *ast.CallExpr:
		group, ok := groupFromExpr(info, e, nil)
		if !ok {
			return nil
		}
		var out []string
		if group.parent != nil {
			out = appendUniqueStrings(out, resolveGroupMiddlewareAt(group.parent, rawGroups, middlewareUses, e.Pos(), map[gotypes.Object]bool{})...)
		}
		out = appendUniqueStrings(out, group.parentMiddleware...)
		return appendUniqueStrings(out, group.middleware...)
	}
	return nil
}

func resolvedRawGroupPrefix(group rawGroup, parentPrefix *string) string {
	prefix := group.parentPrefix
	if parentPrefix != nil {
		prefix = joinPaths(*parentPrefix, prefix)
	}
	if !group.static {
		return prefix
	}
	return joinPaths(prefix, group.prefix)
}

func groupNameFromPrefix(prefix string) string {
	prefix = normalizePath(prefix)
	for _, seg := range reverseSegments(prefix) {
		if seg == "" || seg == "/" || strings.HasPrefix(seg, "{") {
			continue
		}
		return seg
	}
	return ""
}

func reverseSegments(path string) []string {
	segs := strings.Split(strings.Trim(path, "/"), "/")
	for i, j := 0, len(segs)-1; i < j; i, j = i+1, j-1 {
		segs[i], segs[j] = segs[j], segs[i]
	}
	return segs
}

// ginMethod is the in-package alias for GinMethod.
func ginMethod(info *gotypes.Info, call *ast.CallExpr) (name, recvPkgPath string, ok bool) {
	return GinMethod(info, call)
}

// GinMethod resolves a selector call to (methodName, receiverPkgPath, ok) using
// go/types selections — the version- and alias-robust identity check (Pattern 2).
// Only method-value selections on a typed receiver qualify. Shared by the route
// recognizer and the handler analyzer so both gate on the same resolved identity.
func GinMethod(info *gotypes.Info, call *ast.CallExpr) (name, recvPkgPath string, ok bool) {
	sel, isSel := call.Fun.(*ast.SelectorExpr)
	if !isSel {
		return "", "", false
	}
	s := info.Selections[sel]
	if s == nil || s.Kind() != gotypes.MethodVal {
		return "", "", false
	}
	fn, _ := s.Obj().(*gotypes.Func)
	if fn == nil {
		return "", "", false
	}
	if p := fn.Pkg(); p != nil {
		recvPkgPath = p.Path()
	}
	return fn.Name(), recvPkgPath, true
}

// receiverObject returns the *types.Object the selector's receiver expression
// denotes (the `api` group variable), so routes/Use on the same group object are
// correlated. Returns nil when the receiver is not a plain identifier.
func receiverObject(info *gotypes.Info, call *ast.CallExpr) gotypes.Object {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return nil
	}
	ident, ok := sel.X.(*ast.Ident)
	if !ok {
		return nil
	}
	return info.ObjectOf(ident)
}

func receiverIsGinRouterGroup(info *gotypes.Info, call *ast.CallExpr) bool {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	t := gotypes.Unalias(info.TypeOf(sel.X))
	if ptr, ok := t.(*gotypes.Pointer); ok {
		t = gotypes.Unalias(ptr.Elem())
	}
	named, ok := t.(*gotypes.Named)
	if !ok || named.Obj() == nil || named.Obj().Pkg() == nil {
		return false
	}
	return named.Obj().Pkg().Path() == ginPkgPath && named.Obj().Name() == "RouterGroup"
}

func groupOf(groups map[gotypes.Object]*groupInfo, obj gotypes.Object) *groupInfo {
	g, ok := groups[obj]
	if !ok {
		g = &groupInfo{}
		groups[obj] = g
	}
	return g
}

// handlerSymbol returns the trailing selector/identifier name of the handler arg,
// e.g. `h.createGoal` -> "createGoal", `createGoal` -> "createGoal".
func handlerSymbol(arg ast.Expr) string {
	switch e := arg.(type) {
	case *ast.SelectorExpr:
		return e.Sel.Name
	case *ast.Ident:
		return e.Name
	}
	return ""
}

// HandlerFunc resolves a Gin route handler expression to its semantic function
// object. The returned identity is the analyzer hand-off; Handler remains only
// the SDK-facing symbol.
func HandlerFunc(info *gotypes.Info, arg ast.Expr) *gotypes.Func {
	if info == nil || arg == nil {
		return nil
	}
	switch e := arg.(type) {
	case *ast.Ident:
		fn, _ := info.ObjectOf(e).(*gotypes.Func)
		return fn
	case *ast.SelectorExpr:
		if sel := info.Selections[e]; sel != nil {
			fn, _ := sel.Obj().(*gotypes.Func)
			return fn
		}
		fn, _ := info.ObjectOf(e.Sel).(*gotypes.Func)
		return fn
	default:
		return nil
	}
}

// FuncObjectKey is the stable semantic key shared by route recognition and
// handler analysis. go/types object positions are identifier positions, matching
// TypesInfo.Defs for FuncDecl.Name.
func FuncObjectKey(fn *gotypes.Func) string {
	if fn == nil || fn.Pkg() == nil || !fn.Pos().IsValid() {
		return ""
	}
	return fn.Pkg().Path() + "@" + strconv.FormatInt(int64(fn.Pos()), 10)
}

// FuncIdentity renders a human-readable semantic identity for diagnostics and
// debugging. It is not part of the JSON facts contract.
func FuncIdentity(fn *gotypes.Func, fset *token.FileSet) string {
	if fn == nil {
		return ""
	}
	pkgPath := ""
	if fn.Pkg() != nil {
		pkgPath = fn.Pkg().Path()
	}
	recv := ""
	if sig, ok := gotypes.Unalias(fn.Type()).(*gotypes.Signature); ok && sig.Recv() != nil {
		recv = receiverTypeName(sig.Recv().Type())
	}
	file, line := positionOf(fset, fn.Pos())
	if recv == "" {
		return pkgPath + "." + fn.Name() + "@" + file + ":" + strconv.FormatUint(uint64(line), 10)
	}
	return pkgPath + "." + recv + "." + fn.Name() + "@" + file + ":" + strconv.FormatUint(uint64(line), 10)
}

func receiverTypeName(t gotypes.Type) string {
	if ptr, ok := gotypes.Unalias(t).(*gotypes.Pointer); ok {
		t = ptr.Elem()
	}
	named, ok := gotypes.Unalias(t).(*gotypes.Named)
	if !ok || named.Obj() == nil {
		return ""
	}
	return named.Obj().Name()
}

func middlewareSymbols(info *gotypes.Info, args []ast.Expr) []string {
	out := make([]string, 0, len(args))
	for _, arg := range args {
		if symbol := middlewareSymbol(info, arg); symbol != "" {
			out = appendUniqueStrings(out, symbol)
		}
	}
	return out
}

func middlewareSymbol(info *gotypes.Info, arg ast.Expr) string {
	switch e := arg.(type) {
	case *ast.CallExpr:
		return middlewareSymbol(info, e.Fun)
	case *ast.SelectorExpr:
		if fn := selectedFunc(info, e); fn != nil {
			return fn.Name()
		}
		return e.Sel.Name
	case *ast.Ident:
		if obj := info.ObjectOf(e); obj != nil {
			return obj.Name()
		}
		return e.Name
	default:
		return ""
	}
}

func selectedFunc(info *gotypes.Info, sel *ast.SelectorExpr) *gotypes.Func {
	if info == nil || sel == nil {
		return nil
	}
	if selection := info.Selections[sel]; selection != nil {
		fn, _ := selection.Obj().(*gotypes.Func)
		return fn
	}
	fn, _ := info.ObjectOf(sel.Sel).(*gotypes.Func)
	return fn
}

func appendUniqueStrings(dst []string, values ...string) []string {
	for _, value := range values {
		if value == "" {
			continue
		}
		seen := false
		for _, existing := range dst {
			if existing == value {
				seen = true
				break
			}
		}
		if !seen {
			dst = append(dst, value)
		}
	}
	return dst
}

// stringLiteral extracts the value of a basic string-literal expression.
func stringLiteral(arg ast.Expr) (string, bool) {
	lit, ok := arg.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", false
	}
	// Strip the surrounding quotes; route literals are simple, no escapes.
	return strings.Trim(lit.Value, "`\""), true
}

// normalizePath is the in-package alias for NormalizePath.
func normalizePath(p string) string { return NormalizePath(p) }

// NormalizePath converts a Gin path template to the OpenAPI form: each `:param`
// segment becomes `{param}` (Pitfall 5). Other segments pass through unchanged.
func NormalizePath(p string) string {
	if p == "" {
		return ""
	}
	if !strings.Contains(p, ":") {
		return p
	}
	segs := strings.Split(p, "/")
	for i, seg := range segs {
		if strings.HasPrefix(seg, ":") {
			segs[i] = "{" + seg[1:] + "}"
		}
	}
	return strings.Join(segs, "/")
}

func joinPaths(prefix, path string) string {
	prefix = normalizePath(prefix)
	path = normalizePath(path)
	if prefix == "" || prefix == "/" {
		if path == "" {
			return "/"
		}
		return path
	}
	if path == "" || path == "/" {
		return prefix
	}
	return strings.TrimRight(prefix, "/") + "/" + strings.TrimLeft(path, "/")
}

func spanOf(fset *token.FileSet, start, end token.Pos) facts.SourceSpan {
	if fset == nil || !start.IsValid() {
		return facts.SourceSpan{}
	}
	sp := fset.Position(start)
	ep := sp
	if end.IsValid() {
		ep = fset.Position(end)
	}
	return facts.SourceSpan{
		File:      sp.Filename,
		StartLine: uint32(sp.Line),
		EndLine:   uint32(ep.Line),
	}
}

func positionOf(fset *token.FileSet, pos token.Pos) (string, uint32) {
	if fset == nil || !pos.IsValid() {
		return "", 0
	}
	p := fset.Position(pos)
	return p.Filename, uint32(p.Line)
}
