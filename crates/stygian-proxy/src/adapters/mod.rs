//! Adapters implementing the port traits declared in [`crate::ports`].
//!
//! Each submodule maps a concrete backend or strategy onto its
//! corresponding port. The adapter itself is feature-gated by the
//! owning capability (e.g. `coherence-validation`) so the default
//! build remains as minimal as the underlying ports.
//!
//! Currently exposed adapters:
//!
//! - [`coherence::DefaultCoherenceValidator`] — applies the
//!   country + DNS + WebRTC public IP /16 + timezone rules from
//!   the 2026 scraping guide (L2839, L3135-3138). Off by default
//!   behind the `coherence-validation` cargo feature.

#[cfg(feature = "coherence-validation")]
pub mod coherence;
