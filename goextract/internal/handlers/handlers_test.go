package handlers_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gnr8/goextract/internal/diag"
	"github.com/gnr8/goextract/internal/facts"
	"github.com/gnr8/goextract/internal/handlers"
	"github.com/gnr8/goextract/internal/load"
	"github.com/gnr8/goextract/internal/routes"
)

func fixtureDir(t *testing.T) string {
	t.Helper()
	abs, err := filepath.Abs(filepath.Join("..", "..", "..", "fixtures", "goalservice"))
	if err != nil {
		t.Fatalf("resolve fixture dir: %v", err)
	}
	return abs
}

// analyzeFixture loads the fixture, recognizes routes, builds an Analyzer (which
// indexes the handlers and carries the module prefix so refs use 02-01 schema
// ids), and returns per-handler code facts plus the accumulated diagnostics.
func analyzeFixture(t *testing.T) (map[string]handlers.CodeFacts, []facts.DiagnosticFact) {
	t.Helper()
	res, err := load.Load(fixtureDir(t))
	if err != nil {
		t.Fatalf("load fixture: %v", err)
	}
	var module string
	for _, pkg := range res.Packages {
		if pkg.Module != nil && pkg.Module.Main {
			module = pkg.Module.Path
		}
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, module, diags)
	out := map[string]handlers.CodeFacts{}
	for _, r := range routes.Recognize(res) {
		out[r.Handler] = analyzer.Analyze(r, diags)
	}
	return out, diags.Items()
}

func TestCreateGoalRequestAndResponses(t *testing.T) {
	facts, _ := analyzeFixture(t)
	cf, ok := facts["createGoal"]
	if !ok {
		t.Fatal("createGoal not analyzed")
	}

	if cf.RequestBody == nil || !strings.HasSuffix(cf.RequestBody.RefID, "dto.CreateGoalInput") {
		t.Fatalf("createGoal request body: want dto.CreateGoalInput ref, got %+v", cf.RequestBody)
	}

	want := map[uint16]string{201: "dto.CommandMessageWithUUID", 400: "dto.HttpError"}
	if len(cf.Responses) != len(want) {
		t.Fatalf("createGoal: want %d responses, got %d: %+v", len(want), len(cf.Responses), cf.Responses)
	}
	for _, r := range cf.Responses {
		suffix, ok := want[r.Status]
		if !ok {
			t.Errorf("createGoal: unexpected status %d", r.Status)
			continue
		}
		if r.Body == nil || !strings.HasSuffix(r.Body.RefID, suffix) {
			t.Errorf("createGoal %d: want body %s, got %+v", r.Status, suffix, r.Body)
		}
	}
}

func TestUnknownHandlerProducesErrorDiagnostic(t *testing.T) {
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(&load.Result{}, "example.com/service", diags)
	route := routes.Route{
		Method:  "GET",
		Path:    "/missing",
		Handler: "missingHandler",
		Span: facts.SourceSpan{
			File:      "internal/http.go",
			StartLine: 27,
			EndLine:   27,
		},
	}

	analyzer.Analyze(route, diags)
	items := diags.Items()
	if len(items) != 1 {
		t.Fatalf("want one unknown-handler diagnostic, got %+v", items)
	}
	got := items[0]
	if got.Severity != "ERROR" || !strings.Contains(got.Message, "unknown handler 'missingHandler'") {
		t.Fatalf("want actionable unknown-handler diagnostic, got %+v", got)
	}
	if got.File != "internal/http.go" || got.Line != 27 {
		t.Fatalf("want route registration provenance, got %+v", got)
	}
}

// TestStatusFromGoConstant proves status numbers come from go/constant resolution
// of http.StatusXxx, not a hardcoded name->number map: 201, 400, 200 all appear
// exactly as the net/http constants define them.
func TestStatusFromGoConstant(t *testing.T) {
	facts, _ := analyzeFixture(t)

	create := statuses(facts["createGoal"].Responses)
	if !equalUint16(create, []uint16{201, 400}) {
		t.Errorf("createGoal statuses: want [201 400], got %v", create)
	}
	list := statuses(facts["listGoals"].Responses)
	if !equalUint16(list, []uint16{200}) {
		t.Errorf("listGoals statuses: want [200], got %v", list)
	}
}

func TestPathParamForUUIDHandlers(t *testing.T) {
	facts, _ := analyzeFixture(t)
	for _, h := range []string{"updateGoal", "deleteGoal"} {
		cf := facts[h]
		p, ok := paramByName(cf.Params, "uuid")
		if !ok {
			t.Fatalf("%s: missing path param 'uuid': %+v", h, cf.Params)
		}
		// The neutral param type for a path string is a string primitive. Compare the
		// marshaled wire form (the local var `facts` shadows the package here).
		gotSchema, err := json.Marshal(p.Schema)
		if err != nil {
			t.Fatalf("%s uuid: marshal schema: %v", h, err)
		}
		const wantSchema = `{"type":"primitive","of":{"prim":"string"}}`
		if p.Location != "path" || !p.Required || string(gotSchema) != wantSchema {
			t.Errorf("%s uuid: want path/required/%s, got loc=%s req=%v schema=%s",
				h, wantSchema, p.Location, p.Required, gotSchema)
		}
	}
}

func TestListGoalsQueryParamsAndDiagnostics(t *testing.T) {
	facts, diags := analyzeFixture(t)
	cf := facts["listGoals"]

	wantQuery := []string{"aggregation", "cursor", "page_size"}
	var gotQuery []string
	for _, p := range cf.Params {
		if p.Location == "query" {
			gotQuery = append(gotQuery, p.Name)
		}
	}
	if !equalStrings(sortedCopy(gotQuery), wantQuery) {
		t.Fatalf("listGoals query params: want %v, got %v", wantQuery, gotQuery)
	}

	// Exactly 3 untyped-query diagnostics, one per query param, each with file:line.
	var untyped []facts2Diag
	for _, d := range diags {
		if strings.Contains(d.Message, "untyped query param") {
			untyped = append(untyped, facts2Diag{d.Message, d.File, d.Line})
		}
	}
	if len(untyped) != 3 {
		t.Fatalf("want exactly 3 untyped-query diagnostics, got %d: %+v", len(untyped), untyped)
	}
	for _, name := range wantQuery {
		found := false
		for _, d := range untyped {
			if strings.Contains(d.msg, "'"+name+"'") {
				found = true
				if d.file == "" || d.line == 0 {
					t.Errorf("untyped-query diag for %s missing file:line: %+v", name, d)
				}
			}
		}
		if !found {
			t.Errorf("no untyped-query diagnostic for query param %q", name)
		}
	}
}

