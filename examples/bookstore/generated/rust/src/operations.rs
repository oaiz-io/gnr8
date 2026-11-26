//! One method on the client per API operation.

use super::client::Call;
use super::errors::Error;
use super::models;

/// The query, header and cookie parameters of `list_books`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListBooksParams {
    pub genre: Option<String>,
}

impl ListBooksParams {
    /// The parameters with every one `None`, which sends none of them.
    #[must_use]
    pub fn new() -> Self {
        Self { genre: None }
    }
}

impl super::Client {
    #[cfg_attr(
        not(doctest),
        doc = "Returns every book in the catalogue.\n\nPass a genre to narrow the results to one genre; omit it to list everything."
    )]
    ///
    /// `GET /books`
    pub fn list_books(&self, params: &ListBooksParams) -> Result<models::BookList, Error> {
        let mut call = Call::new("GET", "listBooks", "/books");
        call.query("genre", &params.genre, "form", true)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }

    #[cfg_attr(
        not(doctest),
        doc = "Adds a book to the catalogue.\n\nThe book is stored immediately and returned with its generated identifier."
    )]
    ///
    /// `POST /books`
    pub fn create_book(&self, body: &models::CreateBookRequest) -> Result<models::Book, Error> {
        let mut call = Call::new("POST", "createBook", "/books");
        call.json("application/json", body)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(201);
        call.send(self)?.json()
    }

    #[cfg_attr(not(doctest), doc = "Permanently removes one book from the catalogue.")]
    ///
    /// `DELETE /books/{id}`
    pub fn delete_book(&self, id: &str) -> Result<models::ErrorResponse, Error> {
        let mut call = Call::new("DELETE", "deleteBook", "/books/{id}");
        call.path("id", id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }

    #[cfg_attr(not(doctest), doc = "Returns one book by its identifier.")]
    ///
    /// `GET /books/{id}`
    pub fn get_book(&self, id: &str) -> Result<models::Book, Error> {
        let mut call = Call::new("GET", "getBook", "/books/{id}");
        call.path("id", id)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }

    #[cfg_attr(
        not(doctest),
        doc = "Replaces the mutable fields of one book.\n\nFields omitted from the payload keep their current values."
    )]
    ///
    /// `PUT /books/{id}`
    pub fn update_book(
        &self,
        id: &str,
        body: &models::UpdateBookRequest,
    ) -> Result<models::Book, Error> {
        let mut call = Call::new("PUT", "updateBook", "/books/{id}");
        call.path("id", id)?;
        call.json("application/json", body)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }
}
