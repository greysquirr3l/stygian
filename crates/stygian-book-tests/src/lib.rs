//! Compile-test crate for code blocks in the mdbook.
//!
//! The book is documentation; this crate pulls the same workspace
//! dependencies so each fence can be type-checked against the real
//! APIs. mdbook-test runs each fence as an isolated single-file crate
//! with no Cargo.toml, which makes workspace crates unresolvable; this
//! crate fixes that by being a normal workspace member.
//!
//! Each `tests/<file>_<line>.rs` integration test contains a single
//! snippet extracted from the book, prefixed with the imports it needs.
//! `cargo test --workspace --test <file>` builds and runs the test
//! against the real workspace deps.

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

// Workspace re-exports so test snippets can `use stygian_graph::...;`
// without each snippet needing to know which workspace crate owns
// each path.
pub mod graph {
    pub use stygian_graph::*;
}
pub mod browser {
    pub use stygian_browser::*;
}
pub mod proxy {
    pub use stygian_proxy::*;
}
pub mod charon {
    pub use stygian_charon::*;
}
pub mod mcp {
    pub use stygian_mcp::*;
}
pub mod plugin {
    pub use stygian_plugin::*;
}
