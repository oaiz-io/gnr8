package gincontract

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/google/uuid"
)

const sessionIDHeader = "X-Session-ID"

type Handler struct{}

type NativeStatus string

const (
	NativeStatusReady NativeStatus = "ready"
	NativeStatusDone  NativeStatus = "done"
)

type NativeQuery struct {
	IDs       []uuid.UUID    `form:"ids,parser=encoding.TextUnmarshaler" collection_format:"csv"`
	Statuses  []NativeStatus `form:"statuses" collection_format:"csv"`
	Limit     uint           `form:"limit,default=20" binding:"min=1,max=100"`
	RequestID string         `form:"request_id" binding:"omitempty,max=128"`
	Enabled   *bool          `form:"enabled"`
	Count     int32          `form:"count" binding:"gte=-5,lte=5"`
	Ratio     float64        `form:"ratio" binding:"min=0.25,max=0.75"`
	Scores    []int          `form:"scores" binding:"min=1,max=5,dive,gte=0,lte=10"`
	At        time.Time      `form:"at"`
}

type NativeURI struct {
	ItemID uuid.UUID `uri:"itemId,parser=encoding.TextUnmarshaler" binding:"required"`
}

type NativeHeaders struct {
	RequestID string `header:"X-Request-ID" binding:"required,max=64"`
}

type StrictDecoderRequest struct {
	Name string `json:"name" binding:"required"`
}

type RawCommand interface {
	Execute([]byte) error
}

var rawCommand RawCommand

type LoginRequest struct {
	Email string `json:"email" binding:"required"`
}

type LoginResponse struct {
	Token string `json:"token"`
}

type UpdateItemRequest struct {
	Name string `json:"name"`
}

type CreateUploadRequest struct {
	Title string `json:"title"`
}

type ItemResponse struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type ChildResponse struct {
	ItemID  string `json:"itemId"`
	ChildID string `json:"childId"`
}

type SavedViewResponse struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type MessageResponse struct {
	Message string `json:"message"`
}

type SearchResponse struct {
	Q      string `json:"q"`
	Limit  int    `json:"limit"`
	Offset uint   `json:"offset"`
	Page   uint   `json:"page"`
	Days   int    `json:"days"`
	Sort   string `json:"sort"`
	Cursor string `json:"cursor"`
	Token  string `json:"token"`
}

type MarkReadRequest struct {
	LastID string `json:"lastId"`
}

type DirectionalResponse struct {
	UserUUID *string           `json:"userUuid"`
	Items    []ItemResponse    `json:"items"`
	Metadata map[string]string `json:"metadata"`
	Nickname *string           `json:"nickname,omitempty"`
	Tags     []string          `json:"tags,omitempty"`
	Result   map[string]string `json:"result,omitempty"`
	Zero     map[string]string `json:"zero,omitzero"`
}

type ValidatedRequest struct {
	IDs []string        `json:"ids" binding:"required"`
	Raw json.RawMessage `json:"raw" binding:"required"`
}

type SharedPayload struct {
	Data []string `json:"data,omitempty"`
}

type CollectionRules struct {
	Names  []string          `json:"names" binding:"required,min=1,dive"`
	Codes  []string          `json:"codes" validate:"required,min=1"`
	Slots  [3]int            `json:"slots" binding:"required,max=100,dive"`
	Sizes  []int             `json:"sizes" validate:"gte=2,lte=6"`
	Labels map[string]string `json:"labels" binding:"min=1,max=4"`
	Label  string            `json:"label" validate:"min=2,max=24"`
	Rank   int               `json:"rank" binding:"min=1,max=9"`
}

type CollectionPayload struct {
	Rules CollectionRules `json:"rules" binding:"required"`
}

type FileBytes []byte

const (
	defaultSort   = "asc"
	defaultCursor = "first"
	defaultDays   = 5
)

