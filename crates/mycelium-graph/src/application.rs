//! Application layer - orchestration and coordination
//!
//! High-level coordination logic:
//! - Service registry with dependency injection
//! - Pipeline executor
//! - Configuration management

/// Service registry for dependency injection
pub mod registry;

/// Pipeline executor orchestration
pub mod executor;

/// Configuration management
pub mod config;

/// CLI interface
pub mod cli;

/// LLM extraction service with provider fallback
pub mod extraction;

/// Intelligent schema discovery from content and HTML
pub mod schema_discovery;

/// Advanced TOML pipeline definitions: parsing, template expansion, DAG validation, DOT/Mermaid export
pub mod pipeline_parser;

/// Registry for named GraphQL target plugins
pub mod graphql_plugin_registry;

/// Prometheus metrics counters, histograms, gauges and tracing initialisation
pub mod metrics;

/// Health check reporting for Kubernetes liveness and readiness probes
pub mod health;

/// REST API server for pipeline management (T30) + web dashboard (T31)
pub mod api_server;
