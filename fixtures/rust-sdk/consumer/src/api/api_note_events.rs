//! The `noteEvents` operation.

use super::client::Call;
use super::errors::Error;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Streams a note's changes.")]
    ///
    /// `GET /v1/notes/{noteId}/events`
    pub fn note_events(&self, note_id: &str) -> Result<Vec<u8>, Error> {
        let mut call = Call::new("GET", "noteEvents", "/v1/notes/{noteId}/events");
        call.path("noteId", note_id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.bytes()
    }
}