func TestMultipartFormBodiesFromTypedBindAndDirectCalls(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/uploadhandlers

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import "net/http"

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{ Request *http.Request }

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBind(any) error { return nil }
func (c *Context) FormFile(string) (any, error) { return nil, nil }
func (c *Context) PostForm(string) string { return "" }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package uploadhandlers

import (
	"mime/multipart"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }

type UploadForm struct {
	File *multipart.FileHeader `+"`form:\"file\" binding:\"required\"`"+`
	Name string `+"`form:\"name\" validate:\"required\"`"+`
}

type UploadResult struct {
	ID string `+"`json:\"id\"`"+`
}

func (s Server) Register() {
	s.R.POST("/upload", s.uploadTyped)
	s.R.POST("/loose", s.uploadLoose)
	s.R.POST("/request", s.uploadRequest)
	s.R.POST("/named-gin", s.uploadNamedGin)
	s.R.POST("/named-request", s.uploadNamedRequest)
}

func fileFieldName() string { return "named" }

func (s Server) uploadTyped(c *gin.Context) {
	var in UploadForm
	_ = c.ShouldBind(&in)
	c.JSON(201, UploadResult{})
}

func (s Server) uploadLoose(c *gin.Context) {
	_, _ = c.FormFile("file")
	_ = c.PostForm("name")
	c.JSON(200, UploadResult{})
}

func (s Server) uploadRequest(c *gin.Context) {
	_, _, _ = c.Request.FormFile("file")
	_ = c.PostForm("name")
	c.JSON(200, UploadResult{})
}

func (s Server) uploadNamedGin(c *gin.Context) {
	_, _ = c.FormFile(fileFieldName())
	c.JSON(200, UploadResult{})
}

func (s Server) uploadNamedRequest(c *gin.Context) {
	_, _, _ = c.Request.FormFile(fileFieldName())
	c.JSON(200, UploadResult{})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load upload handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/uploadhandlers", diags)
	got := map[string]handlers.CodeFacts{}
	for _, r := range routes.Recognize(res) {
		got[r.Handler] = analyzer.Analyze(r, diags)
	}

	typed := got["uploadTyped"]
	if typed.RequestBody == nil || !strings.HasSuffix(typed.RequestBody.RefID, "UploadForm") {
		t.Fatalf("typed upload body should reference UploadForm, got %+v", typed.RequestBody)
	}
	if typed.RequestBodyContentType != "multipart/form-data" {
		t.Fatalf("typed upload content type: want multipart/form-data got %q", typed.RequestBodyContentType)
	}

	loose := got["uploadLoose"]
	if loose.RequestBody == nil || loose.RequestBody.RefID != "__synthetic.UploadLooseFormRequest" {
		t.Fatalf("loose upload should synthesize request schema, got %+v", loose.RequestBody)
	}
	if loose.RequestBodyContentType != "multipart/form-data" {
		t.Fatalf("loose upload content type: want multipart/form-data got %q", loose.RequestBodyContentType)
	}
	if len(loose.Schemas) != 1 || loose.Schemas[0].Name != "UploadLooseFormRequest" {
		t.Fatalf("loose upload synthetic schemas: %+v", loose.Schemas)
	}
	fields, ok := loose.Schemas[0].Body.Of.([]facts.FieldFact)
	if !ok {
		t.Fatalf("synthetic schema body should be object fields, got %+v", loose.Schemas[0].Body)
	}
	seen := map[string]facts.FieldFact{}
	for _, field := range fields {
		seen[field.JSONName] = field
	}
	if primName(seen["file"].Schema) != facts.PrimBytes || !seen["file"].ValidatorRequiresPresence {
		t.Fatalf("synthetic file field should be required bytes, got %+v", seen["file"])
	}
	if primName(seen["name"].Schema) != facts.PrimString || seen["name"].ValidatorRequiresPresence {
		t.Fatalf("synthetic name field should be optional string, got %+v", seen["name"])
	}

	request := got["uploadRequest"]
	if request.RequestBody == nil || request.RequestBody.RefID != "__synthetic.UploadRequestFormRequest" || request.RequestBodyContentType != "multipart/form-data" {
		t.Fatalf("net/http Request.FormFile should synthesize a multipart request schema, got %+v", request)
	}
	if len(request.Schemas) != 1 {
		t.Fatalf("net/http Request.FormFile synthetic schemas: %+v", request.Schemas)
	}
	requestFields, ok := request.Schemas[0].Body.Of.([]facts.FieldFact)
	if !ok {
		t.Fatalf("net/http Request.FormFile schema should be object fields, got %+v", request.Schemas[0].Body)
	}
	requestSeen := map[string]facts.FieldFact{}
	for _, field := range requestFields {
		requestSeen[field.JSONName] = field
	}
	if primName(requestSeen["file"].Schema) != facts.PrimBytes || !requestSeen["file"].ValidatorRequiresPresence || primName(requestSeen["name"].Schema) != facts.PrimString {
		t.Fatalf("net/http Request.FormFile should share Gin's multipart field representation: %+v", requestSeen)
	}

	// A field name the handler-scoped resolver can reach must resolve the same way
	// through both access paths; otherwise Request.FormFile silently drops the body.
	for _, handler := range []string{"uploadNamedGin", "uploadNamedRequest"} {
		named := got[handler]
		if named.RequestBody == nil || named.RequestBodyContentType != "multipart/form-data" {
			t.Fatalf("%s should synthesize a multipart request body, got %+v", handler, named)
		}
		if len(named.Schemas) != 1 {
			t.Fatalf("%s synthetic schemas: %+v", handler, named.Schemas)
		}
		namedFields, ok := named.Schemas[0].Body.Of.([]facts.FieldFact)
		if !ok || len(namedFields) != 1 {
			t.Fatalf("%s schema should hold one object field, got %+v", handler, named.Schemas[0].Body)
		}
		if namedFields[0].JSONName != "named" || primName(namedFields[0].Schema) != facts.PrimBytes || !namedFields[0].ValidatorRequiresPresence {
			t.Fatalf("%s should resolve the constant-returning helper name: %+v", handler, namedFields[0])
		}
	}
	for _, diagnostic := range diags.Items() {
		if diagnostic.Code == "request.body.unresolved" {
			t.Fatalf("no upload handler has a dynamic field name: %+v", diagnostic)
		}
	}
}

func TestBranchingContentTypeHelperIsDynamic(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/downloadhandlers

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import "io"

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) DataFromReader(int, int64, string, io.Reader, map[string]string) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package downloadhandlers

import (
	"strings"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
type Other struct{}

var preferPDF bool

func (s Server) Register() {
	s.R.GET("/file", s.file)
	s.R.GET("/method-file", s.methodFile)
}

func (s Server) file(c *gin.Context) {
	c.DataFromReader(200, 12, dynamicContentType(), strings.NewReader("hello"), nil)
}

func (s Server) methodFile(c *gin.Context) {
	var other Other
	c.DataFromReader(200, 12, other.ContentType(), strings.NewReader("hello"), nil)
}

func ContentType() string {
	return "application/pdf"
}

func (Other) ContentType() string {
	return "text/plain"
}

func dynamicContentType() string {
	if preferPDF {
		return "application/pdf"
	}
	return "text/plain"
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load download handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/downloadhandlers", diags)
	got := map[string]handlers.CodeFacts{}
	for _, r := range routes.Recognize(res) {
		got[r.Handler] = analyzer.Analyze(r, diags)
	}
	file := got["file"]
	if len(file.Responses) != 1 || file.Responses[0].ContentType != "application/octet-stream" {
		t.Fatalf("branching content type helper should fall back to octet-stream, got %+v", file.Responses)
	}
	methodFile := got["methodFile"]
	if len(methodFile.Responses) != 1 || methodFile.Responses[0].ContentType != "application/octet-stream" {
		t.Fatalf("method content type helper should not fold through a same-named top-level helper, got %+v", methodFile.Responses)
	}
	foundOperations := map[string]bool{}
	for _, item := range diags.Items() {
		if strings.Contains(item.Message, "DataFromReader content type is dynamic") {
			foundOperations[item.Operation] = true
		}
	}
	if !foundOperations["GET /file"] || !foundOperations["GET /method-file"] {
		t.Fatalf("dynamic-content diagnostics should identify both operations, got %+v", diags.Items())
	}
}

func TestDelegatedBinaryResponseHelpersAreAnalyzed(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/delegatedbinary

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import "io"

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Header(string, string) {}
func (c *Context) Data(int, string, []byte) {}
func (c *Context) DataFromReader(int, int64, string, io.Reader, map[string]string) {}
func (c *Context) File(string) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package delegatedbinary

import (
	"io"
	"strings"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
type Other struct{}
type attachment struct {
	Reader io.Reader
	SizeBytes int64
	ContentType string
}

func (s Server) Register() {
	s.R.GET("/download", s.download)
	s.R.GET("/content", s.content)
	s.R.GET("/file", s.file)
}

func (s Server) download(c *gin.Context) {
	s.serveAttachment(c, false)
}

func (s Server) serveAttachment(c *gin.Context, inline bool) {
	attachment := loadAttachment(c)
	c.Header("Content-Disposition", "attachment")
	c.DataFromReader(200, attachment.SizeBytes, attachment.ContentType, attachment.Reader, nil)
}

func (Other) serveAttachment(c *gin.Context, inline bool) {}

func loadAttachment(c *gin.Context) attachment {
	return attachment{Reader: strings.NewReader("hello"), SizeBytes: 5}
}

func (s Server) content(c *gin.Context) {
	writeAttachmentContent(c, dynamicContentType(c), "file.txt", []byte("hello"))
}

func (s Server) file(c *gin.Context) {
	c.Header("Content-Type", "application/pdf")
	s.serveFile(c)
}

func (s Server) serveFile(c *gin.Context) {
	c.File("/tmp/report.pdf")
}

func writeAttachmentContent(c *gin.Context, contentType, filename string, body []byte) {
	if contentType == "" {
		contentType = "application/octet-stream"
	}
	c.Header("Content-Disposition", "attachment; filename="+filename)
	c.Data(200, contentType, body)
}

func dynamicContentType(c *gin.Context) string {
	return ""
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load delegated binary handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/delegatedbinary", diags)
	got := map[string]handlers.CodeFacts{}
	for _, r := range routes.Recognize(res) {
		got[r.Handler] = analyzer.Analyze(r, diags)
	}

	assertBinaryOctetStreamResponse(t, got["download"].Responses)
	assertBinaryOctetStreamResponse(t, got["content"].Responses)
	assertBinaryResponse(t, got["file"].Responses, "application/pdf")
}

func TestPathParamHelperReturnTypeSetsSchema(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/pathhelpers

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	github.com/google/uuid v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub
replace github.com/google/uuid => ./uuidstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	if err := os.Mkdir(filepath.Join(dir, "uuidstub"), 0o755); err != nil {
		t.Fatalf("mkdir uuidstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Param(string) string { return "" }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "uuidstub", "go.mod"), "module github.com/google/uuid\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "uuidstub", "uuid.go"), `package uuid

type UUID [16]byte

func Parse(string) (UUID, error) { return UUID{}, nil }
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package pathhelpers

import (
	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
)

type Server struct{ R *gin.Engine }
type File struct {
	ID string `+"`json:\"id\"`"+`
}

func (s Server) Register() {
	s.R.GET("/files/:fileId", s.getFile)
}

func (s Server) getFile(c *gin.Context) {
	fileID, err := parsePathUUID(c, "fileId")
	_, _ = fileID, err
	c.JSON(200, File{})
}

func parsePathUUID(c *gin.Context, key string) (uuid.UUID, error) {
	return uuid.Parse(c.Param(key))
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load path helper handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/pathhelpers", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		if r.Handler == "getFile" {
			cf = analyzer.Analyze(r, diags)
		}
	}

	param, ok := paramByName(cf.Params, "fileId")
	if !ok {
		t.Fatalf("missing helper-derived fileId param: %+v", cf.Params)
	}
	if param.Location != "path" || !param.Required {
		t.Fatalf("fileId param should be required path param, got %+v", param)
	}
	if param.Schema.Type != facts.TypeWellKnown || param.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("fileId helper return type should infer uuid schema, got %+v", param.Schema)
	}
}

func TestDataApplicationJSONBytesAreRawBinaryResponses(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/rawjson

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Data(int, string, []byte) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package rawjson

import "github.com/gin-gonic/gin"

type Server struct{ R *gin.Engine }

func (s Server) Register() {
	s.R.GET("/export", s.export)
}

func (s Server) export(c *gin.Context) {
	payload := []byte(`+"`"+`{"status":"ready"}`+"`"+`)
	c.Data(200, "application/json", payload)
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load raw json handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/rawjson", diags)
	got := map[string]handlers.CodeFacts{}
	for _, r := range routes.Recognize(res) {
		got[r.Handler] = analyzer.Analyze(r, diags)
	}

	response := got["export"].Responses[0]
	if response.BodyKind != "binary" {
		t.Fatalf("c.Data application/json bytes must be marked binary, got %+v", response)
	}
	if response.ContentType != "application/json" {
		t.Fatalf("raw JSON response content type: got %+v", response)
	}
	if !equalStrings(response.ContentTypes, []string{"application/json"}) {
		t.Fatalf("raw JSON response content_types: got %+v", response)
	}
}

func TestRouteHandlerIdentityDisambiguatesSameNamedMethods(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/identityhandlers

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	for _, pkg := range []string{"a", "b"} {
		if err := os.MkdirAll(filepath.Join(dir, "internal", pkg, "ports"), 0o755); err != nil {
			t.Fatalf("mkdir package %s: %v", pkg, err)
		}
		typePrefix := strings.ToUpper(pkg)
		mustWrite(t, filepath.Join(dir, "internal", pkg, "ports", "handlers.go"), `package ports

import "github.com/gin-gonic/gin"

type Server struct {
	R *gin.Engine
	H Handler
}

type Handler struct{}
type `+typePrefix+`Request struct { Name string `+"`json:\"name\"`"+` }
type `+typePrefix+`Response struct { ID string `+"`json:\"id\"`"+` }

func (s Server) Register() {
	s.R.POST("/`+pkg+`", s.H.Handle)
}

func (Handler) Handle(c *gin.Context) {
	var input `+typePrefix+`Request
	_ = c.ShouldBindJSON(&input)
	c.JSON(200, `+typePrefix+`Response{})
}
`)
	}

	res, err := load.Load(dir, "./internal/.../ports")
	if err != nil {
		t.Fatalf("load identity handlers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/identityhandlers", diags)
	recognized := routes.Recognize(res)
	analyzer.ReportRouteHandlerCollisions(recognized, diags)
	got := map[string]handlers.CodeFacts{}
	for _, r := range recognized {
		if r.HandlerKey == "" {
			t.Fatalf("recognized route %s %s should carry handler identity: %+v", r.Method, r.Path, r)
		}
		got[r.Path] = analyzer.Analyze(r, diags)
	}
	for _, item := range diags.Items() {
		if strings.Contains(item.Message, "duplicate handler name 'Handle'") {
			t.Fatalf("resolved route identities should not warn about bare-name Handle collisions: %+v", diags.Items())
		}
	}
	assertBodySuffix(t, got["/a"].RequestBody, "internal/a/ports.ARequest")
	assertResponseSuffix(t, got["/a"].Responses, 200, "internal/a/ports.AResponse")
	assertBodySuffix(t, got["/b"].RequestBody, "internal/b/ports.BRequest")
	assertResponseSuffix(t, got["/b"].Responses, 200, "internal/b/ports.BResponse")
	if got["/a"].RequestBodyContentType != "application/json" || got["/b"].RequestBodyContentType != "application/json" {
		t.Fatalf("ShouldBindJSON routes should infer application/json content types: a=%q b=%q", got["/a"].RequestBodyContentType, got["/b"].RequestBodyContentType)
	}
}

func TestSamePackageQueryHelpersInferTypeAndRequiredness(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/queryhelpers

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	github.com/google/uuid v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub
replace github.com/google/uuid => ./uuidstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	if err := os.Mkdir(filepath.Join(dir, "uuidstub"), 0o755); err != nil {
		t.Fatalf("mkdir uuidstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Query(string) string { return "" }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "uuidstub", "go.mod"), "module github.com/google/uuid\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "uuidstub", "uuid.go"), `package uuid

type UUID [16]byte

var Nil UUID

func Parse(string) (UUID, error) { return UUID{}, nil }
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package queryhelpers

import (
	"errors"
	"strings"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
)

type Server struct{ R *gin.Engine }
type Result struct { OK bool `+"`json:\"ok\"`"+` }

func (s Server) Register() {
	s.R.GET("/search", s.search)
}

func (s Server) search(c *gin.Context) {
	branchID, _ := optionalQueryUUID(c, "branchId", uuid.Nil)
	orgID, _ := requiredQueryUUID(c, "orgId")
	_, _ = branchID, orgID
	c.JSON(200, Result{})
}

func optionalQueryUUID(c *gin.Context, key string, fallback uuid.UUID) (uuid.UUID, error) {
	raw := strings.TrimSpace(c.Query(key))
	if raw == "" {
		return fallback, nil
	}
	return uuid.Parse(raw)
}

func requiredQueryUUID(c *gin.Context, key string) (uuid.UUID, error) {
	raw := strings.TrimSpace(c.Query(key))
	if raw == "" {
		return uuid.UUID{}, errors.New("missing")
	}
	return uuid.Parse(raw)
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load query helpers: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/queryhelpers", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	branch, ok := paramByName(cf.Params, "branchId")
	if !ok || branch.Required || branch.Schema.Type != facts.TypeWellKnown || branch.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("branchId should be optional uuid query param, got %+v", branch)
	}
	org, ok := paramByName(cf.Params, "orgId")
	if !ok || !org.Required || org.Schema.Type != facts.TypeWellKnown || org.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("orgId should be required uuid query param, got %+v", org)
	}
}

func TestDirectQueryRequirednessFollowsProvenControlFlow(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/queryflow

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Query(string) string { return "" }
func (c *Context) GetQuery(string) (string, bool) { return "", false }
func (c *Context) GetHeader(string) string { return "" }
func (c *Context) JSON(int, any) {}
func (c *Context) String(int, string, ...any) {}
func (c *Context) AbortWithError(int, error) error { return nil }
func (c *Context) Abort() {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package queryflow

import (
	"errors"
	"strconv"
	"strings"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
type Result struct { OK bool `+"`json:\"ok\"`"+` }

func (s Server) Register() {
	s.R.GET("/required", s.required)
	s.R.GET("/inverted", s.inverted)
	s.R.GET("/optional", s.optional)
	s.R.GET("/present", s.present)
	s.R.GET("/early-nested", s.earlyNested)
	s.R.GET("/multiple", s.multiple)
	s.R.GET("/bare", s.bare)
	s.R.GET("/ambiguous-nested", s.ambiguousNested)
	s.R.GET("/reassigned-alias", s.reassignedAlias)
	s.R.GET("/helper-condition", s.helperCondition)
	s.R.GET("/normalized-read", s.normalizedRead)
	s.R.GET("/nonterminal-error", s.nonterminalError)
	s.R.GET("/loop-guard", s.loopGuard)
	s.R.GET("/string-rejected", s.stringRejected)
	s.R.GET("/aborted-with-error", s.abortedWithError)
	s.R.GET("/silent-abort", s.silentAbort)
	s.R.GET("/parsed-guard", s.parsedGuard)
	s.R.GET("/init-guard", s.initGuard)
	s.R.GET("/escaping-default", s.escapingDefault)
	s.R.GET("/closure-write", s.closureWrite)
}

func (s Server) required(c *gin.Context) {
	value := c.Query("name")
	alias := value
	if alias == "" {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) inverted(c *gin.Context) {
	if c.Query("name") != "" {
		use("present")
	} else {
		c.JSON(422, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) optional(c *gin.Context) {
	value := c.Query("name")
	if value != "" {
		use(value)
	}
	c.JSON(200, Result{})
}

func (s Server) present(c *gin.Context) {
	value, present := c.GetQuery("name")
	_, _ = value, present
	c.JSON(200, Result{})
}

func (s Server) earlyNested(c *gin.Context) {
	value := c.Query("name")
	if len(value) == 0 {
		if c.GetHeader("X-Mode") == "strict" {
			c.JSON(400, Result{})
			return
		}
		c.JSON(422, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) multiple(c *gin.Context) {
	first := c.Query("name")
	if first == "" {
		c.JSON(400, Result{})
		return
	}
	second := c.Query("name")
	use(second)
	c.JSON(200, Result{})
}

func (s Server) bare(c *gin.Context) {
	use(c.Query("name"))
	c.JSON(200, Result{})
}

func (s Server) ambiguousNested(c *gin.Context) {
	value := c.Query("name")
	if value == "" {
		if choose() {
			c.JSON(400, Result{})
			return
		}
	}
	c.JSON(200, Result{})
}

func (s Server) reassignedAlias(c *gin.Context) {
	value := c.Query("name")
	alias := value
	alias = "fallback"
	if alias == "" {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) helperCondition(c *gin.Context) {
	value := c.Query("name")
	if helperRejects(value) {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) normalizedRead(c *gin.Context) {
	value := strings.TrimSpace(c.Query("name"))
	use(value)
	c.JSON(200, Result{})
}

func (s Server) nonterminalError(c *gin.Context) {
	value := c.Query("name")
	if value == "" {
		c.JSON(400, Result{})
	}
	c.JSON(200, Result{})
}

func (s Server) loopGuard(c *gin.Context) {
	value := c.Query("name")
	for choose() {
		if value == "" {
			c.JSON(400, Result{})
			return
		}
	}
	c.JSON(200, Result{})
}

func (s Server) stringRejected(c *gin.Context) {
	value := c.Query("name")
	if value == "" {
		c.String(400, "name is required")
		return
	}
	c.JSON(200, Result{})
}

func (s Server) abortedWithError(c *gin.Context) {
	value := c.Query("name")
	if value == "" {
		_ = c.AbortWithError(422, errors.New("name is required"))
		return
	}
	c.JSON(200, Result{})
}

func (s Server) silentAbort(c *gin.Context) {
	value := c.Query("name")
	if value == "" {
		c.Abort()
		return
	}
	c.JSON(200, Result{})
}

func (s Server) parsedGuard(c *gin.Context) {
	if c.Query("limit") == "" {
		c.JSON(400, Result{})
		return
	}
	limit, err := strconv.Atoi(c.Query("limit"))
	if err != nil {
		c.JSON(400, Result{})
		return
	}
	use(strconv.Itoa(limit))
	c.JSON(200, Result{})
}

func (s Server) initGuard(c *gin.Context) {
	if value := c.Query("name"); value == "" {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) escapingDefault(c *gin.Context) {
	value := c.Query("name")
	applyDefault(&value)
	if value == "" {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func (s Server) closureWrite(c *gin.Context) {
	value := c.Query("name")
	fill := func() { value = "filled" }
	fill()
	if value == "" {
		c.JSON(400, Result{})
		return
	}
	c.JSON(200, Result{})
}

func applyDefault(value *string) {
	if *value == "" {
		*value = "fallback"
	}
}

func use(string) {}
func choose() bool { return false }
func helperRejects(string) bool { return false }
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load direct query control flow: %v", err)
	}
	// load.Load reports per-package type errors instead of failing, so a fixture
	// that does not compile would make every assertion below pass vacuously.
	for _, loadErr := range res.Errors {
		t.Fatalf("query control flow fixture must type-check: %+v", loadErr)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/queryflow", diagnostics)
	byPath := map[string]handlers.CodeFacts{}
	for _, route := range routes.Recognize(res) {
		byPath[route.Path] = analyzer.Analyze(route, diagnostics)
	}

	for _, path := range []string{
		"/required", "/inverted", "/early-nested", "/multiple",
		"/string-rejected", "/aborted-with-error", "/init-guard",
	} {
		param, ok := paramByName(byPath[path].Params, "name")
		if !ok || !param.Required {
			t.Fatalf("%s should prove name required, got %+v", path, param)
		}
	}
	for _, path := range []string{"/optional", "/present"} {
		param, ok := paramByName(byPath[path].Params, "name")
		if !ok || param.Required {
			t.Fatalf("%s should prove name optional, got %+v", path, param)
		}
	}

	unresolved := map[string]bool{}
	for _, item := range diagnostics.Items() {
		if item.Code == "request.parameter.unresolved" && item.Subject == "name" {
			unresolved[item.Operation] = true
		}
	}
	for _, path := range []string{"/bare", "/ambiguous-nested", "/reassigned-alias", "/helper-condition", "/normalized-read", "/nonterminal-error", "/loop-guard", "/silent-abort", "/escaping-default", "/closure-write"} {
		operation := "GET " + path
		if !unresolved[operation] {
			t.Fatalf("%s should retain deterministic unresolved requiredness: %+v", operation, diagnostics.Items())
		}
	}
	for _, path := range []string{
		"/required", "/inverted", "/optional", "/present", "/early-nested", "/multiple",
		"/string-rejected", "/aborted-with-error", "/init-guard",
	} {
		operation := "GET " + path
		if unresolved[operation] {
			t.Fatalf("%s has proven requiredness but emitted unresolved: %+v", operation, diagnostics.Items())
		}
	}

	// A proven direct read settles requiredness; it must not overwrite the schema
	// a parser around the same read already proved.
	parsed, ok := paramByName(byPath["/parsed-guard"].Params, "limit")
	if !ok || !parsed.Required || primName(parsed.Schema) != "int" {
		t.Fatalf("/parsed-guard should keep the parsed int schema and prove required, got %+v", parsed)
	}
}

func TestGenericJSONBodyHelperInfersRequestBody(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/genericbody

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package genericbody

import "github.com/gin-gonic/gin"

type Server struct{ R *gin.Engine }
type CreateRequest struct { Name string `+"`json:\"name\"`"+` }
type CreateResponse struct { ID string `+"`json:\"id\"`"+` }

func (s Server) Register() {
	s.R.POST("/create", s.create)
}

func (s Server) create(c *gin.Context) {
	input, err := parseRequest[CreateRequest](c)
	_, _ = input, err
	c.JSON(200, CreateResponse{})
}

func parseRequest[T any](c *gin.Context) (T, error) {
	var input T
	if err := c.ShouldBindJSON(&input); err != nil {
		return input, err
	}
	return input, nil
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load generic body: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/genericbody", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	assertBodySuffix(t, cf.RequestBody, "CreateRequest")
	if cf.RequestBodyContentType != "application/json" {
		t.Fatalf("generic JSON helper content type: want application/json got %q", cf.RequestBodyContentType)
	}
}

func TestGenericJSONBodyHelperPreservesConcreteEnvelopeBind(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/genericbodyguard

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package genericbodyguard

import "github.com/gin-gonic/gin"

type Server struct{ R *gin.Engine }
type Envelope struct { Token string `+"`json:\"token\"`"+` }
type CreateRequest struct { Name string `+"`json:\"name\"`"+` }
type CreateResponse struct { ID string `+"`json:\"id\"`"+` }

func (s Server) Register() {
	s.R.POST("/create", s.create)
}

func (s Server) create(c *gin.Context) {
	input, err := parseEnvelope[CreateRequest](c)
	_, _ = input, err
	c.JSON(200, CreateResponse{})
}

func parseEnvelope[T any](c *gin.Context) (T, error) {
	var zero T
	var envelope Envelope
	if err := c.ShouldBindJSON(&envelope); err != nil {
		return zero, err
	}
	return zero, nil
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load generic body guard: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/genericbodyguard", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	assertBodySuffix(t, cf.RequestBody, "Envelope")
	if cf.RequestBodyContentType != "application/json" {
		t.Fatalf("generic helper's concrete envelope bind should remain the request body, got body=%+v content_type=%q", cf.RequestBody, cf.RequestBodyContentType)
	}
}

func TestDelegatedSamePackageJSONBindHelperInfersRequestBody(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/delegatedbody

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package delegatedbody

import "github.com/gin-gonic/gin"

type HttpServer struct{ R *gin.Engine }
type CreateEventInput struct { Name string `+"`json:\"name\"`"+` }
type CreatedResponse struct { OK bool `+"`json:\"ok\"`"+` }
type ErrorResponse struct { Message string `+"`json:\"message\"`"+` }

func (h HttpServer) Register() {
	h.R.POST("/events", h.publishEvent)
}

func (h HttpServer) publishEvent(c *gin.Context) {
	input, err := h.bindEventInput(c)
	if err != nil {
		c.JSON(400, ErrorResponse{})
		return
	}
	_ = input
	c.JSON(201, CreatedResponse{})
}

func (h HttpServer) bindEventInput(c *gin.Context) (CreateEventInput, error) {
	var input CreateEventInput
	if err := c.ShouldBindJSON(&input); err != nil {
		return CreateEventInput{}, err
	}
	return input, nil
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load delegated body: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/delegatedbody", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	assertBodySuffix(t, cf.RequestBody, "CreateEventInput")
	if !cf.RequestBodyRequired || cf.RequestBodyContentType != "application/json" {
		t.Fatalf("delegated JSON bind should be required application/json, got required=%v content_type=%q", cf.RequestBodyRequired, cf.RequestBodyContentType)
	}
}

func TestHelperBasedQueryPatternsInferConcreteSchemasRequirednessAndDefaults(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/querypatterns

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	github.com/google/uuid v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub
replace github.com/google/uuid => ./uuidstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	if err := os.Mkdir(filepath.Join(dir, "uuidstub"), 0o755); err != nil {
		t.Fatalf("mkdir uuidstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Query(string) string { return "" }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "uuidstub", "go.mod"), "module github.com/google/uuid\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "uuidstub", "uuid.go"), `package uuid

type UUID [16]byte

var Nil UUID

func Parse(string) (UUID, error) { return UUID{}, nil }
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package querypatterns

import (
	"errors"
	"strconv"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
)

type Server struct{ R *gin.Engine }
type Result struct { OK bool `+"`json:\"ok\"`"+` }

func (s Server) Register() {
	s.R.GET("/things", s.listThings)
}

func (s Server) listThings(c *gin.Context) {
	branchID, branchErr := optionalQueryUUID(c, "branchId")
	id, idErr := requiredQueryUUID(c, "id")
	pageSize, pageErr := optionalUintQuery(c, "pageSize", 20)
	startDate, timeErr := ParseTime(c.Query("startDate"))
	_, _, _, _, _, _, _, _ = branchID, branchErr, id, idErr, pageSize, pageErr, startDate, timeErr
	c.JSON(200, Result{})
}

func optionalQueryUUID(c *gin.Context, key string) (*uuid.UUID, error) {
	value := c.Query(key)
	if value == "" {
		return nil, nil
	}
	parsed, err := uuid.Parse(value)
	if err != nil {
		return nil, err
	}
	return &parsed, nil
}

func requiredQueryUUID(c *gin.Context, key string) (uuid.UUID, error) {
	value := c.Query(key)
	if value == "" {
		return uuid.Nil, errors.New("missing")
	}
	return uuid.Parse(value)
}

func optionalUintQuery(c *gin.Context, key string, defaultValue uint) (uint, error) {
	value := c.Query(key)
	if value == "" {
		return defaultValue, nil
	}
	parsed, err := strconv.ParseUint(value, 10, 32)
	if err != nil {
		return 0, err
	}
	return uint(parsed), nil
}

func ParseTime(value string) (*time.Time, error) {
	if value == "" {
		return nil, nil
	}
	parsed, err := time.Parse(time.RFC3339, value)
	if err != nil {
		return nil, err
	}
	return &parsed, nil
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load query patterns: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/querypatterns", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	branch, ok := paramByName(cf.Params, "branchId")
	if !ok || branch.Required || branch.Schema.Type != facts.TypeWellKnown || branch.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("branchId should be optional uuid query param, got %+v", branch)
	}
	id, ok := paramByName(cf.Params, "id")
	if !ok || !id.Required || id.Schema.Type != facts.TypeWellKnown || id.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("id should be required uuid query param, got %+v", id)
	}
	pageSize, ok := paramByName(cf.Params, "pageSize")
	if !ok || pageSize.Required || primName(pageSize.Schema) != facts.PrimInt || pageSize.Default == nil || pageSize.Default.Type != "number" || pageSize.Default.Value != "20" {
		t.Fatalf("pageSize should be optional int query param with default 20, got %+v", pageSize)
	}
	startDate, ok := paramByName(cf.Params, "startDate")
	if !ok || startDate.Required || startDate.Schema.Type != facts.TypeWellKnown || startDate.Schema.Of != facts.WellKnownDateTime {
		t.Fatalf("startDate should be optional date_time query param, got %+v", startDate)
	}
	for _, item := range diags.Items() {
		if strings.Contains(item.Message, "untyped query param") {
			t.Fatalf("typed helper query params should not emit untyped diagnostics: %+v", item)
		}
	}
}

func TestGetRawDataWithJSONUsageSynthesizesFreeFormJSONBody(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/rawjsonbody

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) GetRawData() ([]byte, error) { return nil, nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package rawjsonbody

import (
	"encoding/json"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
type Result struct { OK bool `+"`json:\"ok\"`"+` }

func (s Server) Register() {
	s.R.POST("/ingest", s.ingest)
}

func (s Server) ingest(c *gin.Context) {
	raw, _ := c.GetRawData()
	var payload map[string]any
	_ = json.Unmarshal(raw, &payload)
	c.JSON(200, Result{})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load raw json body: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/rawjsonbody", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	if cf.RequestBody == nil || cf.RequestBody.RefID != "__synthetic.IngestRawJSONRequest" {
		t.Fatalf("raw JSON body should synthesize free-form request schema, got %+v", cf.RequestBody)
	}
	if cf.RequestBodyContentType != "application/json" {
		t.Fatalf("raw JSON body content type: want application/json got %q", cf.RequestBodyContentType)
	}
	if len(cf.Schemas) != 1 || cf.Schemas[0].Body.Type != facts.TypeAny {
		t.Fatalf("raw JSON body schema should be Any, got %+v", cf.Schemas)
	}
}

func TestGetRawDataIgnoresUnrelatedJSONStringEvidence(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/rawjsonbodyguard

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) GetRawData() ([]byte, error) { return nil, nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package rawjsonbodyguard

import (
	"strings"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
type Result struct { OK bool `+"`json:\"ok\"`"+` }

func (s Server) Register() {
	s.R.POST("/ingest", s.ingest)
}

func (s Server) ingest(c *gin.Context) {
	raw, _ := c.GetRawData()
	_ = raw
	_ = strings.Contains("application/json", "json")
	c.JSON(200, Result{})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load raw json body guard: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/rawjsonbodyguard", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	if cf.RequestBody != nil || cf.RequestBodyContentType != "" || len(cf.Schemas) != 0 {
		t.Fatalf("unrelated JSON string evidence should not synthesize raw JSON body, got body=%+v content_type=%q schemas=%+v", cf.RequestBody, cf.RequestBodyContentType, cf.Schemas)
	}
}

func TestTypedSuccessResponseOutranksErrorishSameStatusBranch(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/successresponse

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package successresponse

import "github.com/gin-gonic/gin"

var fail bool

type Server struct{ R *gin.Engine }
type ErrorResponse struct { Message string `+"`json:\"message\"`"+` }
type SuccessResponse struct { ID string `+"`json:\"id\"`"+` }

func (s Server) Register() {
	s.R.GET("/thing", s.getThing)
}

func (s Server) getThing(c *gin.Context) {
	if fail {
		c.JSON(200, ErrorResponse{})
		return
	}
	c.JSON(200, SuccessResponse{})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load success response: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/successresponse", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	assertResponseSuffix(t, cf.Responses, 200, "SuccessResponse")
}

func TestSyntheticSuccessResponseOutranksErrorishSameStatusBranch(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/syntheticsuccessresponse

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type H map[string]any
type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package syntheticsuccessresponse

import "github.com/gin-gonic/gin"

var fail bool

type Server struct{ R *gin.Engine }
type ErrorResponse struct { Message string `+"`json:\"message\"`"+` }

func (s Server) Register() {
	s.R.GET("/thing", s.getThing)
}

func (s Server) getThing(c *gin.Context) {
	if fail {
		c.JSON(200, ErrorResponse{})
		return
	}
	c.JSON(200, gin.H{"id": "ok"})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load synthetic success response: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/syntheticsuccessresponse", diags)
	var cf handlers.CodeFacts
	for _, r := range routes.Recognize(res) {
		cf = analyzer.Analyze(r, diags)
	}
	assertResponseSuffix(t, cf.Responses, 200, "GetThing200Response")
}

func TestModuleOwnedMultiHopTypedParametersAndMultipartAreExtracted(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/typedrequests

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	github.com/google/uuid v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub
replace github.com/google/uuid => ./uuidstub
`)
	for _, subdir := range []string{"ginstub", "uuidstub", "requesthelpers"} {
		if err := os.Mkdir(filepath.Join(dir, subdir), 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", subdir, err)
		}
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import (
	"mime/multipart"
	"net/http"
)

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct { Request *http.Request }

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) Query(string) string { return "" }
func (c *Context) GetQuery(string) (string, bool) { return "", false }
func (c *Context) QueryArray(string) []string { return nil }
func (c *Context) GetQueryArray(string) ([]string, bool) { return nil, false }
func (c *Context) QueryMap(string) map[string]string { return nil }
func (c *Context) GetHeader(string) string { return "" }
func (c *Context) Cookie(string) (string, error) { return "", nil }
func (c *Context) GetPostForm(string) (string, bool) { return "", false }
func (c *Context) PostForm(string) string { return "" }
func (c *Context) FormFile(string) (*multipart.FileHeader, error) { return nil, nil }
func (c *Context) ShouldBindQuery(any) error { return nil }
func (c *Context) ShouldBindHeader(any) error { return nil }
func (c *Context) ShouldBind(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "uuidstub", "go.mod"), "module github.com/google/uuid\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "uuidstub", "uuid.go"), `package uuid

type UUID [16]byte
func MustParse(string) UUID { return UUID{} }
`)
	mustWrite(t, filepath.Join(dir, "requesthelpers", "helpers.go"), `package requesthelpers

import (
	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
)

func RequiredQueryUUID(c *gin.Context, name string) uuid.UUID {
	return uuid.MustParse(requiredQuery(c, name))
}

func requiredQuery(c *gin.Context, name string) string {
	return queryValue(c, name)
}

func queryValue(c *gin.Context, name string) string {
	return c.Query(name)
}

func MultipartParts(c *gin.Context, fileName, titleName string) {
	_, _ = c.FormFile(fileName)
	_, _, _ = c.Request.FormFile("requestAttachment")
	_ = c.PostForm(titleName)
}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package typedrequests

import (
	"mime/multipart"
	"time"

	"example.com/typedrequests/requesthelpers"
	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }

type QueryInput struct {
	Statuses []string  `+"`"+`form:"statuses" binding:"required"`+"`"+`
	Tags     []string  `+"`"+`form:"tags" binding:"omitempty,dive,required"`+"`"+`
	Force    bool      `+"`"+`form:"force"`+"`"+`
	Limit    int       `+"`"+`form:"limit,default=25"`+"`"+`
	State    string    `+"`"+`form:"state" binding:"oneof=active paused"`+"`"+`
	Day      time.Time `+"`"+`form:"day" time_format:"2006-01-02"`+"`"+`
	Spaces   []string  `+"`"+`form:"spaces" collection_format:"ssv"`+"`"+`
	Pipes    []string  `+"`"+`form:"pipes" collection_format:"pipes"`+"`"+`
	Tabs     []string  `+"`"+`form:"tabs" collection_format:"tsv"`+"`"+`
}

type HeaderInput struct {
	Signature string `+"`"+`header:"X-Webhook-Signature" binding:"required"`+"`"+`
	Retries   []int  `+"`"+`header:"X-Retry"`+"`"+`
	Accept    string `+"`"+`header:"Accept"`+"`"+`
}

type UploadMetadata struct {
	Source string `+"`"+`json:"source"`+"`"+`
}

type UploadInput struct {
	BoundFile *multipart.FileHeader `+"`"+`form:"boundFile" binding:"required"`+"`"+`
	Metadata  UploadMetadata        `+"`"+`form:"metadata"`+"`"+`
	Caption   string                `+"`"+`form:"caption,default=untitled"`+"`"+`
}

type Response struct { OK bool `+"`"+`json:"ok"`+"`"+` }

func (s Server) Register() { s.R.GET("/search", s.search) }

func (s Server) search(c *gin.Context) {
	var query QueryInput
	var headers HeaderInput
	var upload UploadInput
	_ = c.ShouldBindQuery(&query)
	_ = c.ShouldBindHeader(&headers)
	_ = c.ShouldBind(&upload)
	_ = requesthelpers.RequiredQueryUUID(c, "branchId")
	_ = c.QueryArray("labels")
	_, _ = c.GetQueryArray("optionalLabels")
	_ = c.QueryMap("filters")
	_ = c.GetHeader("X-Direct")
	_ = c.GetHeader("Content-Type")
	_ = c.GetHeader("Authorization")
	_ = c.Request.Header.Get("X-Request-Direct")
	_, _ = c.Cookie("session")
	_, _ = c.GetPostForm("note")
	requesthelpers.MultipartParts(c, "attachment", "title")
	c.JSON(200, Response{OK: true})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load typed request fixture: %v", err)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/typedrequests", diagnostics)
	var code handlers.CodeFacts
	for _, route := range routes.Recognize(res) {
		code = analyzer.Analyze(route, diagnostics)
	}

	branch, ok := paramByName(code.Params, "branchId")
	if !ok || !branch.Required || branch.Location != "query" || branch.Schema.Type != facts.TypeWellKnown || branch.Schema.Of != facts.WellKnownUUID {
		t.Fatalf("multi-hop cross-package UUID query must be exact, got %+v", branch)
	}
	if !strings.Contains(branch.Span.File, "requesthelpers") || branch.Span.StartLine == 0 {
		t.Fatalf("helper-derived parameter must preserve inner source span, got %+v", branch.Span)
	}
	statuses, ok := paramByName(code.Params, "statuses")
	if !ok || !statuses.Required || statuses.Schema.Type != facts.TypeArray || statuses.Style != "form" || statuses.Explode == nil || !*statuses.Explode {
		t.Fatalf("bound query array must preserve type/required/style/explode, got %+v", statuses)
	}
	// `dive,required` forbids empty entries in the list; it does not demand the
	// parameter be sent, so the parameter itself stays optional.
	scopedTags, ok := paramByName(code.Params, "tags")
	if !ok || scopedTags.Required {
		t.Fatalf("required behind dive must not make the parameter required, got %+v", scopedTags)
	}
	force, _ := paramByName(code.Params, "force")
	if primName(force.Schema) != facts.PrimBool || force.Required {
		t.Fatalf("bound bool query mismatch: %+v", force)
	}
	limit, _ := paramByName(code.Params, "limit")
	if primName(limit.Schema) != facts.PrimInt || limit.Default == nil || limit.Default.Type != "number" || limit.Default.Value != "25" {
		t.Fatalf("bound integer/default mismatch: %+v", limit)
	}
	state, _ := paramByName(code.Params, "state")
	if state.Schema.Type != facts.TypeEnum {
		t.Fatalf("binding oneof must become an enum, got %+v", state.Schema)
	}
	day, _ := paramByName(code.Params, "day")
	if day.Schema.Type != facts.TypeWellKnown || day.Schema.Of != facts.WellKnownDate {
		t.Fatalf("time_format date must remain a date, got %+v", day.Schema)
	}
	spaces, _ := paramByName(code.Params, "spaces")
	if spaces.Style != "spaceDelimited" || spaces.Explode == nil || *spaces.Explode {
		t.Fatalf("ssv query serialization mismatch: %+v", spaces)
	}
	pipes, _ := paramByName(code.Params, "pipes")
	if pipes.Style != "pipeDelimited" || pipes.Explode == nil || *pipes.Explode {
		t.Fatalf("pipes query serialization mismatch: %+v", pipes)
	}
	signature, _ := paramByName(code.Params, "X-Webhook-Signature")
	if signature.Location != "header" || !signature.Required || primName(signature.Schema) != facts.PrimString {
		t.Fatalf("bound header mismatch: %+v", signature)
	}
	retries, _ := paramByName(code.Params, "X-Retry")
	if retries.Style != "simple" || retries.Explode == nil || *retries.Explode {
		t.Fatalf("header array serialization mismatch: %+v", retries)
	}
	filters, _ := paramByName(code.Params, "filters")
	if filters.Schema.Type != facts.TypeMap || filters.Style != "deepObject" || filters.Explode == nil || !*filters.Explode {
		t.Fatalf("QueryMap serialization mismatch: %+v", filters)
	}
	for _, headerName := range []string{"X-Direct", "X-Request-Direct"} {
		header, exists := paramByName(code.Params, headerName)
		if !exists || header.Location != "header" || header.Required {
			t.Fatalf("missing direct header %s: %+v", headerName, code.Params)
		}
	}
	for _, headerName := range []string{"Accept", "Content-Type", "Authorization"} {
		if _, exists := paramByName(code.Params, headerName); exists {
			t.Fatalf("OpenAPI representation header %s must not be emitted as a parameter: %+v", headerName, code.Params)
		}
	}
	cookie, _ := paramByName(code.Params, "session")
	if cookie.Location != "cookie" || cookie.Required {
		t.Fatalf("cookie mismatch: %+v", cookie)
	}

	if code.RequestBody == nil || code.RequestBodyContentType != "multipart/form-data" || len(code.Schemas) != 1 {
		t.Fatalf("helper-wrapped multipart body mismatch: body=%+v type=%q schemas=%+v", code.RequestBody, code.RequestBodyContentType, code.Schemas)
	}
	fields, ok := code.Schemas[0].Body.Of.([]facts.FieldFact)
	if !ok {
		t.Fatalf("multipart schema must be an object: %+v", code.Schemas[0].Body)
	}
	byName := map[string]facts.FieldFact{}
	for _, field := range fields {
		byName[field.JSONName] = field
	}
	if primName(byName["attachment"].Schema) != facts.PrimBytes || !byName["attachment"].ValidatorRequiresPresence || primName(byName["requestAttachment"].Schema) != facts.PrimBytes || !byName["requestAttachment"].ValidatorRequiresPresence || primName(byName["title"].Schema) != facts.PrimString || primName(byName["note"].Schema) != facts.PrimString {
		t.Fatalf("multipart fields mismatch: %+v", byName)
	}
	if primName(byName["boundFile"].Schema) != facts.PrimBytes || !byName["boundFile"].ValidatorRequiresPresence {
		t.Fatalf("bound multipart file mismatch: %+v", byName["boundFile"])
	}
	if byName["metadata"].Schema.Type != facts.TypeNamed {
		t.Fatalf("structured multipart metadata mismatch: %+v", byName["metadata"])
	}
	if byName["caption"].Meta == nil || byName["caption"].Meta.Default == nil || byName["caption"].Meta.Default.Value != "untitled" {
		t.Fatalf("bound multipart scalar default mismatch: %+v", byName["caption"])
	}
	foundUnsupportedCollectionFormat := false
	for _, item := range diagnostics.Items() {
		if item.Code == "request.parameter.unresolved" && (item.Subject == "branchId" || strings.Contains(item.Message, "RequiredQueryUUID")) {
			t.Fatalf("resolved multi-hop parameter must not emit incompleteness diagnostic: %+v", item)
		}
		if item.Code == "request.parameter.unresolved" && item.Subject == "tabs" && strings.Contains(item.Message, "tsv") {
			foundUnsupportedCollectionFormat = true
		}
	}
	if !foundUnsupportedCollectionFormat {
		t.Fatalf("unsupported tsv serialization must emit request.parameter.unresolved: %+v", diagnostics.Items())
	}
}

func TestContextHelperCyclesAndExternalBoundariesAreDiagnosed(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/helperdiagnostics

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	example.net/external v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub
replace example.net/external => ./external
`)
	for _, subdir := range []string{"ginstub", "external"} {
		if err := os.Mkdir(filepath.Join(dir, subdir), 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", subdir, err)
		}
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}
func (e *Engine) GET(string, HandlerFunc) {}
`)
	mustWrite(t, filepath.Join(dir, "external", "go.mod"), "module example.net/external\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "external", "external.go"), `package external

import "github.com/gin-gonic/gin"
func Read(*gin.Context) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package helperdiagnostics

import (
	"example.net/external"
	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }
func (s Server) Register() { s.R.GET("/broken", s.broken) }
func (s Server) broken(c *gin.Context) { cycleA(c); external.Read(c) }
func cycleA(c *gin.Context) { cycleB(c) }
func cycleB(c *gin.Context) { cycleA(c) }
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load helper diagnostic fixture: %v", err)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/helperdiagnostics", diagnostics)
	for _, route := range routes.Recognize(res) {
		_ = analyzer.Analyze(route, diagnostics)
	}
	cycle, external := false, false
	for _, item := range diagnostics.Items() {
		if item.Code != "request.parameter.unresolved" || item.Operation != "GET /broken" {
			continue
		}
		cycle = cycle || strings.Contains(item.Message, "cycle detected")
		external = external || strings.Contains(item.Message, "external package example.net/external")
	}
	if !cycle || !external {
		t.Fatalf("cycle and external context boundaries must both be diagnosed: %+v", diagnostics.Items())
	}
}

func TestDirectRequestFactsPreferTypedEvidenceAndDiagnoseEveryIncompleteShape(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/requestdiagnostics

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import (
	"mime/multipart"
	"net/http"
)

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct { Request *http.Request }
func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) Query(string) string { return "" }
func (c *Context) Param(string) string { return "" }
func (c *Context) GetHeader(string) string { return "" }
func (c *Context) Cookie(string) (string, error) { return "", nil }
func (c *Context) FormFile(string) (*multipart.FileHeader, error) { return nil, nil }
func (c *Context) PostForm(string) string { return "" }
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) ShouldBindQuery(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package requestdiagnostics

import "github.com/gin-gonic/gin"

const limitName = "limit"

type Server struct{ R *gin.Engine }
type SearchQuery struct { Limit int `+"`"+`form:"limit"`+"`"+` }
type GoodBody struct { Name string `+"`"+`json:"name"`+"`"+` }
type OtherBody struct { ID string `+"`"+`json:"id"`+"`"+` }
type Response struct { OK bool `+"`"+`json:"ok"`+"`"+` }

func (s Server) Register() { s.R.POST("/broken", s.broken) }

func (s Server) broken(c *gin.Context) {
	var query SearchQuery
	_ = c.Query(limitName)
	_ = c.ShouldBindQuery(&query)

	dynamic := limitName
	_ = c.Param(dynamic)
	_ = c.GetHeader(dynamic)
	_, _ = c.Cookie(dynamic)
	_ = c.Request.Header.Get(dynamic)
	_, _ = c.FormFile(dynamic)
	_ = c.PostForm(dynamic)

	loose := map[string]any{}
	_ = c.ShouldBindJSON(&loose)
	good := new(GoodBody)
	_ = c.ShouldBindJSON(good)
	var other OtherBody
	_ = c.ShouldBindJSON(&other)
	c.JSON(200, Response{OK: true})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load request diagnostic fixture: %v", err)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/requestdiagnostics", diagnostics)
	var code handlers.CodeFacts
	for _, route := range routes.Recognize(res) {
		code = analyzer.Analyze(route, diagnostics)
	}

	limit, ok := paramByName(code.Params, "limit")
	if !ok || primName(limit.Schema) != facts.PrimInt {
		t.Fatalf("typed binding must replace earlier untyped query evidence, got %+v", limit)
	}
	assertBodySuffix(t, code.RequestBody, "GoodBody")

	parameterSubjects := map[string]bool{}
	bodyReasons := []string{}
	for _, item := range diagnostics.Items() {
		if item.Operation != "POST /broken" {
			t.Fatalf("request completeness diagnostic lost operation identity: %+v", item)
		}
		switch item.Code {
		case "request.parameter.unresolved":
			parameterSubjects[item.Subject] = true
			if item.Subject == "limit" || strings.Contains(item.Message, "untyped query param 'limit'") {
				t.Fatalf("resolved constant-name typed query emitted a false incompleteness diagnostic: %+v", item)
			}
		case "request.body.unresolved":
			bodyReasons = append(bodyReasons, item.Message)
		}
	}
	for _, subject := range []string{"Param", "GetHeader", "Cookie", "Request.Header.Get"} {
		if !parameterSubjects[subject] {
			t.Fatalf("missing dynamic-name parameter diagnostic for %s: %+v", subject, diagnostics.Items())
		}
	}
	for _, reason := range []string{
		"multipart field name is dynamic",
		"form field name is dynamic",
		"binding target does not resolve to a named schema",
		"conflicting body evidence",
	} {
		if !sliceContainsText(bodyReasons, reason) {
			t.Fatalf("missing request.body.unresolved reason %q: %+v", reason, diagnostics.Items())
		}
	}
}

// --- helpers -------------------------------------------------------------

type facts2Diag struct {
	msg, file string
	line      uint32
}

// TestBoundParameterEnumsLandOnTheValueTheirScopeNames pins where a `oneof` on a
// bound parameter puts its members. A parameter's only carrier is its schema —
// `facts.ParamFact` has no constraint object — so an enum is stated by replacing a
// schema, and the tag's scope has to say which one:
//
//   - field scope on a scalar replaces the parameter's schema;
//   - field scope on a container lowers to what the container holds, since a
//     `facts.Type` array or map has no room for an enum beside it;
//   - `dive` lands on an array's element or a map's VALUE, leaving the key intact —
//     the defect this test exists for, where a map parameter used to be replaced
//     wholesale by a bare string enum; and
//   - a scope that reaches no schema is dropped in silence, as a post-`dive` rule
//     already is on a schema field. That covers `dive` on a scalar, which names a
//     value the parameter does not have, and `keys`…`endkeys`, which names one it
//     does: an enum map key lowers to nothing OpenAPI can express until gnr8 emits
//     `propertyNames`, and `lower::is_openapi_map_key` REJECTS a non-string key, so
//     applying it would abort generation on a well-formed tag.
//
// The last case is the invariant-3 one: two rules landing on one value are reported
// and both dropped, because choosing between them would be a precedence rule and a
// fact stated twice has no winner.
func TestBoundParameterEnumsLandOnTheValueTheirScopeNames(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/paramenums

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) ShouldBindQuery(any) error { return nil }
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package paramenums

import "github.com/gin-gonic/gin"

type Server struct{ R *gin.Engine }

type Query struct {
	State    string            `+"`"+`form:"state" binding:"oneof=active paused"`+"`"+`
	Listed   string            `+"`"+`form:"listed" enums:"private,public"`+"`"+`
	Tags     []string          `+"`"+`form:"tags" binding:"oneof=red green"`+"`"+`
	Kinds    []string          `+"`"+`form:"kinds" binding:"dive,oneof=alpha beta"`+"`"+`
	Filters  map[string]string `+"`"+`form:"filters" binding:"dive,oneof=red green"`+"`"+`
	Buckets  map[string]string `+"`"+`form:"buckets" binding:"oneof=hot cold"`+"`"+`
	Labels   map[string]string `+"`"+`form:"labels" binding:"dive,keys,oneof=x y,endkeys"`+"`"+`
	Sides    map[string]string `+"`"+`form:"sides" binding:"dive,keys,oneof=k1 k2,endkeys,oneof=v1 v2"`+"`"+`
	Plain    string            `+"`"+`form:"plain" binding:"dive,oneof=a b"`+"`"+`
	Scoped   []string          `+"`"+`form:"scoped" binding:"keys,oneof=a b,endkeys"`+"`"+`
	Twice    []string          `+"`"+`form:"twice" binding:"oneof=a b,dive,oneof=c d"`+"`"+`
	Restated string            `+"`"+`form:"restated" binding:"oneof=a b" validate:"oneof=a b"`+"`"+`
}

type Result struct { OK bool `+"`"+`json:"ok"`+"`"+` }

func (s Server) Register() { s.R.GET("/search", s.search) }

func (s Server) search(c *gin.Context) {
	var query Query
	_ = c.ShouldBindQuery(&query)
	c.JSON(200, Result{})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load param enum fixture: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/paramenums", diags)
	var code handlers.CodeFacts
	for _, route := range routes.Recognize(res) {
		code = analyzer.Analyze(route, diags)
	}

	assertEnum(t, paramSchema(t, code, "state"), "active", "paused")
	assertEnum(t, paramSchema(t, code, "listed"), "private", "public")
	assertEnum(t, arrayElement(t, paramSchema(t, code, "tags")), "red", "green")
	assertEnum(t, arrayElement(t, paramSchema(t, code, "kinds")), "alpha", "beta")

	// The defect: `dive` names the map's values, and the key must survive it.
	filters := mapOf(t, paramSchema(t, code, "filters"))
	assertEnum(t, filters.Value, "red", "green")
	if primName(filters.Key) != facts.PrimString {
		t.Fatalf("dive enum must leave the map key a string, got %+v", filters.Key)
	}
	// Field scope on a map lowers to the same place, since a map cannot be an enum.
	buckets := mapOf(t, paramSchema(t, code, "buckets"))
	assertEnum(t, buckets.Value, "hot", "cold")
	if primName(buckets.Key) != facts.PrimString {
		t.Fatalf("field-scope enum on a map must leave the key a string, got %+v", buckets.Key)
	}
	// A map key stays a plain string: an enum key is not representable in OpenAPI, and
	// lowering rejects a non-string key outright, so applying it would abort generation.
	labels := mapOf(t, paramSchema(t, code, "labels"))
	if primName(labels.Key) != facts.PrimString || primName(labels.Value) != facts.PrimString {
		t.Fatalf("a keys enum must reach nothing, got %+v", labels)
	}
	// A discarded key rule does not contend with the value rule that follows it.
	sides := mapOf(t, paramSchema(t, code, "sides"))
	if primName(sides.Key) != facts.PrimString {
		t.Fatalf("a keys enum must reach nothing, got %+v", sides.Key)
	}
	assertEnum(t, sides.Value, "v1", "v2")

	// A scope the parameter has no value for reaches nothing either.
	if primName(paramSchema(t, code, "plain")) != facts.PrimString {
		t.Fatalf("dive on a scalar has nothing to bind, got %+v", paramSchema(t, code, "plain"))
	}
	if primName(arrayElement(t, paramSchema(t, code, "scoped"))) != facts.PrimString {
		t.Fatalf("map-key scope on an array has nothing to bind, got %+v", paramSchema(t, code, "scoped"))
	}

	// Two rules on one value: both dropped, contradiction reported.
	if primName(arrayElement(t, paramSchema(t, code, "twice"))) != facts.PrimString {
		t.Fatalf("contradicted enum must not be applied, got %+v", paramSchema(t, code, "twice"))
	}
	if primName(paramSchema(t, code, "restated")) != facts.PrimString {
		t.Fatalf("enum restated under a second tag key must not be applied, got %+v", paramSchema(t, code, "restated"))
	}
	// Collected as a slice, not keyed by subject: one contradiction must raise exactly
	// one diagnostic, and a map would hide a second report for the same parameter.
	ambiguous := []facts.DiagnosticFact{}
	for _, item := range diags.Items() {
		if item.Code == "request.parameter.ambiguous" {
			ambiguous = append(ambiguous, item)
		}
	}
	if len(ambiguous) != 2 {
		t.Fatalf("want exactly one ambiguity per contradicted parameter, got %+v", diags.Items())
	}
	bySubject := map[string]facts.DiagnosticFact{}
	for _, item := range ambiguous {
		bySubject[item.Subject] = item
	}
	for _, subject := range []string{"twice", "restated"} {
		item, ok := bySubject[subject]
		if !ok || item.Severity != "ERROR" || item.Category != "request_parameter" {
			t.Fatalf("%s must raise an ERROR request_parameter ambiguity, got %+v", subject, item)
		}
		if item.Operation != "GET /search" || item.Line == 0 {
			t.Fatalf("%s ambiguity must carry operation and position, got %+v", subject, item)
		}
	}
	if !strings.Contains(bySubject["twice"].Message, `binding:"oneof=a b", binding:"oneof=c d"`) {
		t.Fatalf("ambiguity must quote both rules as written, got %q", bySubject["twice"].Message)
	}
	// Two identically spelled rules are told apart by the tag key that carries them,
	// which is the only thing distinguishing them in the source.
	if !strings.Contains(bySubject["restated"].Message, `binding:"oneof=a b", validate:"oneof=a b"`) {
		t.Fatalf("ambiguity must name the tag key of each rule, got %q", bySubject["restated"].Message)
	}
}

func paramSchema(t *testing.T, code handlers.CodeFacts, name string) facts.Type {
	t.Helper()
	param, ok := paramByName(code.Params, name)
	if !ok {
		t.Fatalf("missing parameter %q in %+v", name, code.Params)
	}
	return param.Schema
}

func arrayElement(t *testing.T, schema facts.Type) facts.Type {
	t.Helper()
	element, ok := schema.Of.(*facts.Type)
	if schema.Type != facts.TypeArray || !ok || element == nil {
		t.Fatalf("want an array schema, got %+v", schema)
	}
	return *element
}

func mapOf(t *testing.T, schema facts.Type) facts.MapType {
	t.Helper()
	mapped, ok := schema.Of.(*facts.MapType)
	if schema.Type != facts.TypeMap || !ok || mapped == nil {
		t.Fatalf("want a map schema, got %+v", schema)
	}
	return *mapped
}

func assertEnum(t *testing.T, schema facts.Type, want ...string) {
	t.Helper()
	members, ok := schema.Of.([]string)
	if schema.Type != facts.TypeEnum || !ok {
		t.Fatalf("want an enum schema, got %+v", schema)
	}
	if !equalStrings(members, want) {
		t.Fatalf("want enum members %v, got %v", want, members)
	}
}

func mustWrite(t *testing.T, path string, contents string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(contents), 0o644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func sliceContainsText(values []string, needle string) bool {
	for _, value := range values {
		if strings.Contains(value, needle) {
			return true
		}
	}
	return false
}

func assertBinaryOctetStreamResponse(t *testing.T, responses []facts.ResponseFact) {
	t.Helper()
	assertBinaryResponse(t, responses, "application/octet-stream")
}

func assertBinaryResponse(t *testing.T, responses []facts.ResponseFact, contentType string) {
	t.Helper()
	if len(responses) != 1 {
		t.Fatalf("want exactly one response, got %+v", responses)
	}
	response := responses[0]
	if response.Status != 200 || response.BodyKind != "binary" || response.Body != nil {
		t.Fatalf("want 200 binary response with no body schema, got %+v", response)
	}
	if response.ContentType != contentType {
		t.Fatalf("want %s content type, got %+v", contentType, response)
	}
	if !equalStrings(response.ContentTypes, []string{contentType}) {
		t.Fatalf("want %s content_types, got %+v", contentType, response)
	}
}

func statuses(rs []facts.ResponseFact) []uint16 {
	out := make([]uint16, 0, len(rs))
	for _, r := range rs {
		out = append(out, r.Status)
	}
	// responses are appended in source order; sort for stable comparison.
	for i := 1; i < len(out); i++ {
		for j := i; j > 0 && out[j] < out[j-1]; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return out
}

func paramByName(ps []facts.ParamFact, name string) (facts.ParamFact, bool) {
	for _, p := range ps {
		if p.Name == name {
			return p, true
		}
	}
	return facts.ParamFact{}, false
}

func primName(ty facts.Type) string {
	if ty.Type != facts.TypePrimitive {
		return ""
	}
	if p, ok := ty.Of.(*facts.Prim); ok {
		return p.Prim
	}
	return ""
}

func assertBodySuffix(t *testing.T, body *facts.TypeRef, suffix string) {
	t.Helper()
	if body == nil || !strings.HasSuffix(body.RefID, suffix) {
		t.Fatalf("want body ref suffix %q, got %+v", suffix, body)
	}
}

func assertResponseSuffix(t *testing.T, responses []facts.ResponseFact, status uint16, suffix string) {
	t.Helper()
	for _, response := range responses {
		if response.Status != status {
			continue
		}
		if response.Body == nil || !strings.HasSuffix(response.Body.RefID, suffix) {
			t.Fatalf("response %d: want body suffix %q, got %+v", status, suffix, response.Body)
		}
		return
	}
	t.Fatalf("missing response %d in %+v", status, responses)
}

func equalUint16(a, b []uint16) bool {
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

func equalStrings(a, b []string) bool {
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

func sortedCopy(in []string) []string {
	out := append([]string(nil), in...)
	for i := 1; i < len(out); i++ {
		for j := i; j > 0 && out[j] < out[j-1]; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return out
}

// TestHandlerDocCommentsBecomeOperationProse proves the four properties that keep doc
// comments a documentation convention rather than an annotation dialect (CLAUDE.md
// rule 0.1 category 2):
//
//  1. a ROUTED handler's doc comment yields summary + description;
//  2. the Go `funcName ` prefix is stripped and the remainder capitalized;
//  3. a handler with NO doc comment yields empty prose (never a guess, rule 3);
//  4. an UNROUTED helper's doc comment is never read, so internal prose cannot leak.
//
// It also pins that a `//go:`-style directive line in the doc block is dropped by
// `(*ast.CommentGroup).Text` rather than becoming prose.
func TestHandlerDocCommentsBecomeOperationProse(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/docprose

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) GET(string, HandlerFunc) {}
func (c *Context) JSON(int, any) {}
`)
	mustWrite(t, filepath.Join(dir, "handlers.go"), `package main

import "github.com/gin-gonic/gin"

type Server struct{ R *gin.Engine }
type Widget struct { ID string `+"`json:\"id\"`"+` }

func (s Server) Register() {
	s.R.GET("/widgets", s.listWidgets)
	s.R.GET("/undocumented", s.undocumented)
}

// listWidgets returns widgets for
// the caller organisation.
//
//go:noinline
//
// Results are paginated by cursor.
func (s Server) listWidgets(c *gin.Context) {
	c.JSON(200, Widget{})
}

func (s Server) undocumented(c *gin.Context) {
	c.JSON(200, Widget{})
}

// unrouted describes an internal helper that is never registered as a route.
//
// Its prose must never reach the API surface.
func (s Server) unrouted(c *gin.Context) {
	c.JSON(200, Widget{})
}
`)

	res, err := load.Load(dir, "./...")
	if err != nil {
		t.Fatalf("load doc prose fixture: %v", err)
	}
	diags := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/docprose", diags)
	recognized := routes.Recognize(res)
	got := map[string]handlers.CodeFacts{}
	for _, r := range recognized {
		got[r.Path] = analyzer.Analyze(r, diags)
	}

	// (1) + (2): prose is read, wrapped lines collapse, the symbol name is stripped and
	// the remainder capitalized. (Directive lines are dropped by CommentGroup.Text.)
	documented, ok := got["/widgets"]
	if !ok {
		t.Fatalf("expected a recognized /widgets route, got %v", keysOf(got))
	}
	if want := "Returns widgets for the caller organisation."; documented.Summary != want {
		t.Errorf("summary:\n got  %q\n want %q", documented.Summary, want)
	}
	if want := "Results are paginated by cursor."; documented.Description != want {
		t.Errorf("description:\n got  %q\n want %q", documented.Description, want)
	}

	// (3): no doc comment yields empty prose — never an inferred or borrowed value.
	bare, ok := got["/undocumented"]
	if !ok {
		t.Fatalf("expected a recognized /undocumented route, got %v", keysOf(got))
	}
	if bare.Summary != "" || bare.Description != "" {
		t.Errorf("undocumented handler must yield empty prose, got summary=%q description=%q",
			bare.Summary, bare.Description)
	}

	// (4): the unrouted helper has a doc comment but no route, so its prose is never
	// read. Assert on the whole analyzed set rather than a single route.
	for path, facts := range got {
		if strings.Contains(facts.Summary, "internal helper") ||
			strings.Contains(facts.Description, "never reach the API surface") {
			t.Errorf("unrouted helper prose leaked into route %s: %+v", path, facts)
		}
	}
}

func keysOf(m map[string]handlers.CodeFacts) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	return sortedCopy(out)
}

// A response header the handler writes under a non-constant name is dropped from the
// contract, so every write site that can carry one must say so rather than leave the
// omission silent. Gin's own `c.Header` is covered end to end by the gin-contract
// fixture; this pins the two remaining sites: a `net/http` header mutation and the
// trailing header map of `DataFromReader`.
func TestDynamicResponseHeaderNamesAreDiagnosedAtEveryWriteSite(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/dynamicheaders

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import (
	"io"
	"net/http"
)

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct {
	Request *http.Request
	Writer  http.ResponseWriter
}

func (e *Engine) GET(string, HandlerFunc)                                          {}
func (c *Context) Status(int)                                                      {}
func (c *Context) DataFromReader(int, int64, string, io.Reader, map[string]string) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package dynamicheaders

import (
	"net/http"
	"strings"

	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }

func (s Server) Register() {
	s.R.GET("/writer", s.writer)
	s.R.GET("/reader", s.reader)
	s.R.GET("/static", s.static)
}

func (s Server) writer(c *gin.Context) {
	c.Writer.Header().Set(headerName(), "v")
	c.Status(http.StatusNoContent)
}

func (s Server) reader(c *gin.Context) {
	c.DataFromReader(http.StatusOK, 5, "text/plain", strings.NewReader("hello"), extraHeaders())
}

func (s Server) static(c *gin.Context) {
	c.Writer.Header().Set("X-Static", "v")
	c.DataFromReader(http.StatusOK, 5, "text/plain", strings.NewReader("hello"), map[string]string{"X-Extra": "v"})
}

func headerName() string { return "X-" + strings.ToUpper("tenant") }

func extraHeaders() map[string]string { return map[string]string{"X-Computed": "v"} }
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load dynamic header fixture: %v", err)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/dynamicheaders", diagnostics)
	analyzed := map[string]handlers.CodeFacts{}
	for _, route := range routes.Recognize(res) {
		analyzed[route.Handler] = analyzer.Analyze(route, diagnostics)
	}

	reported := map[string]int{}
	for _, item := range diagnostics.Items() {
		if item.Code == "response.header.unresolved" {
			reported[item.Operation]++
		}
	}
	for _, operation := range []string{"GET /writer", "GET /reader"} {
		if reported[operation] != 1 {
			t.Fatalf("%s must report exactly one unresolved response header, got %d: %+v",
				operation, reported[operation], diagnostics.Items())
		}
	}
	if reported["GET /static"] != 0 {
		t.Fatalf("constant header names must not be diagnosed: %+v", diagnostics.Items())
	}

	// Dropped, never guessed. `DataFromReader` still states the Content-Type and
	// Content-Length it always writes; what must not appear is a name read out of the
	// unresolvable map.
	intrinsic := map[string]bool{"Content-Type": true, "Content-Length": true}
	for _, handler := range []string{"writer", "reader"} {
		for _, response := range analyzed[handler].Responses {
			for _, header := range response.Headers {
				if !intrinsic[header.Name] {
					t.Fatalf("%s must not declare a guessed response header: %+v", handler, response.Headers)
				}
			}
		}
	}
	// The static twin still resolves both write sites, so the diagnostic is not a
	// blanket opt-out of header extraction.
	staticHeaders := map[string]bool{}
	for _, response := range analyzed["static"].Responses {
		for _, header := range response.Headers {
			staticHeaders[header.Name] = true
		}
	}
	if !staticHeaders["X-Static"] || !staticHeaders["X-Extra"] {
		t.Fatalf("constant header names must still be extracted: %+v", staticHeaders)
	}
}

// A response header is a fact about what the handler SENDS. `c.Request.Header` and a
// scratch `http.Header` have the same type as `c.Writer.Header()`, so a type-only
// predicate states inbound and local headers as part of the outbound contract. Pin the
// provenance rule in both directions, and pin that a constant map key is as statically
// known as the string it was declared from.
func TestResponseHeadersComeOnlyFromTheResponseWriter(t *testing.T) {
	dir := t.TempDir()
	mustWrite(t, filepath.Join(dir, "go.mod"), `module example.com/headerprovenance

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
`)
	if err := os.Mkdir(filepath.Join(dir, "ginstub"), 0o755); err != nil {
		t.Fatalf("mkdir ginstub: %v", err)
	}
	mustWrite(t, filepath.Join(dir, "ginstub", "go.mod"), "module github.com/gin-gonic/gin\n\ngo 1.22\n")
	mustWrite(t, filepath.Join(dir, "ginstub", "gin.go"), `package gin

import (
	"io"
	"net/http"
)

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct {
	Request *http.Request
	Writer  http.ResponseWriter
}

func (e *Engine) GET(string, HandlerFunc)                                          {}
func (c *Context) Status(int)                                                      {}
func (c *Context) DataFromReader(int, int64, string, io.Reader, map[string]string) {}
`)
	mustWrite(t, filepath.Join(dir, "app.go"), `package headerprovenance

import (
	"net/http"
	"strings"

	"github.com/gin-gonic/gin"
)

const sessionHeader = "X-Session-ID"

type Server struct{ R *gin.Engine }

func (s Server) Register() {
	s.R.GET("/request-mutation", s.requestMutation)
	s.R.GET("/local-map", s.localMap)
	s.R.GET("/writer", s.writer)
	s.R.GET("/writer-var", s.writerVar)
	s.R.GET("/writer-helper", s.writerHelper)
	s.R.GET("/const-key", s.constKey)
}

func (s Server) requestMutation(c *gin.Context) {
	c.Request.Header.Set("X-Request-Set", "v")
	c.Request.Header.Add("X-Request-Add", "v")
	c.Status(http.StatusNoContent)
}

func (s Server) localMap(c *gin.Context) {
	local := http.Header{}
	local.Set("X-Local", "v")
	c.Status(http.StatusNoContent)
}

func (s Server) writer(c *gin.Context) {
	c.Writer.Header().Set("X-Writer", "v")
	c.Status(http.StatusNoContent)
}

func (s Server) writerVar(c *gin.Context) {
	out := c.Writer.Header()
	out.Set("X-Writer-Var", "v")
	c.Status(http.StatusNoContent)
}

func (s Server) writerHelper(c *gin.Context) {
	tag(c.Writer)
	c.Status(http.StatusNoContent)
}

func tag(w http.ResponseWriter) { w.Header().Set("X-Writer-Helper", "v") }

func (s Server) constKey(c *gin.Context) {
	c.DataFromReader(http.StatusOK, 5, "text/plain", strings.NewReader("hello"), map[string]string{
		sessionHeader: "v",
	})
}
`)

	res, err := load.Load(dir)
	if err != nil {
		t.Fatalf("load header provenance fixture: %v", err)
	}
	diagnostics := diag.New()
	analyzer := handlers.NewAnalyzer(res, "example.com/headerprovenance", diagnostics)
	analyzed := map[string]handlers.CodeFacts{}
	for _, route := range routes.Recognize(res) {
		analyzed[route.Handler] = analyzer.Analyze(route, diagnostics)
	}

	headerNames := func(handler string) map[string]bool {
		out := map[string]bool{}
		for _, response := range analyzed[handler].Responses {
			for _, header := range response.Headers {
				out[header.Name] = true
			}
		}
		return out
	}

	// A header the handler only reads or keeps locally is not part of what it sends.
	for _, handler := range []string{"requestMutation", "localMap"} {
		if names := headerNames(handler); len(names) != 0 {
			t.Fatalf("%s writes no response header, got %+v", handler, names)
		}
	}
	// Every shape that provably reaches the response writer stays a response fact.
	for handler, want := range map[string]string{
		"writer":       "X-Writer",
		"writerVar":    "X-Writer-Var",
		"writerHelper": "X-Writer-Helper",
	} {
		if names := headerNames(handler); !names[want] {
			t.Fatalf("%s must declare %s, got %+v", handler, want, names)
		}
	}
	// A named constant key is statically known, so it is extracted, not diagnosed.
	if names := headerNames("constKey"); !names[sessionHeaderConstant] {
		t.Fatalf("constant map keys must resolve, got %+v", names)
	}
	for _, item := range diagnostics.Items() {
		if item.Code == "response.header.unresolved" {
			t.Fatalf("a constant header name must not be diagnosed: %+v", item)
		}
	}
}

// The header name the provenance fixture declares as a Go constant.
const sessionHeaderConstant = "X-Session-ID"
