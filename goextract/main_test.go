package main

import (
	"encoding/json"
	"go/token"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/handlers"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/routes"
)

func TestParseScopesRequiresNamedFlags(t *testing.T) {
	scopes, err := parseScopes([]string{
		"--route-package",
		"./internal/http/...",
		"--schema-package",
		"./internal/dto/...",
	})
	if err != nil {
		t.Fatalf("parse named scopes: %v", err)
	}
	if !reflect.DeepEqual(scopes.routePatterns, []string{"./internal/http/..."}) {
		t.Fatalf("route patterns: %+v", scopes.routePatterns)
	}
	if !reflect.DeepEqual(scopes.schemaPatterns, []string{"./internal/dto/..."}) {
		t.Fatalf("schema patterns: %+v", scopes.schemaPatterns)
	}

	if _, err := parseScopes([]string{"./internal/http/..."}); err == nil {
		t.Fatal("positional package scope must be rejected")
	}
}

func TestGinContractRegressionFacts(t *testing.T) {
	dir, err := filepath.Abs("../fixtures/gin-contract-regression")
	if err != nil {
		t.Fatalf("fixture path: %v", err)
	}
	tmp, err := os.CreateTemp("", "gnr8-gin-contract-*.json")
	if err != nil {
		t.Fatalf("temp facts file: %v", err)
	}
	defer os.Remove(tmp.Name())
	defer tmp.Close()

	if err := run(dir, packageScopes{}, tmp); err != nil {
		t.Fatalf("run goextract: %v", err)
	}
	if _, err := tmp.Seek(0, 0); err != nil {
		t.Fatalf("rewind facts file: %v", err)
	}
	var doc facts.GoFacts
	if err := json.NewDecoder(tmp).Decode(&doc); err != nil {
		t.Fatalf("decode facts: %v", err)
	}
	wantDiagnostics := map[string]bool{
		"response.missing\x00GET /v1/items/raw-stream":                        true,
		"response.header.unresolved\x00GET /v1/files/{fileId}/dynamic-header": true,
		"request.parameter.unresolved\x00POST /v1/files/dynamic-upload":       true,
		"request.body.unresolved\x00POST /v1/files/dynamic-upload":            true,
		"request.body.unresolved\x00POST /v1/files/form-file/request-dynamic": true,
		"security.requirement.missing\x00GET /v1/items/request-observations":  true,
	}
	if len(doc.Diagnostics) != len(wantDiagnostics) {
		t.Fatalf("gin contract fixture diagnostics: got %+v", doc.Diagnostics)
	}
	for _, diagnostic := range doc.Diagnostics {
		if !wantDiagnostics[diagnostic.Code+"\x00"+diagnostic.Operation] {
			t.Fatalf("unexpected diagnostic: %+v", diagnostic)
		}
	}

	login := routeByHandler(t, doc, "login")
	assertResponseBodyRef(t, login, 200, "LoginResponse")

	getChild := routeByHandler(t, doc, "getChild")
	assertPathParam(t, getChild, "itemId")
	assertPathParam(t, getChild, "childId")

	update := routeByHandler(t, doc, "updateItem")
	if update.Method != "PATCH" {
		t.Fatalf("updateItem method: want PATCH got %s", update.Method)
	}

	assertBodylessStatus(t, routeByHandler(t, doc, "logout"), 204)
	assertBodylessStatus(t, routeByHandler(t, doc, "deleteItem"), 204)

	list := routeByHandler(t, doc, "listSavedViews")
	if list.Responses[0].Body == nil || list.Responses[0].Body.RefID != "__synthetic.ListSavedViews200Response" {
		t.Fatalf("listSavedViews response should use synthetic array schema, got %+v", list.Responses)
	}
	if schemaByID(t, doc, "__synthetic.ListSavedViews200Response").Body.Type != facts.TypeArray {
		t.Fatalf("listSavedViews synthetic schema should be array")
	}

	job := routeByHandler(t, doc, "createJob")
	if job.Responses[0].Body == nil || job.Responses[0].Body.RefID == "github.com/gin-gonic/gin.H" {
		t.Fatalf("createJob must not reference gin.H, got %+v", job.Responses)
	}
	if schemaByID(t, doc, "__synthetic.CreateJob202Response").Body.Type != facts.TypeObject {
		t.Fatalf("createJob synthetic schema should be object")
	}

	download := routeByHandler(t, doc, "downloadFile")
	if download.Responses[0].BodyKind != "binary" || download.Responses[0].ContentType != "application/octet-stream" {
		t.Fatalf("downloadFile should be binary octet-stream, got %+v", download.Responses)
	}
	stream := routeByHandler(t, doc, "streamFile")
	if stream.Responses[0].BodyKind != "binary" || stream.Responses[0].ContentType != "application/pdf" {
		t.Fatalf("streamFile should be binary application/pdf, got %+v", stream.Responses)
	}
	reader := routeByHandler(t, doc, "readFile")
	if reader.Responses[0].BodyKind != "binary" || reader.Responses[0].ContentType != "application/pdf" {
		t.Fatalf("readFile should be binary application/pdf, got %+v", reader.Responses)
	}
	for _, name := range []string{"Content-Disposition", "Content-Length", "Content-Type", "X-Session-ID"} {
		assertResponseHeader(t, reader, 200, name)
		assertNoResponseHeader(t, reader, 404, name)
	}
	dynamicHeader := routeByHandler(t, doc, "dynamicHeaderFile")
	assertBodylessStatus(t, dynamicHeader, 204)
	for _, response := range dynamicHeader.Responses {
		if len(response.Headers) != 0 {
			t.Fatalf("a dynamic response header name must be diagnosed, never guessed: %+v", response.Headers)
		}
	}
	redirect := routeByHandler(t, doc, "redirectFile")
	assertBodylessStatus(t, redirect, 307)
	assertResponseHeader(t, redirect, 307, "Location")
	assertResponseHeader(t, redirect, 307, "X-Session-ID")
	// The same handler mutates a REQUEST header. Only what it sends is a response fact.
	assertNoResponseHeader(t, redirect, 307, "X-Forwarded-Trace")
	helperRedirect := routeByHandler(t, doc, "helperRedirectFile")
	assertBodylessStatus(t, helperRedirect, 302)
	assertResponseHeader(t, helperRedirect, 302, "Location")

	upload := routeByHandler(t, doc, "uploadFile")
	if upload.RequestBody == nil || upload.RequestBody.RefID != "CreateUploadRequest" || upload.RequestBodyContentType != "application/json" {
		t.Fatalf("upload JSON request body mismatch: %+v", upload)
	}
	if len(upload.RequestBodyVariants) != 1 || upload.RequestBodyVariants[0].ContentType != "multipart/form-data" {
		t.Fatalf("upload multipart request variant mismatch: %+v", upload.RequestBodyVariants)
	}
	uploadForm := schemaByID(t, doc, upload.RequestBodyVariants[0].Body.RefID)
	encodedUploadFields, err := json.Marshal(uploadForm.Body.Of)
	if err != nil {
		t.Fatalf("encode upload multipart schema: %v", err)
	}
	var uploadFields []facts.FieldFact
	if err := json.Unmarshal(encodedUploadFields, &uploadFields); err != nil {
		t.Fatalf("upload multipart schema should be an object: %+v", uploadForm)
	}
	fieldByName := map[string]facts.FieldFact{}
	for _, field := range uploadFields {
		fieldByName[field.JSONName] = field
	}
	if fieldByName["request"].Schema.Type != facts.TypePrimitive || !fieldByName["request"].ValidatorRequiresPresence {
		t.Fatalf("upload JSON string part mismatch: %+v", fieldByName["request"])
	}
	files := fieldByName["files"]
	filesSchema, err := json.Marshal(files.Schema)
	if err != nil {
		t.Fatalf("encode upload file schema: %v", err)
	}
	if !jsonEqual(t, filesSchema, []byte(`{"type":"array","of":{"type":"primitive","of":{"prim":"bytes"}}}`)) || files.ValidatorRequiresPresence {
		t.Fatalf("upload repeated file parts mismatch: %+v", files)
	}
	updateUpload := routeByHandler(t, doc, "updateUploadFile")
	if updateUpload.RequestBody == nil || updateUpload.RequestBody.RefID != "UpdateItemRequest" ||
		len(updateUpload.RequestBodyVariants) != 1 ||
		updateUpload.RequestBodyVariants[0].ContentType != "multipart/form-data" {
		t.Fatalf("second generic upload instantiation mismatch: %+v", updateUpload)
	}
	contextFormFile := routeByHandler(t, doc, "contextFormFile")
	requestFormFile := routeByHandler(t, doc, "requestFormFile")
	contextFields := multipartRequestFields(t, doc, contextFormFile)
	requestFields := multipartRequestFields(t, doc, requestFormFile)
	if !reflect.DeepEqual(contextFields, requestFields) {
		t.Fatalf("Gin and net/http FormFile access must produce the same fields: gin=%+v request=%+v", contextFields, requestFields)
	}
	if field := requestFields["asset"]; primName(field.Schema) != facts.PrimBytes || !field.ValidatorRequiresPresence {
		t.Fatalf("Request.FormFile asset should be required bytes under its exact source name: %+v", field)
	}
	requestFiles := routeByHandler(t, doc, "requestFormFiles")
	requestFileFields := multipartRequestFields(t, doc, requestFiles)
	for _, name := range []string{"primaryImage", "supportingDocument"} {
		field := requestFileFields[name]
		if primName(field.Schema) != facts.PrimBytes || !field.ValidatorRequiresPresence {
			t.Fatalf("Request.FormFile %s should be required bytes: %+v", name, field)
		}
	}
	if field := requestFileFields["caption"]; primName(field.Schema) != facts.PrimString || field.ValidatorRequiresPresence {
		t.Fatalf("manual form fields must compose with Request.FormFile: %+v", requestFileFields)
	}
	assertPathParam(t, requestFiles, "collectionId")
	assertRequestParam(t, requestFiles, "header", "X-Upload-Trace", false)
	dynamicRequestFile := routeByHandler(t, doc, "dynamicRequestFormFile")
	if dynamicRequestFile.RequestBody != nil {
		t.Fatalf("a dynamic Request.FormFile name must be diagnosed without inventing a body: %+v", dynamicRequestFile)
	}
	events := routeByHandler(t, doc, "itemEvents")
	if events.Responses[0].BodyKind != "sse" || events.Responses[0].ContentType != "text/event-stream" {
		t.Fatalf("itemEvents should be SSE text/event-stream, got %+v", events.Responses)
	}
	rawStream := routeByHandler(t, doc, "rawStream")
	if len(rawStream.Responses) != 0 {
		t.Fatalf("rawStream should not be classified as SSE, got %+v", rawStream.Responses)
	}

	search := routeByHandler(t, doc, "searchItems")
	assertQueryParam(t, search, "q", false, `{"type":"primitive","of":{"prim":"string"}}`, "")
	assertQueryParam(t, search, "limit", false, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":true}}`, "")
	assertQueryParam(t, search, "trimmedLimit", false, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":true}}`, "")
	assertQueryParam(t, search, "wrappedLimit", false, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":true}}`, "")
	assertQueryParamDefault(t, search, "sort", false, `{"type":"primitive","of":{"prim":"string"}}`, "string", "asc")
	assertQueryParamDefault(t, search, "cursor", false, `{"type":"primitive","of":{"prim":"string"}}`, "string", "first")
	assertQueryParam(t, search, "token", false, `{"type":"primitive","of":{"prim":"string"}}`, "")
	assertQueryParam(t, search, "offset", false, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":false}}`, "")
	assertQueryParam(t, search, "page", true, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":false}}`, "")

	queryRequired := routeByHandler(t, doc, "queryRequired")
	assertQueryParam(t, queryRequired, "term", true, `{"type":"primitive","of":{"prim":"string"}}`, "")
	queryOptional := routeByHandler(t, doc, "queryOptional")
	assertQueryParam(t, queryOptional, "view", false, `{"type":"primitive","of":{"prim":"string"}}`, "")

	observations := routeByHandler(t, doc, "requestObservations")
	assertRequestParam(t, observations, "header", "X-Observed", false)
	assertRequestParam(t, observations, "header", "X-Helper-Observed", false)
	assertRequestParam(t, observations, "header", "X-Required", true)
	assertRequestParam(t, observations, "cookie", "observed-cookie", false)
	assertRequestParam(t, observations, "cookie", "required-cookie", true)
	for _, param := range observations.Params {
		if param.Location == "header" && param.Name == "Authorization" {
			t.Fatalf("Authorization must be represented by security configuration, not an ordinary parameter: %+v", observations.Params)
		}
	}

	attendance := routeByHandler(t, doc, "attendance")
	assertQueryParam(t, attendance, "startDate", true, `{"type":"well_known","of":"date_time"}`, "")
	assertQueryParam(t, attendance, "days", false, `{"type":"primitive","of":{"prim":"int","bits":64,"signed":true}}`, "5")

	markRead := routeByHandler(t, doc, "markRead")
	if markRead.RequestBody == nil || markRead.RequestBodyRequired {
		t.Fatalf("markRead should have an optional request body, got body=%+v required=%v", markRead.RequestBody, markRead.RequestBodyRequired)
	}
	headerRead := routeByHandler(t, doc, "headerRead")
	if headerRead.RequestBody == nil || headerRead.RequestBodyRequired {
		t.Fatalf("headerRead should have an optional request body from direct Content-Length header guard, got body=%+v required=%v", headerRead.RequestBody, headerRead.RequestBodyRequired)
	}
	combinedHeaderRead := routeByHandler(t, doc, "combinedHeaderRead")
	if combinedHeaderRead.RequestBody == nil || !combinedHeaderRead.RequestBodyRequired {
		t.Fatalf("combinedHeaderRead should keep its request body required because another header can trigger binding, got body=%+v required=%v", combinedHeaderRead.RequestBody, combinedHeaderRead.RequestBodyRequired)
	}
	forceRead := routeByHandler(t, doc, "forceRead")
	if forceRead.RequestBody == nil || !forceRead.RequestBodyRequired {
		t.Fatalf("forceRead should keep its request body required because the OR guard can bind without a body, got body=%+v required=%v", forceRead.RequestBody, forceRead.RequestBodyRequired)
	}
	mixedRead := routeByHandler(t, doc, "mixedRead")
	if mixedRead.RequestBody == nil || !mixedRead.RequestBodyRequired {
		t.Fatalf("mixedRead should keep its request body required because not all binds are guarded, got body=%+v required=%v", mixedRead.RequestBody, mixedRead.RequestBodyRequired)
	}
	unrelatedLengthRead := routeByHandler(t, doc, "unrelatedLengthRead")
	if unrelatedLengthRead.RequestBody == nil || !unrelatedLengthRead.RequestBodyRequired {
		t.Fatalf("unrelatedLengthRead should keep its request body required because unrelated ContentLength is not a Gin request body guard, got body=%+v required=%v", unrelatedLengthRead.RequestBody, unrelatedLengthRead.RequestBodyRequired)
	}
}

func TestBuildRoutesKeepsRequiredBodyDefaultForMissingHandler(t *testing.T) {
	analyzer := handlers.NewAnalyzer(&load.Result{Fset: token.NewFileSet()}, "", diag.New())
	routeFacts, _ := buildRoutes(
		analyzer,
		[]routes.Route{
			{
				Method:  "GET",
				Path:    "/missing",
				Handler: "missingHandler",
				Span: facts.SourceSpan{
					File:      "routes.go",
					StartLine: 1,
					EndLine:   1,
				},
			},
		},
		diag.New(),
	)
	if len(routeFacts) != 1 {
		t.Fatalf("expected one route fact, got %d", len(routeFacts))
	}
	if !routeFacts[0].RequestBodyRequired {
		t.Fatalf("missing handler should preserve request_body_required default true, got %+v", routeFacts[0])
	}
}

// A loader error names the stage that produced it, so a reader can tell an environment
// failure (the go command could not describe the package) from the package's own source.
func TestAddLoadDiagnosticsNamesTheLoaderStage(t *testing.T) {
	diags := diag.New()
	addLoadDiagnostics(&load.Result{
		Errors: []load.LoadError{
			{Pkg: "example.com/app", Pos: "/tmp/app/main.go:1:1", Msg: "boom", Kind: "type"},
			{Pkg: "example.com/dep", Pos: "", Msg: "no go files", Kind: "list"},
		},
	}, diags)

	items := diags.Items()
	if len(items) != 2 {
		t.Fatalf("expected two diagnostics, got %d", len(items))
	}
	if items[0].Message != "go/packages type error: boom" {
		t.Fatalf("type-stage message: %q", items[0].Message)
	}
	if items[0].File != "/tmp/app/main.go" || items[0].Line != 1 {
		t.Fatalf("type-stage location: %q:%d", items[0].File, items[0].Line)
	}
	if items[1].Message != "go/packages list error: no go files" {
		t.Fatalf("list-stage message: %q", items[1].Message)
	}
	for _, item := range items {
		if item.Code != "source.load.failed" || item.Severity != "ERROR" {
			t.Fatalf("load failures keep their stable identity: %+v", item)
		}
	}
}

// The facts document names the toolchain the sidecar was BUILT with. The host compares it
// against the toolchain the analyzed module selects, because go/types admits only the
// language version the application was built with.
func TestRunReportsTheToolchainTheExtractorWasBuiltWith(t *testing.T) {
	dir, err := filepath.Abs("../fixtures/gin-contract-regression")
	if err != nil {
		t.Fatalf("fixture path: %v", err)
	}
	tmp, err := os.CreateTemp("", "gnr8-extractor-toolchain-*.json")
	if err != nil {
		t.Fatalf("temp facts file: %v", err)
	}
	defer os.Remove(tmp.Name())
	defer tmp.Close()

	if err := run(dir, packageScopes{}, tmp); err != nil {
		t.Fatalf("run goextract: %v", err)
	}
	if _, err := tmp.Seek(0, 0); err != nil {
		t.Fatalf("rewind facts file: %v", err)
	}
	var doc facts.GoFacts
	if err := json.NewDecoder(tmp).Decode(&doc); err != nil {
		t.Fatalf("decode facts: %v", err)
	}
	if doc.ExtractorToolchain != runtime.Version() {
		t.Fatalf("extractor_toolchain = %q, want %q", doc.ExtractorToolchain, runtime.Version())
	}
}

func routeByHandler(t *testing.T, doc facts.GoFacts, handler string) facts.RouteFact {
	t.Helper()
	for _, route := range doc.Routes {
		if route.Handler == handler {
			return route
		}
	}
	t.Fatalf("missing route for handler %s", handler)
	return facts.RouteFact{}
}

func schemaByID(t *testing.T, doc facts.GoFacts, id string) facts.SchemaFact {
	t.Helper()
	for _, schema := range doc.Schemas {
		if schema.ID == id {
			return schema
		}
	}
	t.Fatalf("missing schema %s", id)
	return facts.SchemaFact{}
}

func multipartRequestFields(t *testing.T, doc facts.GoFacts, route facts.RouteFact) map[string]facts.FieldFact {
	t.Helper()
	if route.RequestBody == nil || route.RequestBodyContentType != "multipart/form-data" || !route.RequestBodyRequired {
		t.Fatalf("%s should have a required multipart request body, got %+v", route.Handler, route)
	}
	schema := schemaByID(t, doc, route.RequestBody.RefID)
	encoded, err := json.Marshal(schema.Body.Of)
	if err != nil {
		t.Fatalf("encode %s multipart fields: %v", route.Handler, err)
	}
	var fields []facts.FieldFact
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatalf("%s multipart schema should be an object: %+v", route.Handler, schema)
	}
	byName := make(map[string]facts.FieldFact, len(fields))
	for _, field := range fields {
		byName[field.JSONName] = field
	}
	return byName
}

func assertPathParam(t *testing.T, route facts.RouteFact, name string) {
	t.Helper()
	for _, param := range route.Params {
		if param.Location == "path" && param.Name == name && param.Required {
			return
		}
	}
	t.Fatalf("%s should have required path param %s, got %+v", route.Handler, name, route.Params)
}

func assertBodylessStatus(t *testing.T, route facts.RouteFact, status uint16) {
	t.Helper()
	if len(route.Responses) != 1 || route.Responses[0].Status != status || route.Responses[0].Body != nil {
		t.Fatalf("%s should have bodyless status %d, got %+v", route.Handler, status, route.Responses)
	}
}

func assertResponseBodyRef(t *testing.T, route facts.RouteFact, status uint16, refID string) {
	t.Helper()
	for _, response := range route.Responses {
		if response.Status != status {
			continue
		}
		if response.Body == nil || response.Body.RefID != refID {
			t.Fatalf("%s response %d should reference %s, got %+v", route.Handler, status, refID, response)
		}
		return
	}
	t.Fatalf("%s should have response %d, got %+v", route.Handler, status, route.Responses)
}

func assertResponseHeader(t *testing.T, route facts.RouteFact, status uint16, name string) {
	t.Helper()
	for _, response := range route.Responses {
		if response.Status != status {
			continue
		}
		for _, header := range response.Headers {
			if header.Name == name {
				return
			}
		}
		t.Fatalf("%s response %d should declare header %s, got %+v", route.Handler, status, name, response.Headers)
	}
	t.Fatalf("%s should have response %d, got %+v", route.Handler, status, route.Responses)
}

func assertNoResponseHeader(t *testing.T, route facts.RouteFact, status uint16, name string) {
	t.Helper()
	for _, response := range route.Responses {
		if response.Status != status {
			continue
		}
		for _, header := range response.Headers {
			if header.Name == name {
				t.Fatalf("%s response %d must not declare header %s, got %+v", route.Handler, status, name, response.Headers)
			}
		}
		return
	}
	t.Fatalf("%s should have response %d, got %+v", route.Handler, status, route.Responses)
}

func assertRequestParam(t *testing.T, route facts.RouteFact, location, name string, required bool) {
	t.Helper()
	for _, param := range route.Params {
		if param.Location == location && param.Name == name {
			if param.Required != required {
				t.Fatalf("%s %s %s required: got %v, want %v", route.Handler, location, name, param.Required, required)
			}
			return
		}
	}
	t.Fatalf("%s missing %s %s, got %+v", route.Handler, location, name, route.Params)
}

func assertQueryParam(t *testing.T, route facts.RouteFact, name string, required bool, schemaJSON string, defaultNumber string) {
	t.Helper()
	if defaultNumber == "" {
		assertQueryParamDefault(t, route, name, required, schemaJSON, "", nil)
		return
	}
	assertQueryParamDefault(t, route, name, required, schemaJSON, "number", defaultNumber)
}

func assertQueryParamDefault(t *testing.T, route facts.RouteFact, name string, required bool, schemaJSON string, defaultType string, defaultValue any) {
	t.Helper()
	for _, param := range route.Params {
		if param.Location != "query" || param.Name != name {
			continue
		}
		gotSchema, err := json.Marshal(param.Schema)
		if err != nil {
			t.Fatalf("%s query %s schema marshal: %v", route.Handler, name, err)
		}
		if param.Required != required || !jsonEqual(t, gotSchema, []byte(schemaJSON)) {
			t.Fatalf("%s query %s: want required=%v schema=%s, got required=%v schema=%s", route.Handler, name, required, schemaJSON, param.Required, gotSchema)
		}
		if defaultType == "" {
			if param.Default != nil {
				t.Fatalf("%s query %s should not have default, got %+v", route.Handler, name, param.Default)
			}
			return
		}
		if param.Default == nil || param.Default.Type != defaultType || param.Default.Value != defaultValue {
			t.Fatalf("%s query %s default: want %s %v, got %+v", route.Handler, name, defaultType, defaultValue, param.Default)
		}
		return
	}
	t.Fatalf("%s missing query param %s, got %+v", route.Handler, name, route.Params)
}

func jsonEqual(t *testing.T, left, right []byte) bool {
	t.Helper()
	var l, r any
	if err := json.Unmarshal(left, &l); err != nil {
		t.Fatalf("unmarshal left json: %v", err)
	}
	if err := json.Unmarshal(right, &r); err != nil {
		t.Fatalf("unmarshal right json: %v", err)
	}
	return reflect.DeepEqual(l, r)
}

func primName(ty facts.Type) string {
	if ty.Type != facts.TypePrimitive {
		return ""
	}
	switch primitive := ty.Of.(type) {
	case *facts.Prim:
		return primitive.Prim
	case map[string]any:
		name, _ := primitive["prim"].(string)
		return name
	default:
		return ""
	}
}