func RegisterRoutes(r *gin.Engine, h *Handler) {
	v1 := r.Group("/v1")

	auth := v1.Group("/auth")
	auth.POST("/login", h.login)
	auth.POST("/logout", h.logout)

	files := v1.Group("/files")
	files.GET("/:fileId/download", h.downloadFile)
	files.GET("/:fileId/open", h.openFile)
	files.GET("/:fileId/stream", h.streamFile)
	files.GET("/:fileId/read", h.readFile)
	files.GET("/:fileId/dynamic-header", h.dynamicHeaderFile)
	files.GET("/:fileId/redirect", h.redirectFile)
	files.GET("/:fileId/helper-redirect", h.helperRedirectFile)
	files.POST("/upload", h.uploadFile)
	files.PATCH("/upload/:fileId", h.updateUploadFile)
	files.POST("/dynamic-upload", h.dynamicUpload)
	files.POST("/form-file/context", h.contextFormFile)
	files.POST("/form-file/request", h.requestFormFile)
	files.POST("/form-file/request-parts/:collectionId", h.requestFormFiles)
	files.POST("/form-file/request-dynamic", h.dynamicRequestFormFile)

	items := v1.Group("/items")
	items.GET("/:itemId/children/:childId", h.getChild)
	items.PATCH("/:itemId", h.updateItem)
	items.PATCH("/:itemId/read", h.markRead)
	items.PATCH("/:itemId/header-read", h.headerRead)
	items.PATCH("/:itemId/combined-header-read", h.combinedHeaderRead)
	items.PATCH("/:itemId/force-read", h.forceRead)
	items.PATCH("/:itemId/mixed-read", h.mixedRead)
	items.PATCH("/:itemId/unrelated-length-read", h.unrelatedLengthRead)
	items.GET("/saved-views", h.listSavedViews)
	items.GET("/search", h.searchItems)
	items.GET("/query-required", h.queryRequired)
	items.GET("/query-optional", h.queryOptional)
	items.GET("/request-observations", h.requestObservations)
	items.GET("/:itemId/native-bindings", h.nativeBindings)
	items.GET("/attendance", h.attendance)
	items.GET("/events", h.itemEvents)
	items.GET("/raw-stream", h.rawStream)
	items.POST("/jobs", h.createJob)
	items.GET("/directional", h.directional)
	items.POST("/validated", h.validated)
	items.POST("/shared", h.shared)
	items.POST("/collection-cardinality", h.collectionCardinality)
	items.GET("/cookie-accepted", h.cookieAccepted)
	items.GET("/cookie-default", h.cookieDefault)
	items.GET("/cookie-rejected", h.cookieRejected)
	items.GET("/cookie-unresolved", h.cookieUnresolved)
	items.POST("/queueable", h.queueable)
	items.POST("/strict-decoder", h.strictDecoder)
	items.POST("/raw-json", h.rawJSON)
	items.POST("/raw-command", h.rawCommand)
	items.POST("/raw-ambiguous", h.rawAmbiguous)
	items.POST("/unrelated-decoder", h.unrelatedDecoder)
	items.DELETE("/:itemId", h.deleteItem)
}

func (h *Handler) nativeBindings(c *gin.Context) {
	var query NativeQuery
	var uri NativeURI
	var headers NativeHeaders
	_ = c.ShouldBindQuery(&query)
	_ = c.ShouldBindUri(&uri)
	_ = c.ShouldBindHeader(&headers)
	c.Status(http.StatusNoContent)
}

func (h *Handler) strictDecoder(c *gin.Context) {
	var request StrictDecoderRequest
	if err := decodeStrictJSON(c, &request); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}

func decodeStrictJSON(c *gin.Context, target any) error {
	decoder := json.NewDecoder(c.Request.Body)
	decoder.DisallowUnknownFields()
	return decoder.Decode(target)
}

func (h *Handler) rawJSON(c *gin.Context) {
	raw, _ := c.GetRawData()
	var request StrictDecoderRequest
	if err := json.Unmarshal(raw, &request); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}

func (h *Handler) rawCommand(c *gin.Context) {
	raw, _ := io.ReadAll(c.Request.Body)
	switch c.ContentType() {
	case "application/octet-stream":
		_ = rawCommand.Execute(raw)
	}
	c.Status(http.StatusNoContent)
}

