// Command goextract loads a target Go module with the official go/packages loader
// in full-type mode, extracts router-agnostic HTTP/schema facts, and prints a
// single deterministic sorted JSON facts document on stdout. Errors go to stderr
// with a non-zero exit (the Rust subprocess driver maps them to typed CoreError).
//
// Usage:
//
//	goextract <target-dir> [--route-package <pattern>] [--schema-package <pattern>]
//
// 02-01 extracts DTO struct/enum schemas + float64/free-form-map diagnostics.
// Routes/handlers (02-02) and the Rust ApiGraph/inspect (02-03) build on this.
package main

import (
	"fmt"
	"os"
	"runtime"
	"strconv"
	"strings"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/handlers"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/routes"
	"github.com/gnr8/goextract/internal/types"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: goextract <target-dir> [--route-package <pattern>] [--schema-package <pattern>]")
		os.Exit(1)
	}
	targetDir := os.Args[1]
	scopes, err := parseScopes(os.Args[2:])
	if err != nil {
		fmt.Fprintln(os.Stderr, "goextract:", err)
		os.Exit(1)
	}

	if err := run(targetDir, scopes, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, "goextract:", err)
		os.Exit(1)
	}
}

type packageScopes struct {
	routePatterns  []string
	schemaPatterns []string
}

func parseScopes(args []string) (packageScopes, error) {
	var scopes packageScopes
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--route-package":
			i++
			if i >= len(args) {
				return scopes, fmt.Errorf("--route-package requires a pattern")
			}
			scopes.routePatterns = append(scopes.routePatterns, args[i])
		case "--schema-package":
			i++
			if i >= len(args) {
				return scopes, fmt.Errorf("--schema-package requires a pattern")
			}
			scopes.schemaPatterns = append(scopes.schemaPatterns, args[i])
		default:
			return scopes, fmt.Errorf("unknown argument %q", args[i])
		}
	}
	return scopes, nil
}

// run loads the module, builds the facts document, and writes JSON to w. Any hard
// loader failure is returned as an error; per-package load errors become
// diagnostics (GO-06) so a partial graph is never silently emitted.
func run(targetDir string, scopes packageScopes, w *os.File) error {
	routeRes, err := load.Load(targetDir, scopes.routePatterns...)
	if err != nil {
		return err
	}
	schemaRes := routeRes
	if !sameStrings(scopes.routePatterns, scopes.schemaPatterns) {
		schemaRes, err = load.Load(targetDir, scopes.schemaPatterns...)
		if err != nil {
			return err
		}
	}

	diags := diag.New()
	addLoadDiagnostics(routeRes, diags)
	if schemaRes != routeRes {
		addLoadDiagnostics(schemaRes, diags)
	}

	schemas := types.Extract(schemaRes, diags)

	module := moduleOf(routeRes)
	if module == "" {
		module = moduleOf(schemaRes)
	}

	// Recognize the Gin route table, then enrich each route with handler-inferred
	// request/response/param facts. buildRoutes owns the wiring + the per-route
	// diagnostics (untyped query params, dynamic responses). Every fact is derived
	// PURELY from Go code — there is no annotation source and no fallback path
	// (AGENTS.md rules 1 & 3).
	//
	// The Analyzer carries the module prefix as per-invocation context (WR-03), so
	// the analysis is reentrant rather than depending on process-global setup
	// ordering. The analyzer keeps duplicate bare-name collisions so they can be
	// reported after route recognition only when they affect a route (WR-02).
	analyzer := handlers.NewAnalyzer(routeRes, module, diags)
	recognized := routes.RecognizeWithDiagnostics(routeRes, diags)
	analyzer.ReportRouteHandlerCollisions(recognized, diags)
	routeFacts, syntheticSchemas := buildRoutes(analyzer, recognized, diags)
	schemas = append(schemas, syntheticSchemas...)

	doc := facts.GoFacts{
		Module: module,
		// The toolchain this binary was COMPILED with, which is not necessarily the one
		// `go/packages` shells out to: go/types admits only the language version the
		// application was built with, so a helper older than the module it reads reports
		// every package gated on the newer release as a load error. The host compares this
		// against the toolchain the analyzed module selects and refuses to publish facts
		// from a helper that cannot type-check them.
		ExtractorToolchain: runtime.Version(),
		Routes:             routeFacts,
		Schemas:            schemas,
		Diagnostics:        diags.Items(),
	}

	return facts.Marshal(doc, w)
}

