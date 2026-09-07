package sdk

import (
	"context"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"strings"
	"testing"
)

type capturedRequest struct {
	path        string
	rawQuery    string
	contentType string
	body        []byte
}

type captureTransport struct {
	requests []capturedRequest
}

func (transport *captureTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	var body []byte
	if request.Body != nil {
		var err error
		body, err = io.ReadAll(request.Body)
		if err != nil {
			return nil, err
		}
	}
	transport.requests = append(transport.requests, capturedRequest{
		path:        request.URL.Path,
		rawQuery:    request.URL.RawQuery,
		contentType: request.Header.Get("Content-Type"),
		body:        body,
	})
	header := make(http.Header)
	status := http.StatusNoContent
	responseBody := ""
	if strings.HasSuffix(request.URL.Path, "/search") {
		status = http.StatusOK
		header.Set("Content-Type", "application/json")
		responseBody = `{"q":"","limit":0,"offset":0,"page":1,"days":0,"sort":"","cursor":"","token":""}`
	}
	if strings.HasSuffix(request.URL.Path, "/redirect") {
		status = http.StatusTemporaryRedirect
		header.Set("Location", "/v1/files/final")
		header.Set("X-Session-ID", "session-123")
	}
	return &http.Response{
		StatusCode: status,
		Header:     header,
		Body:       io.NopCloser(strings.NewReader(responseBody)),
		Request:    request,
	}, nil
}