func (h *Handler) rawAmbiguous(c *gin.Context) {
	raw, _ := c.GetRawData()
	_ = raw
	c.Status(http.StatusNoContent)
}

func (h *Handler) unrelatedDecoder(c *gin.Context) {
	var request StrictDecoderRequest
	_ = decodeReader(strings.NewReader("{}"), &request)
	c.Status(http.StatusNoContent)
}

func decodeReader(reader io.Reader, target any) error {
	return json.NewDecoder(reader).Decode(target)
}

// queueable answers a typed body on one success and plain text on another, the shape a
// single-return-type SDK method cannot carry whole.
func (h *Handler) queueable(c *gin.Context) {
	var body LoginRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	if body.Email == "" {
		c.String(http.StatusAccepted, "queued")
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: "done"})
}

func (h *Handler) login(c *gin.Context) {
	var body LoginRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusOK)
	c.JSON(http.StatusOK, LoginResponse{Token: "token"})
}

func (h *Handler) logout(c *gin.Context) {
	c.Status(http.StatusNoContent)
}

func (h *Handler) getChild(c *gin.Context) {
	itemID, ok := parsePathUUID(c, "itemId")
	if !ok {
		return
	}
	childID := c.Param("childId")
	c.JSON(http.StatusOK, ChildResponse{ItemID: itemID, ChildID: childID})
}

func (h *Handler) updateItem(c *gin.Context) {
	var body UpdateItemRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, ItemResponse{ID: c.Param("itemId"), Name: body.Name})
}

func (h *Handler) deleteItem(c *gin.Context) {
	c.AbortWithStatus(http.StatusNoContent)
}

func (h *Handler) listSavedViews(c *gin.Context) {
	views := []SavedViewResponse{
		{ID: "1", Name: "Default"},
	}
	c.JSON(http.StatusOK, views)
}

func (h *Handler) downloadFile(c *gin.Context) {
	c.Header("Content-Type", "application/octet-stream")
	c.FileAttachment("/tmp/report.pdf", "report.pdf")
}

func (h *Handler) openFile(c *gin.Context) {
	c.File("/tmp/report.pdf")
}

func (h *Handler) streamFile(c *gin.Context) {
	payload := FileBytes("...")
	c.Data(http.StatusOK, "application/pdf", payload)
}

func (h *Handler) readFile(c *gin.Context) {
	if c.GetHeader("X-Force-Missing") != "" {
		c.JSON(http.StatusNotFound, MessageResponse{Message: "not found"})
		return
	}
	c.DataFromReader(http.StatusOK, 12, attachmentContentType(), strings.NewReader("hello"), map[string]string{
		"Content-Disposition": `attachment; filename="report.pdf"`,
		sessionIDHeader:       "session-123",
	})
}

func (h *Handler) dynamicHeaderFile(c *gin.Context) {
	c.Header(sessionHeaderName(), "session-123")
	c.Status(http.StatusNoContent)
}

func sessionHeaderName() string {
	return fmt.Sprintf("X-Session-%d", time.Now().Year())
}

func (h *Handler) redirectFile(c *gin.Context) {
	c.Request.Header.Set("X-Forwarded-Trace", "trace-1")
	c.Header(sessionIDHeader, "session-123")
	c.Redirect(http.StatusTemporaryRedirect, "/v1/files/final")
}

func (h *Handler) helperRedirectFile(c *gin.Context) {
	redirectResponse(c, "/v1/files/final", http.StatusFound)
}

func redirectResponse(c *gin.Context, location string, status int) {
	http.Redirect(c.Writer, c.Request, location, status)
}

func (h *Handler) uploadFile(c *gin.Context) {
	_, err := parseUploadRequest[CreateUploadRequest](c)
	if err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}

func (h *Handler) updateUploadFile(c *gin.Context) {
	_ = c.Param("fileId")
	_, err := parseUploadRequest[UpdateItemRequest](c)
	if err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}

func parseUploadRequest[T any](c *gin.Context) (T, error) {
	var body T
	if c.GetHeader("Content-Type") == "application/json" {
		_ = c.ShouldBindJSON(&body)
	} else {
		raw := c.PostForm("request")
		if raw == "" {
			return body, fmt.Errorf("request is required")
		}
		_ = json.Unmarshal([]byte(raw), &body)
		parseUploadFiles(c)
	}
	return body, nil
}

