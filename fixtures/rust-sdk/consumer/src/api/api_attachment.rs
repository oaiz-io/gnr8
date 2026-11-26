//! The `attachment` operation.

use super::client::Call;
use super::errors::Error;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Downloads a note's attachment.")]
    ///
    /// `GET /v1/notes/{noteId}/attachment`
    pub fn attachment(&self, note_id: &str) -> Result<Vec<u8>, Error> {
        let mut call = Call::new("GET", "attachment", "/v1/notes/{noteId}/attachment");
        call.path("noteId", note_id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.bytes()
    }
}
