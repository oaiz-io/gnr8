//! The `getNote` operation.

use super::client::Call;
use super::errors::Error;
use super::models;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Reads one note.")]
    ///
    /// `GET /v1/notes/{noteId}`
    pub fn get_note(&self, note_id: &str) -> Result<models::Note, Error> {
        let mut call = Call::new("GET", "getNote", "/v1/notes/{noteId}");
        call.path("noteId", note_id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }
}
