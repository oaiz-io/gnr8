# Rust SDK

[Agent docs index](../agents/index.md)

`RustSdk` writes a Rust client for your API: a blocking `Client`, one method per operation, the
models, the error types, a README and reference, and contract tests. It is the Rust counterpart of
`GoSdk`, `PySdk` and `TsSdk`. To emit models alone, use [`RustModels`](rust-models.md).

```rust
.target(
    RustSdk::new()
        .module("acme-books")   // names the crate
        .to("generated/rust"),
)
```

The generated code depends on `serde` (with `derive`) and `serde_json`, and on nothing else: no
async runtime, no HTTP or TLS crate, no gnr8 runtime crate. It contains no HTTP client either: the
caller supplies one exchange at a time through the generated `Transport` trait. That is the seam the
Go, Python and TypeScript SDKs expose as an option; in Rust it is required, because the standard
library has no HTTP client and every real one is a third-party choice that is yours to make.

## Declaration

| Method | Meaning |
|---|---|
| `module(name)` | Required. The crate's name comes from it, by the derivation every SDK target shares (`acme-books` → `acmebooks`); `package(SdkPackageMetadata::new().name("acme-books"))` sets it exactly. |
| `to(dir)` | Required. The output directory. |
| `exclude(selector)` | Leave the operations `selector` matches out of this target, with every model only they reach. |
| `model_root(schema)` / `all_models()` / `without_registered_schema_roots()` | Which models are emitted beyond those the operations reach; as for the other SDKs. |
| `layout(..)` / `split_files()` | One file for all operations and one for all models (default), or one file per operation and per model. |
| `docs(..)` / `without_docs()` | The `README.md` and `reference.md`. On by default. |
| `package_metadata(bool)` / `package(SdkPackageMetadata)` | Whether the tree is a crate or a module tree; below. |
| `without_contract_tests()` | Stop emitting `contract_test.rs`. |
| `source_only()` | No docs and no package metadata: a module tree for a crate you own. |

## Two tree forms

With package metadata (the default) the output is a crate:

```text
generated/rust/Cargo.toml      serde + serde_json, edition 2021, no [workspace] table
generated/rust/PUBLISHING.md
generated/rust/README.md
generated/rust/reference.md
generated/rust/src/lib.rs      the root: lint allowances, modules, re-exports
generated/rust/src/client.rs   Client, ClientBuilder, Transport, Request, Response, RequestOptions
generated/rust/src/errors.rs   Error, ApiError, TransportError, DecodeError, EncodeError, AuthConfigurationError
generated/rust/src/operations.rs
generated/rust/src/models.rs
generated/rust/src/multipart.rs      only when an operation sends multipart
generated/rust/src/contract_test.rs  #[cfg(test)]
```

The manifest has no `[workspace]` table, so a Cargo workspace can list the crate as a member. Add it
to `members` when it lives inside one.

With `.package_metadata(false)` (or `.source_only()`) the output is the same tree without `src/`,
rooted at `mod.rs`, with no manifest. A crate you own declares it:

```rust
// src/lib.rs
pub mod api;
```

Generated code names its own items through `self::` and `super::` only, so the tree is the same
wherever it is mounted.

## The transport

```rust
pub trait Transport: Send + Sync {
    fn send(&self, request: &Request) -> Result<Response, TransportError>;
}

pub struct Request {
    pub method: &'static str,
    pub url: String,                    // absolute: base URL, path and query
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Option<Duration>,      // for this attempt; the transport enforces it
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
```

A transport sends one exchange and returns the response whatever its status. It never follows a
redirect and never retries: the client does both, so its policy cannot be bypassed by a
transport's defaults. A closure is a transport too:

```rust
let client = Client::new("https://api.example.com", |request: &Request| {
    my_http_client.send(request) // map to Response, or to TransportError::new(..)
});
```

### Over a Unix socket

`fixtures/rust-sdk/consumer/src/unix.rs` is a complete, standard-library-only transport that sends
each request as HTTP/1.0 over a Unix socket, to a relay that forwards it and adds the credential. It
opens one connection per exchange, sends `Connection: close`, refuses a header with a line break,
and reads the reply to the end of the stream up to a bound. Build the client with
`.transport_auth()` so it attaches no credential of its own:

```rust
let client = Client::builder("http://api.internal", UnixSocketTransport::new("/run/relay.sock"))
    .transport_auth()
    .build();
```

### Wrapping an HTTP client

Any blocking HTTP client works the same way: build its request from `Request`, send it without
following redirects, and copy the status, headers and body into `Response`. Map a failure to send
or receive into `TransportError::new(message)`.

## Calls

```rust
let client = Client::builder("https://api.example.com", transport)
    .bearer_token(token)
    .build();

let book = client.get_book("b_123")?;
let page = client.list_books(&ListBooksParams { page_size: Some(50), ..ListBooksParams::new() })?;
let made = client.create_book(&models::CreateBookRequest::new(title, author))?;
let slow = client
    .with_options(RequestOptions::new().timeout(Duration::from_secs(120)))
    .export_books()?;
```

A method takes its path parameters in path order, then the body, then `&{Op}Params` when the
operation has query, header or cookie parameters. A params struct is built with `new` over its
required parameters; every other field starts as `None`, which sends nothing. A text path parameter
is a `&str`; a number, boolean or named enum is taken by value.

| Body | Argument |
|---|---|
| JSON, form | `&models::Model` |
| multipart | `&models::Model`; a file field is a `MultipartFile` (or `Vec<MultipartFile>`) |
| text | `&str` |
| binary | `&[u8]` |
| several media types | `&{Op}Body`, an enum with one variant per media type |
| optional | `Option<…>` of the above |

