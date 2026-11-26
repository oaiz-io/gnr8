//! The `deleteNote` operation.

use super::client::Call;
use super::errors::Error;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Removes a note.")]
    ///
    /// `DELETE /v1/notes/{noteId}`
    pub fn delete_note(&self, note_id: &str) -> Result<(), Error> {
        let mut call = Call::new("DELETE", "deleteNote", "/v1/notes/{noteId}");
        call.path("noteId", note_id)?;
        call.bearer(0);
        call.send(self)?;
        Ok(())
    }
}
