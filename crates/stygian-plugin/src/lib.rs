//! stygian-plugin: Chrome browser plugin fallback scraper
//!
//! Provides a flexible, interactive visual data extraction framework as a fallback
//! when stygian-graph and stygian-browser cannot scrape a page.
//!
//! # Architecture
//!
//! Following hexagonal architecture with clear separation:
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │  Application / MCP Layer            │
//! │  (plugin_apply_template, etc.)      │
//! └──────────────┬──────────────────────┘
//!                │
//! ┌──────────────▼──────────────────────┐
//! │  Domain Layer (pure Rust)           │
//! │  ExtractionTemplate                 │
//! │  ExtractionRequest/Result           │
//! │  Transformation Pipeline            │
//! └──────────────┬──────────────────────┘
//!                │
//! ┌──────────────▼──────────────────────┐
//! │  Ports (traits)                     │
//! │  PluginTemplateStore                │
//! │  PluginExtractionPort               │
//! │  IdempotencyKeyStore                │
//! └──────────────┬──────────────────────┘
//!                │
//! ┌──────────────▼──────────────────────┐
//! │  Adapters (implementations)         │
//! │  FileTemplateStore                  │
//! │  ExtractionEngine                   │
//! │  MemoryIdempotencyStore             │
//! └─────────────────────────────────────┘
//! ```
//!
//! # Features
//!
//! - **Template-based extraction**: Define schema once, apply to multiple elements
//! - **Recording-based**: User clicks/highlights → learns pattern
//! - **Query-driven**: Declarative extraction with CSS/XPath selectors
//! - **Region-based**: Multiple independent zones, each with own rules
//! - **Multi-instance**: Iterate template across matching elements
//! - **Multi-set**: Extract different shapes from same page
//! - **Cross-page**: Reuse templates in crawl sessions
//! - **Idempotency**: Safe retries via ULID-based deduplication
//! - **Transformations**: Regex, type coercion, HTML stripping, etc.
//!
//! # Quick Start
//!
//! ```no_run
//! use stygian_plugin::domain::{ExtractionTemplate, Region, Selector, ExtractionRequest};
//! use stygian_plugin::ports::PluginExtractionPort;
//! use serde_json::json;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create a template with regions
//! let template = ExtractionTemplate::new("Product")
//!     .with_region(
//!         Region::new(
//!             "title",
//!             Selector::css(".product-title"),
//!             json!({"type": "string"}),
//!         )
//!     )
//!     .with_region(
//!         Region::new(
//!             "price",
//!             Selector::css(".product-price"),
//!             json!({"type": "number"}),
//!         )
//!     );
//!
//! // Create extraction request
//! let request = ExtractionRequest::new(
//!     template,
//!     "https://example.com/products",
//!     "<html>...</html>"
//! );
//!
//! // Execute (requires a PluginExtractionPort adapter)
//! // let result = extraction_port.execute(&request).await?;
//! # Ok(())
//! # }
//! ```

#![allow(clippy::multiple_crate_versions)]

// ═══════════════════════════════════════════════════════════════════════════
// Module Organization
// ═══════════════════════════════════════════════════════════════════════════

/// Error types
pub mod error;

/// Domain layer: pure business logic and value objects
///
/// Contains zero external dependencies; all I/O happens in adapters.
pub mod domain;

/// Port trait definitions: interfaces adapters must implement
///
/// The domain depends only on these traits, not on concrete implementations.
pub mod ports;

/// Adapter implementations: concrete providers of port traits
pub mod adapters;

/// Storage adapters: template persistence, idempotency tracking
pub mod storage;

/// MCP (Model Context Protocol) server for the plugin system
pub mod mcp;

/// Runtime configuration for the standalone MCP server
pub mod config;

// ═══════════════════════════════════════════════════════════════════════════
// Public API Re-exports
// ═══════════════════════════════════════════════════════════════════════════

pub use domain::{
    ExtractionRequest, ExtractionResult, ExtractionTemplate, IdempotencyKey, Region, Selector,
    Transformation,
};
pub use error::{PluginError, Result};
pub use mcp::{McpPluginServer, McpRequestHandler};
pub use ports::{IdempotencyKeyStore, PluginExtractionPort, PluginTemplateStore};
