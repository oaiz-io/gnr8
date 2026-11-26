// Package notes is the Rust SDK fixture: a small Gin service whose routes reach every kind of
// request and reply the Rust SDK target sends and reads. The `.gnr8/` pipeline adds what the
// source cannot state: its security schemes, its pagination and its idempotent operation.
package notes

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// Handler serves the notes API.
type Handler struct{}

// Visibility says who may read a note.
type Visibility string

const (
	VisibilityPrivate Visibility = "private"
	VisibilityShared  Visibility = "shared"
)

// Note is one stored note.
type Note struct {
	ID         string            `json:"id"`
	Title      string            `json:"title"`
	Body       string            `json:"body,omitempty"`
	Visibility Visibility        `json:"visibility"`
	Tags       []string          `json:"tags"`
	Labels     map[string]string `json:"labels,omitempty"`
	Score      *float64          `json:"score"`
}

// NewNote is the body that creates a note.
type NewNote struct {
	Title      string     `json:"title" binding:"required"`
	Body       string     `json:"body"`
	Visibility Visibility `json:"visibility" binding:"required"`
	Tags       []string   `json:"tags"`
}

// NotePage is one page of notes.
type NotePage struct {
	Items      []Note `json:"items"`
	NextCursor string `json:"next_cursor,omitempty"`
}

// Problem is the body of a rejected request.
type Problem struct {
	Message string   `json:"message"`
	Slug    string   `json:"slug"`
	Hints   []string `json:"hints,omitempty"`
}

// ListQuery filters the notes a page lists.
type ListQuery struct {
	Cursor string   `form:"cursor"`
	Limit  int32    `form:"limit" binding:"min=1,max=100"`
	Tags   []string `form:"tag"`
	Order  string   `form:"order" binding:"omitempty,oneof=newest oldest"`
}

// ListHeaders carry the caller's trace.
type ListHeaders struct {
	Trace string `header:"X-Trace"`
}

// NoteURI names one note.
type NoteURI struct {
	ID string `uri:"noteId" binding:"required"`
}

// RegisterRoutes mounts the notes API.
func RegisterRoutes(r *gin.Engine, h *Handler) {
	v1 := r.Group("/v1")
	notes := v1.Group("/notes")
	notes.GET("", h.listNotes)
	notes.POST("", h.createNote)
	notes.GET("/:noteId", h.getNote)
	notes.DELETE("/:noteId", h.deleteNote)
	notes.GET("/:noteId/export", h.exportNote)
	notes.GET("/:noteId/attachment", h.attachment)
	notes.POST("/:noteId/files", h.uploadFile)
	notes.GET("/:noteId/events", h.noteEvents)
	v1.GET("/session", h.session)
}

// listNotes lists the notes, newest first.
//
// Pass the next cursor of a page to read the page after it.
func (h *Handler) listNotes(c *gin.Context) {
	var query ListQuery
	var headers ListHeaders
	_ = c.ShouldBindQuery(&query)
	_ = c.ShouldBindHeader(&headers)
	c.JSON(http.StatusOK, NotePage{})
}

// createNote stores a new note.
func (h *Handler) createNote(c *gin.Context) {
	var body NewNote
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, Problem{Message: err.Error()})
		return
	}
	c.JSON(http.StatusCreated, Note{Title: body.Title})
}

// getNote reads one note.
func (h *Handler) getNote(c *gin.Context) {
	var uri NoteURI
	_ = c.ShouldBindUri(&uri)
	if uri.ID == "" {
		c.JSON(http.StatusNotFound, Problem{Message: "not found", Slug: "note-not-found"})
		return
	}
	c.JSON(http.StatusOK, Note{ID: uri.ID})
}

// deleteNote removes a note.
func (h *Handler) deleteNote(c *gin.Context) {
	_ = c.Param("noteId")
	c.Status(http.StatusNoContent)
}

// exportNote renders a note as plain text.
func (h *Handler) exportNote(c *gin.Context) {
	_ = c.Param("noteId")
	c.String(http.StatusOK, "note")
}

// attachment downloads a note's attachment.
func (h *Handler) attachment(c *gin.Context) {
	_ = c.Param("noteId")
	c.Data(http.StatusOK, "application/octet-stream", []byte("attachment"))
}

// uploadFile attaches a file to a note.
func (h *Handler) uploadFile(c *gin.Context) {
	_ = c.Param("noteId")
	_, _ = c.FormFile("file")
	_ = c.PostForm("caption")
	c.Status(http.StatusNoContent)
}

// noteEvents streams a note's changes.
func (h *Handler) noteEvents(c *gin.Context) {
	_ = c.Param("noteId")
	c.SSEvent("change", Note{})
}

// session reads the caller's session cookie.
func (h *Handler) session(c *gin.Context) {
	_, _ = c.Cookie("session")
	c.Status(http.StatusNoContent)
}
