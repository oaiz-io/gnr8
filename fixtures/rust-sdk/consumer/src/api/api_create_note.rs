//! The `createNote` operation.

use super::client::Call;
use super::errors::Error;
use super::models;

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Stores a new note.")]
    ///
    /// `POST /v1/notes`
    pub fn create_note(&self, body: &models::NewNote) -> Result<models::Note, Error> {
        let mut call = Call::new("POST", "createNote", "/v1/notes");
        call.json("application/json", body)?;
        call.bearer(0);
        call.idempotent("Idempotency-Key");
        call.returns(201);
        call.send(self)?.json()
    }
}
