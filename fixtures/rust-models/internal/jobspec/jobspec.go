// Package jobspec is the Rust models fixture: the payloads a job runner exchanges with a service that
// schedules jobs. Nothing here is routed; the `.gnr8/` pipeline registers the event the service
// writes and the output it reads, and the RustModels target selects them by their graph ids.
package jobspec

import "encoding/json"

// JobStatus is where a job is in its lifecycle. Two members spell the same words in different
// case, so their Rust variants must be told apart while the wire values stay exact.
type JobStatus string

const (
	JobStatusQueued        JobStatus = "queued"
	JobStatusInProgress    JobStatus = "in-progress"
	JobStatusInProgressOld JobStatus = "IN_PROGRESS"
	JobStatusDone          JobStatus = "done"
)

// FileReference points at a file in the job's workspace. It is reached from the event the service
// writes and from the output it reads, and the two directions answer presence differently, so the
// graph projects it into one model per direction.
type FileReference struct {
	Path      string          `json:"path"`
	SizeBytes int64           `json:"size_bytes"`
	MediaType string          `json:"media_type,omitempty"`
	Children  []FileReference `json:"children,omitempty"`
	Parent    *FileReference  `json:"parent,omitempty"`
}

// Event is what the service writes for a job runner to read.
type Event struct {
	ID          string            `json:"id"`
	Type        string            `json:"type"`
	Status      JobStatus         `json:"status"`
	Attempt     int32             `json:"attempt"`
	DryRun      bool              `json:"dryRun"`
	Input       FileReference     `json:"input"`
	Attachments []FileReference   `json:"attachments"`
	Labels      map[string]string `json:"labels,omitempty"`
	Params      map[string]any    `json:"params"`
	Payload     json.RawMessage   `json:"payload"`
	Note        *string           `json:"note"`
	FileID      string            `json:"fileId"`
	FileIDRaw   string            `json:"file_id"`
}

// JobOutput is what a job runner writes back for the service to read.
type JobOutput struct {
	Status    JobStatus       `json:"status" binding:"required"`
	Summary   *string         `json:"summary"`
	ExitCode  int             `json:"exit_code"`
	Retry     bool            `json:"retry"`
	Artifacts []FileReference `json:"artifacts" binding:"required"`
	Result    any             `json:"result"`
}

// BillingInvoice is unrelated to jobs. No root reaches it, so the Rust models leave it out, and its
// map keyed by integers, which the Rust target cannot represent (a JSON object key is a string),
// never stops generation. Naming it as a root makes generation fail with an error naming the field.
type BillingInvoice struct {
	Number       string          `json:"number"`
	TotalsByLine map[int]float64 `json:"totals_by_line"`
}
