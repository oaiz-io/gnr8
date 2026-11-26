//! The `listNotes` operation.

use super::client::Call;
use super::errors::Error;
use super::models;

/// The query, header and cookie parameters of `list_notes`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListNotesParams {
    pub x_trace: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
    pub order: Option<String>,
    pub tag: Option<Vec<String>>,
}

impl ListNotesParams {
    /// The parameters with every one `None`, which sends none of them.
    #[must_use]
    pub fn new() -> Self {
        Self {
            x_trace: None,
            cursor: None,
            limit: None,
            order: None,
            tag: None,
        }
    }
}

/// The pages of `list_notes`, each fetched when the iteration reaches it.
pub struct ListNotesPages {
    client: super::Client,
    params: ListNotesParams,
    done: bool,
}

impl Iterator for ListNotesPages {
    type Item = Result<models::NotePage, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let client = &self.client;
        let result = client.list_notes(&self.params);
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.done = true;
                return Some(Err(error));
            }
        };
        let params = &mut self.params;
        let more = super::client::advance_cursor(&mut params.cursor, &page.next_cursor);
        self.done = !more;
        Some(Ok(page))
    }
}

/// The items of every page of `list_notes`, each page fetched when the iteration reaches it.
pub struct ListNotesItems {
    pages: ListNotesPages,
    items: std::vec::IntoIter<models::Note>,
}

impl Iterator for ListNotesItems {
    type Item = Result<models::Note, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.items.next() {
                return Some(Ok(item));
            }
            let page = match self.pages.next()? {
                Ok(page) => page,
                Err(error) => return Some(Err(error)),
            };
            let items = super::client::page_items(page.items);
            self.items = items.into_iter();
        }
    }
}

impl super::Client {
    #[cfg_attr(
        not(doctest),
        doc = "Lists the notes, newest first.\n\nPass the next cursor of a page to read the page after it."
    )]
    ///
    /// `GET /v1/notes`
    pub fn list_notes(&self, params: &ListNotesParams) -> Result<models::NotePage, Error> {
        let mut call = Call::new("GET", "listNotes", "/v1/notes");
        call.header("X-Trace", &params.x_trace, false)?;
        call.query("cursor", &params.cursor, "form", true)?;
        call.query("limit", &params.limit, "form", true)?;
        call.query("order", &params.order, "form", true)?;
        call.query("tag", &params.tag, "form", true)?;
        call.api_key_header(0, "ApiKeyAuth", "X-API-Key");
        call.returns(200);
        call.send(self)?.json()
    }

    /// Every page of `list_notes`, each fetched when the iteration reaches it.
    pub fn list_notes_pages(&self, params: &ListNotesParams) -> ListNotesPages {
        ListNotesPages {
            client: self.clone(),
            params: params.clone(),
            done: false,
        }
    }

    /// Every item of every page of `list_notes`, each page fetched when the iteration reaches it.
    pub fn iter_list_notes(&self, params: &ListNotesParams) -> ListNotesItems {
        let pages = self.list_notes_pages(params);
        ListNotesItems {
            pages,
            items: Vec::new().into_iter(),
        }
    }
}
