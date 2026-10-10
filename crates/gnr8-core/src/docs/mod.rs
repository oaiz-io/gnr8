//! The docs model: what gnr8 says about an API in prose form, derived once from the frozen graph.
//!
//! `StaticDocs` renders its pages from here. Nothing in this module reads another target's output;
//! every fact comes from the graph and the built-in declarations of the plan it runs in.

pub mod identity;