func parseUploadFiles(c *gin.Context) {
	form, _ := c.MultipartForm()
	files := form.File["files"]
	_ = files
}

func (h *Handler) dynamicUpload(c *gin.Context) {
	form, _ := c.MultipartForm()
	field := c.Query("field")
	_ = form.File[field]
	c.Status(http.StatusNoContent)
}

func (h *Handler) contextFormFile(c *gin.Context) {
	_, _ = c.FormFile("asset")
	c.Status(http.StatusNoContent)
}

func (h *Handler) requestFormFile(c *gin.Context) {
	_, _, _ = c.Request.FormFile("asset")
	c.Status(http.StatusNoContent)
}

func (h *Handler) requestFormFiles(c *gin.Context) {
	_ = c.Param("collectionId")
	_ = c.GetHeader("X-Upload-Trace")
	_ = c.PostForm("caption")
	_, _, _ = c.Request.FormFile("primaryImage")
	_, _, _ = c.Request.FormFile("supportingDocument")
	var unrelated http.Request
	_, _, _ = unrelated.FormFile("ignored")
	c.Status(http.StatusNoContent)
}

func (h *Handler) dynamicRequestFormFile(c *gin.Context) {
	field := requestFileField()
	_, _, _ = c.Request.FormFile(field)
	c.Status(http.StatusNoContent)
}

func requestFileField() string {
	if time.Now().Unix()%2 == 0 {
		return "front"
	}
	return "back"
}

func (h *Handler) itemEvents(c *gin.Context) {
	c.Stream(func(w io.Writer) bool {
		c.SSEvent("message", MessageResponse{Message: "ok"})
		return false
	})
}

func (h *Handler) rawStream(c *gin.Context) {
	c.Stream(func(w io.Writer) bool {
		return false
	})
}

func (h *Handler) createJob(c *gin.Context) {
	c.JSON(http.StatusAccepted, gin.H{
		"jobId": "job_123",
	})
}

func (h *Handler) directional(c *gin.Context) {
	c.JSON(http.StatusOK, DirectionalResponse{})
}

func (h *Handler) validated(c *gin.Context) {
	var body ValidatedRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: "ok"})
}

func (h *Handler) shared(c *gin.Context) {
	var body SharedPayload
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, body)
}

func (h *Handler) collectionCardinality(c *gin.Context) {
	var body CollectionPayload
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, body)
}

func sharedCookie(c *gin.Context) (string, error) {
	return c.Cookie("shared-cookie")
}

