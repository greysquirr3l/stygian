//! Public report types for the transport-realism layer.
//!
//! [`TransportCompatibility`] and [`TransportRealismReport`] are the
//! typed structs the [`AcquisitionRunner`][crate::acquisition::AcquisitionRunner]
//! attaches to the
//! [`AcquisitionResult::transport_realism`][crate::acquisition::AcquisitionResult::transport_realism]
//! field. They are also the structs downstream policy mapping (T83 /
//! T85 / T89 / T93) consumes as a strategy hint.

pub use super::scoring::TransportCompatibility;

/// Top-level transport-realism report (re-exported here for stable
/// import paths — see [`crate::transport_realism`]).
pub use super::scoring::TransportRealismReport;
