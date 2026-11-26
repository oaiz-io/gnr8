//! The `uploadFile` operation.

use super::client::Call;
use super::errors::Error;
use super::models;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Attaches a file to a note.")]
    ///
    /// `POST /v1/notes/{noteId}/files`
    pub fn upload_file(
        &self,
        note_id: &str,
        body: &models::UploadFileFormRequest,
    ) -> Result<(), Error> {
        let mut call = Call::new("POST", "uploadFile", "/v1/notes/{noteId}/files");
        call.path("noteId", note_id)?;
        call.multipart();
        call.part("caption", &body.caption)?;
        call.file("file", &body.file)?;
        call.bearer(0);
        call.send(self)?;
        Ok(())
    }
}