// addLoadDiagnostics turns each per-package loader error into a diagnostic, naming the
// loader stage that produced it. Which stage failed decides what the reader has to fix —
// a "list" error is the go command failing to describe the package (module, build, or
// toolchain resolution), while "parse"/"type" are the package's own source — so the
// classification the loader already made is carried through rather than flattened away.
func addLoadDiagnostics(res *load.Result, diags *diag.Accumulator) {
	for _, le := range res.Errors {
		file, line := splitPos(le.Pos)
		diags.Error("go/packages "+le.Kind+" error: "+le.Msg, file, line)
	}
}

func sameStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// buildRoutes maps each recognized Gin route to a router-agnostic RouteFact.
//
// Every fact has exactly ONE code-derived source (AGENTS.md rules 1 & 3): the
// method/path/handler come from the route recognizer, the operationId is the
// handler symbol, and the request body / responses / params come from analyzing
// the handler body. There is no annotation source and no fallback anywhere.
func buildRoutes(analyzer *handlers.Analyzer, recognized []routes.Route, diags *diag.Accumulator) ([]facts.RouteFact, []facts.SchemaFact) {
	out := make([]facts.RouteFact, 0, len(recognized))
	schemas := []facts.SchemaFact{}
	seenSchema := map[string]bool{}
	for _, r := range recognized {
		rf := facts.RouteFact{
			Method: r.Method,
			Path:   r.Path,
			// operationId is derived deterministically from the handler symbol in
			// code (e.g. "createGoal", "updateGoal") — there is no override source.
			Handler:             r.Handler,
			OperationID:         r.Handler,
			Group:               r.Group,
			Middleware:          r.Middleware,
			Params:              []facts.ParamFact{},
			Responses:           []facts.ResponseFact{},
			RequestBodyRequired: true,
			Span:                r.Span,
		}

		// Code-inferred request/response/param facts — the only source.
		cf := analyzer.Analyze(r, diags)
		// Handler doc-comment prose (rule 0.1 category 2): human words only. It cannot
		// state or override any structural fact assembled above or below it.
		rf.Summary = cf.Summary
		rf.Description = cf.Description
		rf.RequestBody = cf.RequestBody
		rf.RequestBodyRequired = cf.RequestBodyRequired
		rf.RequestBodyContentType = cf.RequestBodyContentType
		rf.RequestBodyVariants = cf.RequestBodyVariants
		rf.Responses = cf.Responses
		rf.Params = mergeRoutePathParams(r.Path, r.Span, cf.Params)
		for _, schema := range cf.Schemas {
			if seenSchema[schema.ID] {
				continue
			}
			seenSchema[schema.ID] = true
			schemas = append(schemas, schema)
		}

		out = append(out, rf)
	}
	return out, schemas
}

func mergeRoutePathParams(path string, span facts.SourceSpan, params []facts.ParamFact) []facts.ParamFact {
	seen := map[string]bool{}
	out := make([]facts.ParamFact, 0, len(params))
	for _, param := range params {
		seen[param.Location+"/"+param.Name] = true
		out = append(out, param)
	}
	for _, name := range pathTokens(path) {
		key := "path/" + name
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, facts.ParamFact{
			Name:     name,
			Location: "path",
			Required: true,
			Schema:   facts.PrimitiveType(facts.StringPrim()),
			Span:     span,
		})
	}
	return out
}

func pathTokens(path string) []string {
	tokens := []string{}
	rest := path
	for {
		open := strings.Index(rest, "{")
		if open < 0 {
			return tokens
		}
		after := rest[open+1:]
		close := strings.Index(after, "}")
		if close < 0 {
			return tokens
		}
		tokens = append(tokens, after[:close])
		rest = after[close+1:]
	}
}

func moduleOf(res *load.Result) string {
	for _, pkg := range res.Packages {
		if pkg.Module != nil && pkg.Module.Main {
			return pkg.Module.Path
		}
	}
	return ""
}

// splitPos parses a "file:line:col" position string from go/packages into a file
// and line by splitting on the LAST two colons (so filenames containing ':' still
// resolve the line). Missing/unparsable parts degrade to ("", 0) so a load error
// still emits as a diagnostic (GO-06).
func splitPos(pos string) (string, uint32) {
	if pos == "" {
		return "", 0
	}
	parts := strings.Split(pos, ":")
	if len(parts) < 3 {
		return "", 0
	}
	lineStr := parts[len(parts)-2]
	line, err := strconv.Atoi(lineStr)
	if err != nil {
		return "", 0
	}
	file := strings.Join(parts[:len(parts)-2], ":")
	return file, uint32(line)
}