func TestRequestBodyVariantsAndRedirectPolicy(t *testing.T) {
	transport := &captureTransport{}
	var responseStatuses []int
	var redirectSession string
	client := NewClient(
		"https://api.test",
		WithHTTPClient(&http.Client{Transport: transport}),
		WithResponseHook(func(_ context.Context, context RequestContext, response *http.Response) error {
			responseStatuses = append(responseStatuses, context.StatusCode)
			if context.OperationID == "redirectFile" {
				redirectSession = response.Header.Get("X-Session-ID")
			}
			return nil
		}),
	)

	if _, err := client.UploadFile(context.Background(), UploadFileJSONBody{
		Value: CreateUploadRequest{Title: Ptr(Ptr("json"))},
	}); err != nil {
		t.Fatalf("JSON upload: %v", err)
	}
	if got := transport.requests[len(transport.requests)-1]; got.contentType != "application/json" || string(got.body) != `{"title":"json"}` {
		t.Fatalf("JSON wire body: %+v", got)
	}

	requestJSON := `{"name":"multipart"}`
	for _, files := range [][]MultipartFile{
		nil,
		{NewMultipartFile("one.txt", []byte("one"))},
		{
			NewMultipartFile("one.txt", []byte("one")),
			NewMultipartFile("two.txt", []byte("two")),
		},
	} {
		body := UploadFileFormRequest{Request: requestJSON, Files: files}
		if _, err := client.UploadFile(context.Background(), UploadFileMultipartBody{Value: body}); err != nil {
			t.Fatalf("multipart upload with %d files: %v", len(files), err)
		}
		got := transport.requests[len(transport.requests)-1]
		parts := multipartParts(t, got)
		if len(parts["files"]) != len(files) {
			t.Fatalf("multipart files: got %d, want %d (%+v)", len(parts["files"]), len(files), parts)
		}
		if len(files) == 0 {
			if _, emitted := parts["files"]; emitted {
				t.Fatalf("nil multipart file field must be omitted: %+v", parts)
			}
		}
		if got := string(parts["request"][0]); got != requestJSON {
			t.Fatalf("multipart JSON string part = %q, want %q", got, requestJSON)
		}
	}

	for _, test := range []struct {
		name     string
		filename string
		call     func() error
	}{
		{
			name:     "Gin Context.FormFile",
			filename: "context-asset.txt",
			call: func() error {
				_, err := client.ContextFormFile(context.Background(), ContextFormFileFormRequest{
					Asset: NewMultipartFile("context-asset.txt", []byte("context")),
				})
				return err
			},
		},
		{
			name:     "net/http Request.FormFile",
			filename: "request-asset.txt",
			call: func() error {
				_, err := client.RequestFormFile(context.Background(), RequestFormFileFormRequest{
					Asset: NewMultipartFile("request-asset.txt", []byte("request")),
				})
				return err
			},
		},
	} {
		if err := test.call(); err != nil {
			t.Fatalf("%s upload: %v", test.name, err)
		}
		got := transport.requests[len(transport.requests)-1]
		if filename := multipartFilename(t, got, "asset"); filename != test.filename {
			t.Fatalf("%s filename = %q, want %q", test.name, filename, test.filename)
		}
	}

	if _, err := client.SearchItems(context.Background(), SearchItemsParams{Page: 1, Q: ""}); err != nil {
		t.Fatalf("search without optional offset: %v", err)
	}
	withoutOffset := transport.requests[len(transport.requests)-1]
	if strings.Contains(withoutOffset.rawQuery, "offset=") || strings.Contains(withoutOffset.rawQuery, "sort=") || strings.Contains(withoutOffset.rawQuery, "cursor=") {
		t.Fatalf("absent optional query values were emitted: %q", withoutOffset.rawQuery)
	}
	zero := int64(0)
	if _, err := client.SearchItems(context.Background(), SearchItemsParams{Page: 1, Q: "", Offset: &zero}); err != nil {
		t.Fatalf("search with explicit zero offset: %v", err)
	}
	if got := transport.requests[len(transport.requests)-1].rawQuery; !strings.Contains(got, "offset=0") {
		t.Fatalf("explicit zero offset was omitted: %q", got)
	}

	responseStatuses = nil
	before := len(transport.requests)
	if _, err := client.RedirectFile(context.Background(), "file-1"); err != nil {
		t.Fatalf("default redirect call: %v", err)
	}
	if got := len(transport.requests) - before; got != 1 {
		t.Fatalf("default call followed redirect: %d transport requests", got)
	}
	if len(responseStatuses) != 1 || responseStatuses[0] != http.StatusTemporaryRedirect || redirectSession != "session-123" {
		t.Fatalf("redirect response hook: statuses=%v session=%q", responseStatuses, redirectSession)
	}

	responseStatuses = nil
	before = len(transport.requests)
	if _, err := client.RedirectFile(context.Background(), "file-1", WithFollowRedirects(true)); err != nil {
		t.Fatalf("opt-in redirect call: %v", err)
	}
	if got := len(transport.requests) - before; got != 2 {
		t.Fatalf("opt-in call made %d transport requests, want redirect plus destination", got)
	}
	if len(responseStatuses) != 1 || responseStatuses[0] != http.StatusNoContent {
		t.Fatalf("followed redirect response hook statuses = %v", responseStatuses)
	}
}

func multipartParts(t *testing.T, request capturedRequest) map[string][][]byte {
	t.Helper()
	mediaType, params, err := mime.ParseMediaType(request.contentType)
	if err != nil || mediaType != "multipart/form-data" {
		t.Fatalf("multipart Content-Type %q: %v", request.contentType, err)
	}
	reader := multipart.NewReader(strings.NewReader(string(request.body)), params["boundary"])
	parts := map[string][][]byte{}
	for {
		part, err := reader.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatalf("read multipart body: %v", err)
		}
		value, err := io.ReadAll(part)
		if err != nil {
			t.Fatalf("read multipart part: %v", err)
		}
		parts[part.FormName()] = append(parts[part.FormName()], value)
	}
	return parts
}

func multipartFilename(t *testing.T, request capturedRequest, field string) string {
	t.Helper()
	mediaType, params, err := mime.ParseMediaType(request.contentType)
	if err != nil || mediaType != "multipart/form-data" {
		t.Fatalf("multipart Content-Type %q: %v", request.contentType, err)
	}
	reader := multipart.NewReader(strings.NewReader(string(request.body)), params["boundary"])
	for {
		part, err := reader.NextPart()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatalf("read multipart body: %v", err)
		}
		if part.FormName() == field {
			return part.FileName()
		}
	}
	t.Fatalf("missing multipart field %q", field)
	return ""
}
