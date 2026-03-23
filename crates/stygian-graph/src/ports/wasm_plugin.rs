//! WASM plugin port — dynamic plugin system
//!
//! Defines the interface for loading and executing WebAssembly plugins that
//! implement the [`ScrapingService`](crate::ports::ScrapingService) interface.  Any language that compiles to
//! WASM + WASI can be used to write a Stygian plugin.
//!
//! # Architecture
//!
//! ```text
//! stygian-graph
//!   └─ WasmPluginPort  ← this file
//!         │
//!         └─ WasmPluginLoader (adapters/wasm_plugin.rs)
//!               │
//!               ├─ wasmtime Engine + Component (feature = "wasm-plugins")
//!               └─ WasmScrapingService implements ScrapingService
//! ```
//!
//! # Feature gate
//!
//! The adapter that actually loads WASM files requires the `wasm-plugins`
//! Cargo feature.  The *port trait* is always available so application code
//! can depend on it without pulling in `wasmtime`.
//!
//! # Plugin contract
//!
//! A WASM plugin must export two functions with these signatures (in WASI
//! Preview 1 / C ABI style):
//!
//! | Export | Signature | Description |
//! | -------- | ----------- | ------------- |
//! | `plugin_name` | `() → *const u8` | Null-terminated UTF-8 name |
//! | `plugin_execute` | `(url_ptr: i32, url_len: i32, params_ptr: i32, params_len: i32, out_ptr: *mut i32) → i32` | Execute and return output length |
//!
//! See `examples/wasm-plugin/` for a Rust template.

use crate::domain::error::Result;
use crate::ports::ScrapingService;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// Metadata
// ─────────────────────────────────────────────────────────────────────────────

/// Static metadata about a loaded WASM plugin.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::wasm_plugin::WasmPluginMeta;
///
/// let meta = WasmPluginMeta {
///     name: "my-scraper".to_string(),
///     version: "0.1.0".to_string(),
///     description: "Scrapes example.com".to_string(),
///     path: std::path::PathBuf::from("plugins/my-scraper.wasm"),
/// };
/// assert_eq!(meta.name, "my-scraper");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginMeta {
    /// Human-readable plugin name (must match the `plugin_name` export)
    pub name: String,
    /// Semantic version string
    pub version: String,
    /// Short description of what the plugin scrapes
    pub description: String,
    /// Path to the `.wasm` file on disk
    pub path: PathBuf,
}

// ─────────────────────────────────────────────────────────────────────────────
// Port trait
// ─────────────────────────────────────────────────────────────────────────────

/// Port: load and manage WASM scraping plugins.
///
/// Implementations are responsible for:
/// 1. Discovering `.wasm` files in a plugin directory.
/// 2. Validating that they export the required functions.
/// 3. Wrapping each plugin as an `Arc<dyn ScrapingService>` for the service
///    registry.
/// 4. Hot-reloading when the plugin file changes on disk.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::ports::wasm_plugin::WasmPluginPort;
///
/// // Implemented by WasmPluginLoader in adapters/wasm_plugin.rs
/// async fn register_plugins<P: WasmPluginPort>(loader: &P) {
///     let plugins = loader.discover().await.unwrap();
///     for (meta, svc) in plugins {
///         println!("loaded plugin: {}", meta.name);
///         let _ = svc;
///     }
/// }
/// ```
#[async_trait]
pub trait WasmPluginPort: Send + Sync {
    /// Scan the configured plugin directory and load all `.wasm` files.
    ///
    /// Returns `(metadata, service)` pairs — the service can be registered
    /// directly in a [`ServiceRegistry`].
    ///
    /// [`ServiceRegistry`]: crate::application::registry::ServiceRegistry
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::wasm_plugin::WasmPluginPort;
    /// # async fn example(loader: impl WasmPluginPort) {
    /// let plugins = loader.discover().await.unwrap();
    /// println!("{} plugins found", plugins.len());
    /// # }
    /// ```
    async fn discover(&self) -> Result<Vec<(WasmPluginMeta, Arc<dyn ScrapingService>)>>;

    /// Load a single `.wasm` file by path.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::wasm_plugin::WasmPluginPort;
    /// # use std::path::PathBuf;
    /// # async fn example(loader: impl WasmPluginPort) {
    /// let path = PathBuf::from("plugins/my-scraper.wasm");
    /// let (meta, _svc) = loader.load(&path).await.unwrap();
    /// println!("loaded: {} v{}", meta.name, meta.version);
    /// # }
    /// ```
    async fn load(
        &self,
        path: &std::path::Path,
    ) -> Result<(WasmPluginMeta, Arc<dyn ScrapingService>)>;

    /// List metadata for all currently loaded plugins (without reloading).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use stygian_graph::ports::wasm_plugin::WasmPluginPort;
    /// # async fn example(loader: impl WasmPluginPort) {
    /// for meta in loader.loaded().await.unwrap() {
    ///     println!("{} v{}", meta.name, meta.version);
    /// }
    /// # }
    /// ```
    async fn loaded(&self) -> Result<Vec<WasmPluginMeta>>;
}
