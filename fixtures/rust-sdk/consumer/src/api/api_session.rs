//! The `session` operation.

use super::client::Call;
use super::errors::Error;

/// The query, header and cookie parameters of `session`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionParams {
    pub session: Option<String>,
}

impl SessionParams {
    /// The parameters with every one `None`, which sends none of them.
    #[must_use]
    pub fn new() -> Self {
        Self { session: None }
    }
}

impl super::Client {
    #[cfg_attr(not(doctest), doc = "Reads the caller's session cookie.")]
    ///
    /// `GET /v1/session`
    pub fn session(&self, params: &SessionParams) -> Result<(), Error> {
        let mut call = Call::new("GET", "session", "/v1/session");
        call.cookie("session", &params.session, true)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.send(self)?;
        Ok(())
    }
}
