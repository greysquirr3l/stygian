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

// Workspace re-exports. Snippets can `use stygian_book_tests::*;` or
// refer to specific items via the original crate paths (e.g.
// `stygian_graph::domain::Pipeline`).
//
// Note: three workspace crates (browser, graph, plugin) define a local
// `pub type Result<T> = Result<T, CrateError>;` alias. When test code
// imports via glob (`use stygian_book_tests::stygian_browser::*;`)
// or `pub use stygian_book_tests::*`, that 1-generic alias shadows
// `std::result::Result<T, E>`. Snippets that write
// `Result<(), Box<dyn std::error::Error>>` therefore fail to compile.
// The fix is to use the snippet's full path:
// `std::result::Result<(), Box<dyn std::error::Error>>` — a small doc
// fix tracked in #131's doc-rot cleanup PR.

pub use stygian_graph;
pub use stygian_browser;
pub use stygian_proxy;
pub use stygian_charon;
pub use stygian_mcp;
pub use stygian_plugin;

pub mod graph {
    pub use ::stygian_graph::*;
}
pub mod browser {
    pub use ::stygian_browser::*;
}
pub mod proxy {
    pub use ::stygian_proxy::*;
}
pub mod charon {
    pub use ::stygian_charon::*;
}
pub mod mcp {
    pub use ::stygian_mcp::*;
}
pub mod plugin {
    pub use ::stygian_plugin::*;
}