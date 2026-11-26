# Publishing Rust SDK

Package: `bookstore-sdk`
Version: `0.1.0`

`gnr8` never stores registry credentials and never uploads packages. Run these commands in this generated SDK directory after reviewing the generated files.

1. `cargo test`
2. `cargo package`
3. `cargo publish --dry-run`
4. Publish with your own crates.io token when the dry run matches expectations.
