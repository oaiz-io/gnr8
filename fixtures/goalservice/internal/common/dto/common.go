// Package dto holds the request/response data-transfer objects shared across the
// goalservice domain. These structs are the schema source-of-truth that gnr8's
// analyzer (Phase 2) lowers to OpenAPI schemas and Go SDK models (Phase 3).
//
// Patterns deliberately exercised here (per TARGET-API.md §3.4–§3.5):
//   - json field renaming via struct tags
//   - binding:"required" → OpenAPI required list
//   - omitempty → optional field
//   - example tags → schema metadata
//   - embedded struct composition → flattened/promoted fields
//   - named string newtype + const set → string enum
package dto

import "github.com/google/uuid"

// HttpError is the standard error envelope returned on 4xx responses.
// It demonstrates scalar example tags and omitempty optionality on a []string field.
type HttpError struct {
	Message string   `json:"message" example:"error message" binding:"required"`
	Slug    string   `json:"slug,omitempty" example:"error-slug"`
	Hints   []string `json:"hints,omitempty"`
}

// CommandMessage is a minimal success envelope.
type CommandMessage struct {
	Message string `json:"message" binding:"required"`
}

// CommandMessageWithUUID embeds CommandMessage (composition) so the "message"
// field is promoted/flattened into the schema, and adds a uuid.UUID well-known
// type field. The embedded struct must flatten in the generated schema.
type CommandMessageWithUUID struct {
	CommandMessage           // embedded -> "message" promoted/flattened
	UUID           uuid.UUID `json:"uuid" binding:"required"`
}

// TargetDirection is a closed vocabulary expressed as a named string newtype.
// A named string type with a fixed const set maps to an OpenAPI string enum.
type TargetDirection string

const (
	// TargetDirectionGte means a higher measured value is better.
	TargetDirectionGte TargetDirection = "gte"
	// TargetDirectionLte means a lower measured value is better.
	TargetDirectionLte TargetDirection = "lte"
)
