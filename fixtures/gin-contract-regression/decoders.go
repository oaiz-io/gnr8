package gincontract

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/gin-gonic/gin"
)

// FolderName refuses an explicit null. Only its UnmarshalJSON says so, so gnr8
// must not publish the null as accepted unless the pipeline states it.
type FolderName string

func (n *FolderName) UnmarshalJSON(data []byte) error {
	if string(data) == "null" {
		return fmt.Errorf("name must not be null")
	}
	var value string
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	*n = FolderName(value)
	return nil
}

// MoveFolderParent has no decoder of its own: MoveFolderRequest's decodes it.
type MoveFolderParent struct {
	FolderID *string `json:"folderId,omitempty"`
}

// MoveFolderRequest decodes itself. It refuses a null parentFolder, and reads a
// null note as clearing the note.
type MoveFolderRequest struct {
	Note         *string           `json:"note,omitempty"`
	ParentFolder *MoveFolderParent `json:"parentFolder,omitempty"`
}

func (r *MoveFolderRequest) UnmarshalJSON(data []byte) error {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}
	if string(raw["parentFolder"]) == "null" {
		return fmt.Errorf("parentFolder cannot be null")
	}
	type plain MoveFolderRequest
	return json.Unmarshal(data, (*plain)(r))
}

type RenameFolderRequest struct {
	Name     FolderName      `json:"name,omitempty"`
	Previous *FolderName     `json:"previous"`
	Proof    json.RawMessage `json:"proof" binding:"required"`
}

// DecodedReply has a decoder but is only ever a reply, so no request null is in
// question.
type DecodedReply struct {
	Label *string `json:"label"`
}

func (r *DecodedReply) UnmarshalJSON(data []byte) error {
	type plain DecodedReply
	return json.Unmarshal(data, (*plain)(r))
}

func RegisterDecoderRoutes(r *gin.Engine, h *Handler) {
	folders := r.Group("/v1/items/folders")
	folders.POST("/move", h.moveFolder)
	folders.POST("/rename", h.renameFolder)
}

func (h *Handler) moveFolder(c *gin.Context) {
	var body MoveFolderRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.Status(http.StatusNoContent)
}

func (h *Handler) renameFolder(c *gin.Context) {
	var body RenameFolderRequest
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(http.StatusBadRequest, MessageResponse{Message: err.Error()})
		return
	}
	c.JSON(http.StatusOK, DecodedReply{})
}
