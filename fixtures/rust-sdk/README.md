# Rust SDK fixture

A small Gin service (`notes/notes.go`) whose routes reach every kind of request and reply the
`RustSdk` target sends and reads: path, query, header and cookie parameters, a JSON body, a
multipart upload, text, binary and event-stream replies, cursor pagination, bearer and API-key
credentials, and an idempotent create.

`.gnr8/src/main.rs` emits two Rust SDKs from it:

- `generated/rust`: a publishable crate with `Cargo.toml`, docs and contract tests;
- `consumer/src/api`: a module tree, one file per operation and per model, which the hand-written
  `consumer` crate declares with `pub mod api;`.

The `consumer` crate talks to the API through a relay on a Unix socket. `consumer/src/unix.rs` is
that transport, written against the standard library alone, and `consumer/tests/wire.rs` drives the
generated client through it against a `UnixListener`. `crates/gnr8/tests/rust_sdk_e2e.rs` runs the
whole lifecycle over a copy of this project, and `make rust-sdk-consumer-test` runs the consumer's
checks in place.
