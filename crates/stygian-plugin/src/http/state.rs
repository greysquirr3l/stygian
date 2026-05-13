//! Shared server state injected into axum handlers.

use crate::config::Config;
use crate::error::{PluginError, Result as PluginResult};
use crate::mcp::{McpPluginServer, McpRequestHandler};
use std::sync::Arc;

/// Axum application state — cloned cheaply via `Arc` on every request.
#[derive(Clone)]
pub struct AppState {
    /// The JSON-RPC dispatcher (wraps `McpPluginServer`).
    pub handler: Arc<McpRequestHandler>,
}

impl AppState {
    /// Build application state from config, constructing the full handler chain.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] if the template store cannot be initialized.
    pub fn new(config: Config) -> PluginResult<Self> {
        // Validate templates dir path up-front to surface bad config early
        if let Some(parent) = config.templates_dir.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                PluginError::Other(format!(
                    "cannot create templates directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let server = Arc::new(McpPluginServer::new_with_file_storage(
            config.templates_dir.clone(),
        ));
        let handler = Arc::new(McpRequestHandler::new(Arc::clone(&server), config));
        Ok(Self { handler })
    }
}
