//! WASM plugin adapter
//!
//! Implements [`WasmPluginPort`](crate::ports::wasm_plugin::WasmPluginPort) using `wasmtime` (feature = `"wasm-plugins"`).
//!
//! When the feature is *disabled*, only [`MockWasmPlugin`](wasm_plugin::MockWasmPlugin) is compiled — a
//! no-wasmtime stand-in that returns canned output, useful for tests.
//!
//! # Feature gate
//!
//! ```toml
//! [features]
//! wasm-plugins = ["dep:wasmtime"]
//! ```

use crate::domain::error::Result;
use crate::ports::wasm_plugin::{WasmPluginMeta, WasmPluginPort};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// MockWasmPlugin — always compiled, no wasmtime dep
// ─────────────────────────────────────────────────────────────────────────────

/// Mock WASM plugin for use in tests.
///
/// Returns a predetermined JSON output and never touches the filesystem.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::wasm_plugin::MockWasmPlugin;
/// use stygian_graph::ports::wasm_plugin::WasmPluginPort;
/// use std::path::PathBuf;
///
/// # tokio_test::block_on(async {
/// let loader = MockWasmPlugin::new();
/// let plugins = loader.discover().await.unwrap();
/// assert_eq!(plugins.len(), 1);
/// # });
/// ```
pub struct MockWasmPlugin {
    /// Directory the mock will pretend to scan (not used, purely metadata)
    pub plugin_dir: PathBuf,
}

impl Default for MockWasmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl MockWasmPlugin {
    /// Create a new mock loader.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_graph::adapters::wasm_plugin::MockWasmPlugin;
    /// let loader = MockWasmPlugin::new();
    /// ```
    pub fn new() -> Self {
        Self {
            plugin_dir: PathBuf::from("plugins"),
        }
    }

    fn mock_meta() -> WasmPluginMeta {
        WasmPluginMeta {
            name: "mock-wasm-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "Mock WASM plugin for testing".to_string(),
            path: PathBuf::from("plugins/mock.wasm"),
        }
    }
}

#[async_trait]
impl WasmPluginPort for MockWasmPlugin {
    async fn discover(&self) -> Result<Vec<(WasmPluginMeta, Arc<dyn ScrapingService>)>> {
        let meta = Self::mock_meta();
        let svc: Arc<dyn ScrapingService> = Arc::new(MockWasmService::new(meta.name.clone()));
        Ok(vec![(meta, svc)])
    }

    async fn load(
        &self,
        path: &std::path::Path,
    ) -> Result<(WasmPluginMeta, Arc<dyn ScrapingService>)> {
        let meta = WasmPluginMeta {
            name: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string(),
            version: "0.0.0".to_string(),
            description: "Mock-loaded plugin".to_string(),
            path: path.to_path_buf(),
        };
        let svc: Arc<dyn ScrapingService> = Arc::new(MockWasmService::new(meta.name.clone()));
        Ok((meta, svc))
    }

    async fn loaded(&self) -> Result<Vec<WasmPluginMeta>> {
        Ok(vec![Self::mock_meta()])
    }
}

/// Scraping service backed by the mock WASM plugin.
struct MockWasmService {
    /// Leaked name for &'static lifetime compatibility
    name: &'static str,
}

impl MockWasmService {
    fn new(name: String) -> Self {
        // Leak is intentional in test/mock code: each name is small and
        // the number of mock instances in a test process is bounded.
        let leaked: &'static str = Box::leak(name.into_boxed_str());
        Self { name: leaked }
    }
}

#[async_trait]
impl ScrapingService for MockWasmService {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let payload = serde_json::json!({
            "plugin": self.name,
            "url": input.url,
            "status": "mock",
        });
        Ok(ServiceOutput {
            data: payload.to_string(),
            metadata: serde_json::Value::Null,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WasmPluginLoader — only compiled when feature = "wasm-plugins"
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "wasm-plugins")]
pub use real::WasmPluginLoader;

#[cfg(feature = "wasm-plugins")]
mod real {
    //! Real wasmtime-backed WASM plugin loader.
    //!
    //! Each `.wasm` file in `plugin_dir` is compiled with a `wasmtime::Engine`
    //! and cached.  Invoking a plugin passes the serialised [`ServiceInput`] as
    //! a JSON byte slice through shared linear memory and reads back the
    //! serialised [`ServiceOutput`].

    use crate::domain::error::{Result, ServiceError, StygianError};
    use crate::ports::wasm_plugin::{WasmPluginMeta, WasmPluginPort};
    use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use wasmtime::{Engine, Linker, Module, Store};

    /// Context passed into the WASM store (WASI-compatible host state).
    pub struct HostState;

    /// Wasmtime-backed WASM plugin loader.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::wasm_plugin::WasmPluginLoader;
    /// use stygian_graph::ports::wasm_plugin::WasmPluginPort;
    /// use std::path::PathBuf;
    ///
    /// # tokio_test::block_on(async {
    /// let loader = WasmPluginLoader::new(PathBuf::from("plugins"));
    /// let plugins = loader.discover().await.unwrap();
    /// println!("loaded {} plugins", plugins.len());
    /// # });
    /// ```
    pub struct WasmPluginLoader {
        plugin_dir: PathBuf,
        engine: Engine,
        /// name → (meta, compiled module)
        cache: RwLock<HashMap<String, (WasmPluginMeta, Module)>>,
    }

