//! Ports for the `stygian-mcp` aggregator.
//!
//! Consumer-owned port traits that the aggregator and any tool that
//! emits untrusted text consume. Adapters live in `crates/stygian-mcp/src/adapters/`.

pub mod prompt_injection;