| Success | Return |
|---|---|
| a JSON model | the model |
| `text/*` | `String` |
| binary, or an event stream without a schema | `Vec<u8>` |
| nothing | `()` |

A success that has a body beside one that has none returns `Option<…>`. A status the operation does
not declare as a success is `Error::Api`.

`client.with_options(..)` returns a cheap clone that applies `RequestOptions` to every call made
through it: a timeout, a retry count, an idempotency key, opting into redirects, and hook metadata.

### Pagination

An operation the graph configures with `ConfigurePagination` gains two methods, named as in the
other SDKs: `{op}_pages`, an iterator over every page, and `iter_{op}`, an iterator over every item
of every page. Each fetches a page only when the iteration reaches it, and yields
`Result<…, Error>`; an error ends the iteration.

```rust
for item in client.iter_list_books(&ListBooksParams::new()) {
    println!("{}", item?.title);
}
```

## Client behavior

The client behaves as the Go, Python and TypeScript clients do.

- **Credentials.** A credential setter exists for each scheme the graph declares:
  `bearer_token`, `basic_auth`, `api_key` (the key for every API-key scheme) and
  `api_key_for(scheme_or_name, key)`. A call that needs a credential the client was not given is
  `Error::Auth`, before any request. `.transport_auth()` leaves authentication to the transport.
- **Retries.** `GET`, `HEAD`, `OPTIONS`, `PUT` and `DELETE`, and operations marked idempotent, are
  retried on a transport error, a `5xx`, or a status the graph's runtime policy lists (`408` and
  `429` by default), honoring `Retry-After`, with doubling backoff from 100 ms. The waits of one
  call never total more than 60 s.
- **Timeouts.** The timeout bounds each attempt; the transport enforces it. With retries a call can
  take `timeout × (max_retries + 1)` plus the waits.
- **Redirects.** Never followed unless the call opts in with `follow_redirects(true)`. When it does,
  the client sends the next hop through the transport, and drops `Authorization`, `Cookie`,
  `Proxy-Authorization` and header API keys on a hop to another origin.
- **Hooks.** `on_request` may edit each attempt's `Request`; `on_response` sees each `Response`;
  `on_error` sees the call's final error.

## Errors

```rust
match client.get_book("missing") {
    Err(Error::Api(error)) if error.is_not_found() => { /* … */ }
    Err(Error::Api(error)) => {
        let problem: models::Problem = error.decode().map_err(Error::Decode)?;
    }
    Err(other) => return Err(other),
    Ok(book) => { /* … */ }
}
```

`ApiError` carries the status, the headers and the raw body, with `message()`, `slug()`, `hints()`,
`request_id()`, `json_body()` and `decode::<T>()` to read them. `Error` is `#[non_exhaustive]`.

## Graph shapes

| Graph shape | Rust |
|---|---|
| string, boolean | `String`, `bool` |
| 8-, 16-, 32-, 64-bit integers; 32- and 64-bit floats | `i8` … `u64`, `f32`, `f64` |
| uuid, date-time, date, duration, decimal, email, uri | `String`, the wire text |
| array; map keyed by string; free-form value | `Vec<T>`; `BTreeMap<String, V>`; `serde_json::Value` |
| named object, named string enum, named alias | `struct`, `enum`, `type` |
| inline enum, at any position | `String` (`&str` as an argument); its members are in the docs, not enforced |
| byte string | by where it travels: a raw body `&[u8]`, a binary reply `Vec<u8>`, a multipart file `MultipartFile`, a named byte alias `Vec<u8>` |

These are refused, in one error that names every position:

- a union: the graph states no discriminator;
- an inline object: declare it as a named type so it has a Rust name;
- a byte string inside JSON, directly or through an alias: the graph does not state its text
  encoding;
- a map keyed by anything but a string, and a number of any other width;
- an alias that refers to itself;
- a Rust name that collides or is reserved (two operations with one method name, a parameter type
  named like a model, an operation named `new`, `builder` or `with_options`): rename it with
  `RenameOperation` or `RenameType`.

`.exclude(..)` removes an operation, and the models only it reaches, from this target alone. Python-
and TypeScript-sourced services usually declare unions, so the target refuses them until those
operations are excluded.

## Formatting and lints

The generated tree is exactly what `rustfmt` writes in its 2021 and 2024 style editions, so a
crate's `cargo fmt` leaves it alone; nothing spawns `rustfmt` at generation. It compiles in editions
2021 and 2024. The root carries the lint allowances `RustModels` carries, for what the API decides
(names, prose, shape); the runtime and operations pass default Clippy without them.

## Verification

`gnr8 verify` runs `contract_test.rs` with `cargo test` in a crate it synthesizes around the tree,
so it never reads or changes your manifest, lockfile or workspace. It also compiles the samples the
README, the reference and a `StaticDocs` site print against the generated crate, and runs each call
on a transport that answers the reply its page prints. `gnr8 doctor` checks the tree with
`cargo check`. Both need `cargo` on `PATH`; without it the Rust suites are skipped. The contract
tests are a `#[cfg(test)]` module of the tree, so your own `cargo test` runs them too.

## Limits

- The client is blocking; there is no async variant.
- No transport ships with the SDK.
- Unions, inline objects and a byte string inside JSON are refused.
- An inline enum is a `String`, so an unknown value is never rejected; a named enum rejects one.
- There is no generated CLI for Rust.
