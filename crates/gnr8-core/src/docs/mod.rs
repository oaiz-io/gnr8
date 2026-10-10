//! The docs model: what gnr8 says about an API in prose form, derived once from the frozen graph.
//!
//! One builder ([`build`]) derives a [`model::DocsModel`] from the projected graph and the plan's
//! sibling SDK declarations; the Markdown renderer ([`markdown::render`]) prints it and derives no
//! fact; verification ([`verify`]) checks the samples against the same model. `StaticDocs` is one
//! view of it. Nothing here reads another target's output.

mod build;
pub mod identity;
pub(crate) mod markdown;
pub(crate) mod model;
pub(crate) mod sample;
pub mod verify;
