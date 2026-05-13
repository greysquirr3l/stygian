//! Domain types for template extraction

pub mod extraction;
pub mod idempotency;
pub mod selector;
pub mod transformation;

pub use extraction::{
    ExtractionMetadata, ExtractionRequest, ExtractionResult, ExtractionTemplate, Region,
    RegionStatus, TemplateMetadata,
};
pub use idempotency::IdempotencyKey;
pub use selector::Selector;
pub use transformation::Transformation;
