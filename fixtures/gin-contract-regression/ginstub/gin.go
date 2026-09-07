package gin

import (
	"io"
	"mime/multipart"
	"net/http"
)

type H map[string]any
type HandlerFunc func(*Context)

type Engine struct{}
type RouterGroup struct{}
type Context struct {
	Request *http.Request
	Writer  http.ResponseWriter
}

func (e *Engine) Group(string) *RouterGroup { return &RouterGroup{} }

func (g *RouterGroup) Group(string) *RouterGroup  { return &RouterGroup{} }
func (g *RouterGroup) GET(string, HandlerFunc)    {}
func (g *RouterGroup) POST(string, HandlerFunc)   {}
func (g *RouterGroup) PATCH(string, HandlerFunc)  {}
func (g *RouterGroup) DELETE(string, HandlerFunc) {}

func (c *Context) Param(string) string                                             { return "" }
func (c *Context) Query(string) string                                             { return "" }
func (c *Context) DefaultQuery(string, string) string                              { return "" }
func (c *Context) GetQuery(string) (string, bool)                                  { return "", false }
func (c *Context) GetHeader(string) string                                         { return "" }
func (c *Context) Cookie(string) (string, error)                                   { return "", nil }
func (c *Context) ShouldBindJSON(any) error                                        { return nil }
func (c *Context) JSON(int, any)                                                   {}
func (c *Context) Status(int)                                                      {}
func (c *Context) AbortWithStatus(int)                                             {}
func (c *Context) Header(string, string)                                           {}
func (c *Context) FileAttachment(string, string)                                   {}
func (c *Context) File(string)                                                     {}
func (c *Context) Data(int, string, []byte)                                        {}
func (c *Context) DataFromReader(int, int64, string, io.Reader, map[string]string) {}
func (c *Context) Redirect(int, string)                                            {}
func (c *Context) PostForm(string) string                                          { return "" }
func (c *Context) FormFile(string) (*multipart.FileHeader, error)                  { return nil, nil }
func (c *Context) MultipartForm() (*multipart.Form, error)                         { return nil, nil }
func (c *Context) SSEvent(string, any)                                             {}
func (c *Context) Stream(func(io.Writer) bool)                                     {}
