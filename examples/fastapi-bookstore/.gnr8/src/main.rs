//! gnr8 generation lifecycle for the FastAPI bookstore example. This file IS the config — edit it to
//! adapt how the API is parsed and how the OpenAPI document + Python SDK are generated. It is an
//! ordinary Rust binary that composes a `Pipeline` and hands it to the gnr8 worker runtime. The
//! built-in stages below are declarations the installed `gnr8` host executes; only your own stages
//! run here.
//!
//! Run it from the example root so `FastApi::new().inputs(["."])` analyzes the `app/` package here.
//! The input is the project root (`.`), not `app/`, so the source's own `from app.models import …`
//! absolute imports resolve; `.gnr8/` is excluded from language detection so the tree reads as Python:
//!
//! ```sh
//! cd examples/fastapi-bookstore
//! gnr8 generate
//! ```
//!
//! This reproduces the committed `examples/fastapi-bookstore/generated/` output (same routes/schemas/
//! title/base path). Every setting is a method call below — there is no `config.toml`:
//!   inputs            → FastApi::new().inputs(["."])     (the static `app/` package; never executed)
//!   route prefix      → extracted from APIRouter(prefix="/books")
//!   title             → SetTitle::new("Bookstore API")
//!   pagination        → ConfigurePagination::cursor(list_books, cursor, next_cursor, books)
//!   output.openapi    → OpenApi31::new().to("generated/openapi.yaml")
//!   output.sdk + module → PySdk::new().module("example.com/bookstore/sdk").to("generated/sdk")
//!                              .cli(SdkCli::new("bookstore").base_url("http://127.0.0.1:8000"))
//! plus a Header post-process that stamps the generated banner on every .py file.
//!
//! The FastAPI app is parsed STATICALLY (pyextract reads the `ast` — it never imports or runs the
//! app), so no `pip install` is needed. There is no auth in the source, so no `ApplySecurity` stage.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(FastApi::new().inputs(["."]))
            .transform(SetTitle::new("Bookstore API"))
            .transform(ConfigurePagination::cursor(
                OperationSelector::operation("list_books"),
                "cursor",
                "next_cursor",
                "books",
            ))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("generated/sdk")
                    .cli(bookstore_cli()),
            )
            .post(Header::generated()),
    )
}

fn bookstore_cli() -> SdkCli {
    SdkCli::new("bookstore")
        .base_url("http://127.0.0.1:8000")
        .topic(
            CliTopic::new("books")
                .concept("Browse and manage the catalogue")
                .command(
                    CliCommand::operation("list_books", "list")
                        .example("bookstore books list --genre fiction")
                        .see_also(["books get"])
                        .docs_url("https://example.com/cli/books/list"),
                )
                .command(
                    CliCommand::operation("get_book", "get")
                        .positional("book_id")
                        .selector(CliSelector::new("list_books", "id", "id"))
                        .example("bookstore books get 1"),
                )
                .command(
                    CliCommand::operation("create_book", "create")
                        .example("bookstore books create --body '{\"title\":\"Dune\"}'"),
                )
                .command(
                    CliCommand::operation("update_book", "update")
                        .positional("book_id")
                        .body_fields()
                        .example("bookstore books update 1 --title Dune"),
                ),
        )
        .rename_error(CliRenameError::new(["books", "list-books"], "books list"))
        .view(
            CliView::schema("Book")
                .preview(["id", "title", "author"])
                .table(["id", "title", "author"]),
        )
}