    impl WasmPluginLoader {
        /// Create a loader that scans `plugin_dir` for `.wasm` files.
        ///
        /// # Example
        ///
        /// ```no_run
        /// use stygian_graph::adapters::wasm_plugin::WasmPluginLoader;
        /// use std::path::PathBuf;
        ///
        /// let loader = WasmPluginLoader::new(PathBuf::from("plugins"));
        /// ```
        pub fn new(plugin_dir: PathBuf) -> Self {
            Self {
                plugin_dir,
                engine: Engine::default(),
                cache: RwLock::new(HashMap::new()),
            }
        }

        async fn compile_module(&self, path: &std::path::Path) -> Result<(WasmPluginMeta, Module)> {
            let bytes = tokio::fs::read(path).await.map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "failed to read WASM file {}: {e}",
                    path.display()
                )))
            })?;

            let module = Module::from_binary(&self.engine, &bytes).map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "failed to compile WASM module {}: {e}",
                    path.display()
                )))
            })?;

            // Extract name from filename (plugins name themselves via export, but
            // filename is the authoritative ID at load time)
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let meta = WasmPluginMeta {
                name: name.clone(),
                version: "1.0.0".to_string(),
                description: format!("WASM plugin: {name}"),
                path: path.to_path_buf(),
            };

            Ok((meta, module))
        }
    }

    #[async_trait::async_trait]
    impl WasmPluginPort for WasmPluginLoader {
        async fn discover(&self) -> Result<Vec<(WasmPluginMeta, Arc<dyn ScrapingService>)>> {
            let Ok(mut entries) = tokio::fs::read_dir(&self.plugin_dir).await else {
                return Ok(vec![]); // dir missing → no plugins
            };

            let mut results = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                    continue;
                }
                match self.load(&path).await {
                    Ok(pair) => results.push(pair),
                    Err(e) => {
                        tracing::warn!("skipping WASM plugin {}: {e}", path.display());
                    }
                }
            }

            Ok(results)
        }

        async fn load(
            &self,
            path: &std::path::Path,
        ) -> Result<(WasmPluginMeta, Arc<dyn ScrapingService>)> {
            let (meta, module) = self.compile_module(path).await?;

            // Cache the compiled module
            self.cache
                .write()
                .await
                .insert(meta.name.clone(), (meta.clone(), module.clone()));

            let svc: Arc<dyn ScrapingService> = Arc::new(WasmScrapingService {
                name: Box::leak(meta.name.clone().into_boxed_str()),
                engine: self.engine.clone(),
                module,
            });

            Ok((meta, svc))
        }

        async fn loaded(&self) -> Result<Vec<WasmPluginMeta>> {
            let guard = self.cache.read().await;
            Ok(guard.values().map(|(m, _)| m.clone()).collect())
        }
    }

    /// Wraps a compiled WASM module as a [`ScrapingService`].
    ///
    /// The module must export:
    /// - `alloc(size: i32) → i32` — guest allocator
    /// - `dealloc(ptr: i32, size: i32)` — guest deallocator
    /// - `plugin_execute(in_ptr: i32, in_len: i32, out_ptr_ptr: i32) → i32`
    ///   — execute and write output, returning output length
    struct WasmScrapingService {
        name: &'static str,
        engine: Engine,
        module: Module,
    }

    #[async_trait::async_trait]
    impl ScrapingService for WasmScrapingService {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
            let engine = self.engine.clone();
            let module = self.module.clone();

            // WASM execution is CPU-bound — run on a blocking thread
            let result =
                tokio::task::spawn_blocking(move || execute_wasm_sync(&engine, &module, &input))
                    .await
                    .map_err(|e| {
                        StygianError::Service(ServiceError::InvalidResponse(format!(
                            "WASM task panicked: {e}"
                        )))
                    })??;

            Ok(ServiceOutput {
                data: result.to_string(),
                metadata: serde_json::Value::default(),
            })
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute_wasm_sync(
        engine: &Engine,
        module: &Module,
        input: &ServiceInput,
    ) -> Result<serde_json::Value> {
        let mut store = Store::new(engine, HostState);
        let linker: Linker<HostState> = Linker::new(engine);

        let instance = linker.instantiate(&mut store, module).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM instantiation failed: {e}"
            )))
        })?;

        // Serialise input to JSON bytes
        let input_json = serde_json::to_vec(&serde_json::json!({
            "url": &input.url,
            "params": &input.params,
        }))
        .map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "failed to serialise WASM input: {e}"
            )))
        })?;

        // Locate exported functions
        let alloc: wasmtime::TypedFunc<i32, i32> =
            instance.get_typed_func(&mut store, "alloc").map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "WASM missing `alloc` export: {e}"
                )))
            })?;

        let execute: wasmtime::TypedFunc<(i32, i32, i32), i32> = instance
            .get_typed_func(&mut store, "plugin_execute")
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "WASM missing `plugin_execute` export: {e}"
                )))
            })?;

        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| {
            StygianError::Service(ServiceError::InvalidResponse(
                "WASM module has no exported `memory`".to_string(),
            ))
        })?;

        // Allocate input buffer in guest memory
        let in_len = i32::try_from(input_json.len()).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM input too large: {e}"
            )))
        })?;
        let in_ptr = alloc.call(&mut store, in_len).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM alloc failed: {e}"
            )))
        })?;

        // Write input bytes into guest memory
        let in_ptr_usize = usize::try_from(in_ptr).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM invalid input pointer: {e}"
            )))
        })?;
        memory
            .write(&mut store, in_ptr_usize, &input_json)
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "WASM memory write failed: {e}"
                )))
            })?;

        // Allocate a 4-byte slot for the output pointer (i32)
        let out_ptr_slot = alloc.call(&mut store, 4).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM alloc (out_ptr) failed: {e}"
            )))
        })?;

        // Call plugin_execute
        let out_len = execute
            .call(&mut store, (in_ptr, in_len, out_ptr_slot))
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "WASM plugin_execute failed: {e}"
                )))
            })?;

        if out_len < 0 {
            return Err(StygianError::Service(ServiceError::InvalidResponse(
                format!("WASM plugin_execute returned error code {out_len}"),
            )));
        }

        // Read the output pointer value from the slot
        let mut ptr_bytes = [0u8; 4];
        let out_ptr_slot_usize = usize::try_from(out_ptr_slot).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM invalid output pointer slot: {e}"
            )))
        })?;
        memory
            .read(&store, out_ptr_slot_usize, &mut ptr_bytes)
            .map_err(|e| {
                StygianError::Service(ServiceError::InvalidResponse(format!(
                    "WASM output ptr read failed: {e}"
                )))
            })?;
        let out_ptr = usize::try_from(i32::from_le_bytes(ptr_bytes)).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM invalid output pointer: {e}"
            )))
        })?;

        // Read the output bytes
        let out_len_usize = usize::try_from(out_len).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM invalid output length: {e}"
            )))
        })?;
        let mut out_bytes = vec![0u8; out_len_usize];
        memory.read(&store, out_ptr, &mut out_bytes).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM output read failed: {e}"
            )))
        })?;

        let value: serde_json::Value = serde_json::from_slice(&out_bytes).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "WASM output deserialisation failed: {e}"
            )))
        })?;

        Ok(value)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::MockWasmPlugin;
    use crate::ports::ServiceInput;
    use crate::ports::wasm_plugin::WasmPluginPort;
    use std::path::PathBuf;

    #[tokio::test]
    async fn mock_loader_discover_returns_one_plugin() {
        let loader = MockWasmPlugin::new();
        let plugins = loader.discover().await.unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].0.name, "mock-wasm-plugin");
    }

    #[tokio::test]
    async fn mock_loader_loaded_returns_meta() {
        let loader = MockWasmPlugin::new();
        let metas = loader.loaded().await.unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].version, "0.1.0");
    }

    #[tokio::test]
    async fn mock_loader_load_by_path() {
        let loader = MockWasmPlugin::new();
        let (meta, svc) = loader
            .load(&PathBuf::from("plugins/example.wasm"))
            .await
            .unwrap();
        assert_eq!(meta.name, "example");
        let input = ServiceInput {
            url: "https://example.com".to_string(),
            params: serde_json::Value::Null,
        };
        let output = svc.execute(input).await.unwrap();
        assert!(output.data.contains("example"));
        assert!(output.data.contains("mock"));
    }

    #[tokio::test]
    async fn mock_service_returns_plugin_name_in_output() {
        let loader = MockWasmPlugin::new();
        let (_, svc) = loader.discover().await.unwrap().remove(0);
        let input = ServiceInput {
            url: "https://test.com".to_string(),
            params: serde_json::Value::Null,
        };
        let out = svc.execute(input).await.unwrap();
        assert!(out.data.contains("mock-wasm-plugin"));
        assert!(out.data.contains("https://test.com"));
    }

    #[test]
    fn mock_plugin_default_dir_is_plugins() {
        let loader = MockWasmPlugin::default();
        assert_eq!(loader.plugin_dir, PathBuf::from("plugins"));
    }

    #[tokio::test]
    async fn mock_service_execute_with_json_params() {
        let loader = MockWasmPlugin::new();
        let (_, svc) = loader.discover().await.unwrap().remove(0);
        let input = ServiceInput {
            url: "https://api.example.com/data".to_string(),
            params: serde_json::json!({"key": "value"}),
        };
        let out = svc.execute(input).await.unwrap();
        // Mock service always succeeds regardless of params
        assert!(!out.data.is_empty());
        assert!(out.data.contains("https://api.example.com/data"));
    }

    #[tokio::test]
    async fn mock_loader_load_uses_file_stem_as_name() {
        let loader = MockWasmPlugin::new();
        let (meta, _) = loader
            .load(&PathBuf::from("plugins/my-extractor.wasm"))
            .await
            .unwrap();
        assert_eq!(meta.name, "my-extractor");
    }
}
