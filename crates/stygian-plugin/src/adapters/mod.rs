//! Adapter implementations of port traits

pub mod extraction_engine;

#[cfg(feature = "graph-integration")]
pub mod scraping_service;

pub use extraction_engine::ExtractionEngine;

#[cfg(feature = "graph-integration")]
pub use scraping_service::PluginExtractionAdapter;
