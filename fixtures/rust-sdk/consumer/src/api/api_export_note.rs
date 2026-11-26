//! The `exportNote` operation.

use super::client::Call;
use super::errors::Error;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Renders a note as plain text.")]
    ///
    /// `GET /v1/notes/{noteId}/export`
    pub fn export_note(&self, note_id: &str) -> Result<Vec<u8>, Error> {
        let mut call = Call::new("GET", "exportNote", "/v1/notes/{noteId}/export");
        call.path("noteId", note_id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.bytes()
    }
}
