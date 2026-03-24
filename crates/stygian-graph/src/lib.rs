//! # Stygian Graph
#![allow(clippy::multiple_crate_versions)]
//!
//! A high-performance, graph-based web scraping engine for Rust.
//!
//! ## Overview
//!
//! Stygian treats scraping pipelines as Directed Acyclic Graphs (DAGs) where each node
//! is a pluggable service module (HTTP fetchers, AI extractors, headless browsers).
//! Built for extreme concurrency and extensibility using hexagonal architecture.
//!
//! ## Quick Start
//!
//! ```no_run
//! use stygian_graph::domain::graph::Pipeline;
//! use stygian_graph::domain::pipeline::PipelineUnvalidated;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a simple scraping pipeline
//!     let config = serde_json::json!({
//!         "nodes": [],
//!         "edges": []
//!     });
//!     
//!     let pipeline = PipelineUnvalidated::new(config)
//!         .validate()?
//!         .execute()
//!         .complete(serde_json::json!({"status": "success"}));
//!     
//!     println!("Pipeline complete: {:?}", pipeline.results());
//!     Ok(())
//! }
//! ```
//!
//! ## Architecture
//!
//! Stygian follows hexagonal (ports & adapters) architecture:
//!
//! - **Domain**: Core business logic (graph execution, pipeline orchestration)
//! - **Ports**: Trait definitions (service interfaces, abstractions)
//! - **Adapters**: Implementations (HTTP, AI providers, storage, caching)
//! - **Application**: Orchestration (service registry, executor, CLI)
//!
//! ## Features
//!
//! - 🕸️ **Graph-based execution**: DAG pipelines with petgraph
//! - 🤖 **Multi-AI support**: Claude, GPT, Gemini, Copilot, Ollama
//! - 🌐 **JavaScript rendering**: Optional browser automation via `stygian-browser`
//! - 📊 **Multi-modal extraction**: HTML, PDF, images, video, audio
//! - 🛡️ **Anti-bot handling**: User-Agent rotation, proxy support, rate limiting
//! - 🚀 **High concurrency**: Worker pools, backpressure, Tokio + Rayon
//! - 🔄 **Idempotent operations**: Safe retries with idempotency keys
//! - 📈 **Observability**: Metrics, tracing, monitoring
//!
//! ## Crate Features
//!
//! - `browser` (default): Include stygian-browser for JavaScript rendering
//! - `full`: All features enabled
//!
//! ## Request Signing
//!
//! Use [`ports::signing::SigningPort`] + [`adapters::signing::HttpSigningAdapter`] to attach
//! HMAC signatures, AWS Sig V4, OAuth 1.0a, or Frida RPC tokens to any outbound request.
//! No feature flag required — zero additional dependencies.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

// ═══════════════════════════════════════════════════════════════════════════
// Internal Module Organization (Hexagonal Architecture)
// ═══════════════════════════════════════════════════════════════════════════

/// Core domain logic - graph execution, pipelines, orchestration
///
/// **Hexagonal principle**: Domain never imports adapters, only ports (traits).
pub mod domain;

/// Port trait definitions - service abstractions
///
/// Defines interfaces that adapters must implement:
/// - `ScrapingService`: HTTP fetchers, browser automation
/// - `AIProvider`: LLM extraction services
/// - `CachePort`: Caching abstractions
/// - `CircuitBreaker`: Resilience patterns
pub mod ports;

/// Adapter implementations - infrastructure concerns
///
/// Concrete implementations of port traits:
/// - HTTP client with anti-bot features
/// - AI providers (Claude, GPT, Gemini, Ollama)
/// - Storage backends (file, S3, database)
/// - Cache backends (memory, Redis, file)
pub mod adapters;

/// Application layer - orchestration and coordination
///
/// High-level coordination logic:
/// - Service registry with dependency injection
/// - Pipeline executor
/// - CLI interface
/// - Configuration management
pub mod application;

// ═══════════════════════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════════════════════

/// Error types used throughout the crate
pub mod error {
    pub use crate::domain::error::*;
}

/// Re-exports for convenient imports
///
/// # Example
///
/// ```
/// use stygian_graph::prelude::*;
/// ```
pub mod prelude {
    pub use crate::domain::pipeline::*;
    pub use crate::error::*;
    pub use crate::ports::*;
}

// Re-export browser crate if feature is enabled
#[cfg(feature = "browser")]
#[cfg_attr(docsrs, doc(cfg(feature = "browser")))]
pub use stygian_browser;

/// MCP (Model Context Protocol) server — exposes scraping &amp; pipeline tools
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