func (h *Handler) cookieAccepted(c *gin.Context) {
	value, err := sharedCookie(c)
	if err != nil {
		value = "anonymous"
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) cookieDefault(c *gin.Context) {
	value, err := sharedCookie(c)
	if err != nil {
		c.JSON(http.StatusOK, MessageResponse{})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) cookieRejected(c *gin.Context) {
	value, err := sharedCookie(c)
	if err != nil {
		c.JSON(http.StatusUnauthorized, MessageResponse{Message: "missing cookie"})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) cookieUnresolved(c *gin.Context) {
	value, err := sharedCookie(c)
	if err != nil && time.Now().Unix()%2 == 0 {
		c.JSON(http.StatusUnauthorized, MessageResponse{Message: "missing cookie"})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) searchItems(c *gin.Context) {
	q, _ := c.GetQuery("q")
	q = strings.TrimSpace(q)
	limit := parseOptionalPositiveInt(c.Query("limit"))
	trimmedLimit := parseOptionalPositiveInt(strings.TrimSpace(c.Query("trimmedLimit")))
	wrappedLimit := fmt.Sprint(parseOptionalPositiveInt(c.Query("wrappedLimit")))
	sort := parseSort(strings.TrimSpace(c.Query("sort")), defaultSort)
	cursor := parseSort(c.DefaultQuery("cursor", defaultCursor), "fallback")
	token, _ := c.GetQuery("token")
	offset, _ := parseOptionalUint(c, "offset")
	page, err := parseRequiredUint(c, "page")
	if err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	_ = wrappedLimit
	_ = trimmedLimit
	c.JSON(http.StatusOK, SearchResponse{Q: q, Limit: limit, Offset: offset, Page: page, Sort: sort, Cursor: cursor, Token: token})
}

func (h *Handler) queryRequired(c *gin.Context) {
	value := c.Query("term")
	alias := value
	if alias == "" {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: "term is required"})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) queryOptional(c *gin.Context) {
	value := c.Query("view")
	if value != "" {
		value = strings.TrimSpace(value)
	}
	c.JSON(http.StatusOK, MessageResponse{Message: value})
}

func (h *Handler) requestObservations(c *gin.Context) {
	_ = c.GetHeader("X-Observed")
	_ = helperObservedHeader(c)
	if c.GetHeader("X-Required") == "" {
		c.AbortWithStatus(http.StatusBadRequest)
		return
	}
	_, _ = c.Cookie("observed-cookie")
	_, err := c.Cookie("required-cookie")
	if err != nil {
		c.AbortWithStatus(http.StatusUnauthorized)
		return
	}
	_ = c.GetHeader("Authorization")
	c.Status(http.StatusNoContent)
}

func helperObservedHeader(c *gin.Context) string {
	return c.Request.Header.Get("X-Helper-Observed")
}

func (h *Handler) attendance(c *gin.Context) {
	startDate, err := parseAttendanceTime(c.Query("startDate"), "bad-date", "invalid")
	if err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	days, err := parseAttendanceDays(c.Query("days"), defaultDays, 31)
	if err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	_ = startDate
	c.JSON(http.StatusOK, SearchResponse{Days: days})
}

func (h *Handler) markRead(c *gin.Context) {
	var body MarkReadRequest
	if len(strings.TrimSpace(c.GetHeader("Content-Length"))) != 0 || c.Request.ContentLength > 0 {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func (h *Handler) headerRead(c *gin.Context) {
	var body MarkReadRequest
	if c.GetHeader("Content-Length") != "" {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func (h *Handler) combinedHeaderRead(c *gin.Context) {
	var body MarkReadRequest
	if len(c.GetHeader("Content-Length")+c.GetHeader("X-Force-Bind")) != 0 {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func (h *Handler) forceRead(c *gin.Context) {
	var body MarkReadRequest
	if c.Request.ContentLength > 0 || c.GetHeader("X-Force-Bind") != "" {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func (h *Handler) mixedRead(c *gin.Context) {
	var body MarkReadRequest
	if c.Request.ContentLength > 0 {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func (h *Handler) unrelatedLengthRead(c *gin.Context) {
	var body MarkReadRequest
	var request http.Request
	if request.ContentLength > 0 {
		if err := c.ShouldBindJSON(&body); err != nil {
			c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
			return
		}
	}
	c.JSON(http.StatusOK, MessageResponse{Message: body.LastID})
}

func parsePathUUID(c *gin.Context, name string) (string, bool) {
	value := c.Param(name)
	return value, value != ""
}

func parseOptionalPositiveInt(raw string) int {
	if raw == "" {
		return 0
	}
	return 1
}

func parseAttendanceTime(raw string, _, _ string) (time.Time, error) {
	if raw == "" {
		return time.Time{}, nil
	}
	return time.Now(), nil
}

func parseAttendanceDays(raw string, fallback, _ int) (int, error) {
	if raw == "" {
		return fallback, nil
	}
	return fallback + 1, nil
}

func parseOptionalUint(c *gin.Context, name string) (uint, error) {
	raw := c.Query(name)
	if raw == "" {
		return 0, nil
	}
	return 1, nil
}

func parseRequiredUint(c *gin.Context, name string) (uint, error) {
	raw := c.Query(name)
	if raw == "" {
		return 0, fmt.Errorf("missing %s", name)
	}
	return 1, nil
}

func parseSort(raw string, fallback string) string {
	if raw == "" {
		return fallback
	}
	return raw
}

func attachmentContentType() string {
	return "application/pdf"
}
