//! Hexagonal port traits for stygian-proxy.
//!
//! Each submodule under `ports/` exposes a pure trait that captures a
//! capability boundary between the domain core (`manager`, `types`,
//! `strategy`) and any number of pluggable implementations living under
//! `adapters/`. The trait itself is always compiled so the manager plumbing
//! (field types, builder steps, hot-path integrations) does not depend on
//! any feature gate; concrete adapters live behind their respective
//! cargo features so they can be opted into without breaking the rest of
//! the crate.
//!
//! Currently exposed ports:
//!
//! - [`coherence::CoherencePort`] — evaluates a [`coherence::CoherenceContext`]
//!   and returns a [`coherence::CoherenceVerdict`]. Used by
//!   `ProxyManager::acquire_proxy_with_coherence` when the
//!   `coherence-validation` cargo feature is enabled.

pub mod coherence;
