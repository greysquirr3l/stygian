//! MCP (Model Context Protocol) server for browser automation.
//!
//! Exposes `stygian-browser` capabilities as an MCP server over stdin/stdout
//! using the JSON-RPC 2.0 protocol.  External tools (LLM agents, IDE plugins)
//! can acquire browsers, navigate pages, evaluate JavaScript, and capture
//! screenshots via the standardised MCP interface.
//!
//! ## Enabling
//!
//! ```toml
//! [dependencies]
//! stygian-browser = { version = "0.1", features = ["mcp"] }
//! ```
//!
//! ## Running the server
//!
//! ```sh
//! STYGIAN_MCP_ENABLED=true cargo run --example mcp_server -p stygian-browser
//! ```
//!
//! ## Protocol
//!
//! The server implements MCP 2024-11-05 over JSON-RPC 2.0 on stdin/stdout.
//! Supported methods:
//!
//! | MCP Method | Description |
//! | ----------- | ------------- |
//! | `initialize` | Handshake, return server capabilities |
//! | `tools/list` | List available browser tools |
//! | `tools/call` | Execute a browser tool |
//! | `resources/list` | List active browser sessions as MCP resources |
//! | `resources/read` | Read session state |
//!
//! ## Tools
//!
//! | Tool | Parameters | Returns |
//! | ------ | ----------- | --------- |
//! | `browser_acquire` | `stealth_level?`, `tls_profile?`, `webrtc_policy?`, `cdp_fix_mode?`, `proxy?` | `session_id`, `config` |
//! | `browser_navigate` | `session_id, url, timeout_secs?` | `title, url` |
//! | `browser_eval` | `session_id, script` | `result: Value` |
//! | `browser_screenshot` | `session_id` | `data: base64 PNG` |
//! | `browser_content` | `session_id` | `html: String` |
//! | `browser_verify_stealth` | `session_id, url, timeout_secs?` | `DiagnosticReport` JSON |
//! | `browser_release` | `session_id` | success |
//! | `pool_stats` | – | `active, max, available` |

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};
use tracing::{debug, info};
use ulid::Ulid;

use crate::{
    BrowserHandle, BrowserPool,
    config::StealthLevel,
    error::{BrowserError, Result},
    page::WaitUntil,
};

// ─── JSON-RPC types ──────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version — always `"2.0"`.
    pub jsonrpc: String,
    /// Method name (e.g. `"tools/call"`).
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
    /// Request ID. `null` for notifications.
    #[serde(default)]
    pub id: Value,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Value,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    const fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
            id,
        }
    }

    fn method_not_found(id: Value, method: &str) -> Self {
        Self::err(id, -32601, format!("Method not found: {method}"))
    }
}

// ─── Session state ────────────────────────────────────────────────────────────

/// An active MCP browser session.
///
/// The handle is wrapped in an `Arc<Mutex<Option<_>>>` so callers can clone
/// the `Arc` and release the sessions map lock before performing long browser
/// I/O operations.
struct McpSession {
    /// Pool handle for this session — `None` after [`tool_browser_release`].
    handle: Arc<Mutex<Option<BrowserHandle>>>,
    /// Requested stealth level for this session.
    stealth_level: StealthLevel,
    /// Requested TLS profile name (informational — takes effect at browser launch).
    tls_profile: Option<String>,
    /// Requested WebRTC policy name (informational — takes effect at browser launch).
    webrtc_policy: Option<String>,
    /// Requested CDP fix mode for this session.
    cdp_fix_mode: Option<String>,
    /// Proxy URL for this session (informational — takes effect at browser launch).
    proxy: Option<String>,
}

// ─── MCP server ──────────────────────────────────────────────────────────────

/// MCP server that exposes `BrowserPool` over stdin/stdout JSON-RPC.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::{BrowserConfig, BrowserPool};
/// use stygian_browser::mcp::McpBrowserServer;
/// use std::sync::Arc;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let server = McpBrowserServer::new(pool);
/// server.run().await?;
/// # Ok(())
/// # }
/// ```
static TOOL_DEFINITIONS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    vec![
        json!({
            "name": "browser_acquire",
            "description": "Acquire a browser from the pool. Returns a session_id and the effective session config. Optional parameters set per-session preferences; browser-launch-level params (tls_profile, webrtc_policy, proxy) take effect only if browsers are launched with that config.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "stealth_level": {
                        "type": "string",
                        "enum": ["none", "basic", "advanced"],
                        "description": "Anti-detection intensity. Defaults to 'advanced'."
                    },
                    "tls_profile": {
                        "type": "string",
                        "enum": ["chrome131", "firefox133", "safari18", "edge131"],
                        "description": "TLS fingerprint profile (requires stealth feature; browser-launch-level)."
                    },
                    "webrtc_policy": {
                        "type": "string",
                        "enum": ["allow_all", "disable_non_proxied", "block_all"],
                        "description": "WebRTC IP-leak policy (requires stealth feature; browser-launch-level)."
                    },
                    "cdp_fix_mode": {
                        "type": "string",
                        "enum": ["addBinding", "isolatedWorld", "enableDisable", "none"],
                        "description": "CDP Runtime.enable leak-mitigation mode."
                    },
                    "proxy": {
                        "type": "string",
                        "description": "HTTP/SOCKS proxy URL, e.g. 'http://user:pass@host:port' (browser-launch-level)."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "browser_navigate",
            "description": "Navigate to a URL within a session. Opens a new page if needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "url": { "type": "string" },
                    "timeout_secs": { "type": "number", "default": 30 }
                },
                "required": ["session_id", "url"]
            }
        }),
        json!({
            "name": "browser_eval",
            "description": "Evaluate JavaScript in the current page of a session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "script": { "type": "string" }
                },
                "required": ["session_id", "script"]
            }
        }),
        json!({
            "name": "browser_screenshot",
            "description": "Capture a full-page PNG screenshot. Returns base64-encoded PNG.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "browser_content",
            "description": "Get the full HTML content of the current page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "browser_verify_stealth",
            "description": "Navigate to a URL and run all built-in stealth detection checks (requires stealth feature). Returns a DiagnosticReport with per-check pass/fail results and a coverage percentage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "url": { "type": "string", "description": "URL to navigate to before running checks." },
                    "timeout_secs": { "type": "number", "default": 15, "description": "Navigation timeout in seconds." }
                },
                "required": ["session_id", "url"]
            }
        }),
        json!({
            "name": "browser_release",
            "description": "Release a browser session back to the pool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "pool_stats",
            "description": "Return current browser pool statistics.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
});

pub struct McpBrowserServer {
    pool: Arc<BrowserPool>,
    sessions: Arc<Mutex<HashMap<String, McpSession>>>,
}

impl McpBrowserServer {
    /// Create a new server backed by the given `pool`.
    ///
    /// Call [`run`](Self::run) to start the stdin/stdout event loop.
    pub fn new(pool: Arc<BrowserPool>) -> Self {
        Self {
            pool,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Run the JSON-RPC event loop.
    ///
    /// Reads newline-delimited JSON from stdin and writes responses to stdout.
    /// Runs until stdin is closed (EOF).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if stdin/stdout cannot be read from or written to.
    pub async fn run(&self) -> Result<()> {
        info!("MCP browser server starting (stdin/stdout mode)");

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();
        let mut stdout = stdout;

        while let Some(line) = reader.next_line().await.map_err(BrowserError::Io)? {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            debug!(?line, "MCP request");

            let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(req) => self.handle_request(req).await,
                Err(e) => JsonRpcResponse::err(Value::Null, -32700, format!("Parse error: {e}")),
            };

            let mut out = serde_json::to_string(&response).unwrap_or_default();
            out.push('\n');
            stdout
                .write_all(out.as_bytes())
                .await
                .map_err(BrowserError::Io)?;
            stdout.flush().await.map_err(BrowserError::Io)?;
        }

        info!("MCP browser server stopping (stdin closed)");
        Ok(())
    }

    /// Dispatch a single raw JSON-RPC request value.
    ///
    /// Used by the `stygian-mcp` aggregator to route tool calls through this
    /// server without running the full stdin/stdout loop.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserConfig, BrowserPool};
    /// use stygian_browser::mcp::McpBrowserServer;
    /// use std::sync::Arc;
    /// use serde_json::json;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let server = McpBrowserServer::new(pool);
    /// let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    /// let resp = server.dispatch(&req).await;
    /// assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch(&self, req: &Value) -> Value {
        let typed: JsonRpcRequest = match serde_json::from_value(req.clone()) {
            Ok(r) => r,
            Err(e) => {
                return json!({
                    "jsonrpc": "2.0",
                    "id": req.get("id").cloned().unwrap_or(Value::Null),
                    "error": { "code": -32700, "message": format!("Parse error: {e}") }
                });
            }
        };
        let resp = self.handle_request(typed).await;
        serde_json::to_value(resp).unwrap_or_else(|_| json!({"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}}))
    }

    async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();
        match req.method.as_str() {
            "initialize" => Self::handle_initialize(id),
            "tools/list" => Self::handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            "resources/list" => self.handle_resources_list(id).await,
            "resources/read" => self.handle_resources_read(id, req.params).await,
            "notifications/initialized" | "ping" => {
                // Notifications — no response needed; return a no-op result.
                JsonRpcResponse::ok(id, json!({}))
            }
            other => JsonRpcResponse::method_not_found(id, other),
        }
    }

    // ── MCP lifecycle ──────────────────────────────────────────────────────────

    fn handle_initialize(id: Value) -> JsonRpcResponse {
        JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "listChanged": false, "subscribe": false }
                },
                "serverInfo": {
                    "name": "stygian-browser",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    // ── tools/list ────────────────────────────────────────────────────────────

    fn handle_tools_list(id: Value) -> JsonRpcResponse {
        JsonRpcResponse::ok(id, json!({ "tools": &*TOOL_DEFINITIONS }))
    }

    // ── tools/call ────────────────────────────────────────────────────────────

    async fn handle_tools_call(&self, id: Value, params: Value) -> JsonRpcResponse {
        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return JsonRpcResponse::err(id, -32602, "Missing tool 'name'"),
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = match name.as_str() {
            "browser_acquire" => self.tool_browser_acquire(&args).await,
            "browser_navigate" => self.tool_browser_navigate(&args).await,
            "browser_eval" => self.tool_browser_eval(&args).await,
            "browser_screenshot" => self.tool_browser_screenshot(&args).await,
            "browser_content" => self.tool_browser_content(&args).await,
            "browser_verify_stealth" => self.tool_browser_verify_stealth(&args).await,
            "browser_release" => self.tool_browser_release(&args).await,
            "pool_stats" => Ok(self.tool_pool_stats()),
            other => Err(BrowserError::ConfigError(format!("Unknown tool: {other}"))),
        };

        match result {
            Ok(content) => JsonRpcResponse::ok(
                id,
                json!({ "content": [{ "type": "text", "text": content.to_string() }], "isError": false }),
            ),
            Err(e) => JsonRpcResponse::ok(
                id,
                json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true }),
            ),
        }
    }

    async fn tool_browser_acquire(&self, args: &Value) -> Result<Value> {
        // Parse per-session config preferences.
        let stealth_level = args
            .get("stealth_level")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "none" => StealthLevel::None,
                "basic" => StealthLevel::Basic,
                _ => StealthLevel::Advanced,
            })
            .unwrap_or_default();
        let tls_profile = args
            .get("tls_profile")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let webrtc_policy = args
            .get("webrtc_policy")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let cdp_fix_mode = args
            .get("cdp_fix_mode")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let proxy = args
            .get("proxy")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        let handle = self.pool.acquire().await?;
        let session_id = Ulid::new().to_string();

        let effective_stealth = format!("{stealth_level:?}").to_lowercase();
        self.sessions.lock().await.insert(
            session_id.clone(),
            McpSession {
                handle: Arc::new(Mutex::new(Some(handle))),
                stealth_level,
                tls_profile: tls_profile.clone(),
                webrtc_policy: webrtc_policy.clone(),
                cdp_fix_mode: cdp_fix_mode.clone(),
                proxy: proxy.clone(),
            },
        );

        info!(%session_id, %effective_stealth, "MCP session acquired");
        Ok(json!({
            "session_id": session_id,
            "config": {
                "stealth_level": effective_stealth,
                "tls_profile": tls_profile,
                "webrtc_policy": webrtc_policy,
                "cdp_fix_mode": cdp_fix_mode,
                "proxy": proxy
            }
        }))
    }

    async fn tool_browser_verify_stealth(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(15.0);

        let (session_arc, requested_stealth) = self.session_handle_and_stealth(&session_id).await?;

        let mut page = session_arc
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Session already released: {session_id}"))
            })?
            .browser()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
            })?
            .new_page()
            .await?;

        page.navigate(
            &url,
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let mut result = Self::run_stealth_diagnostic(&page).await;
        page.close().await?;
        // Annotate with the session's requested stealth level.
        if let Ok(ref mut v) = result
            && let Some(obj) = v.as_object_mut()
        {
            obj.insert(
                "requested_stealth_level".to_string(),
                Value::String(requested_stealth),
            );
        }
        result
    }

    #[cfg(feature = "stealth")]
    async fn run_stealth_diagnostic(page: &crate::page::PageHandle) -> Result<Value> {
        let report = page.verify_stealth().await?;
        serde_json::to_value(&report)
            .map_err(|e| BrowserError::ConfigError(format!("failed to serialize report: {e}")))
    }

    #[cfg(not(feature = "stealth"))]
    async fn run_stealth_diagnostic(_page: &crate::page::PageHandle) -> Result<Value> {
        Err(BrowserError::ConfigError(
            "browser_verify_stealth requires the 'stealth' feature to be enabled".to_string(),
        ))
    }

    async fn tool_browser_navigate(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let session_arc = self.session_handle(&session_id).await?;

        let mut page = session_arc
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Session already released: {session_id}"))
            })?
            .browser()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
            })?
            .new_page()
            .await?;

        page.navigate(
            &url,
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let title = page.title().await.unwrap_or_default();
        let current_url = url.clone();
        page.close().await?;

        Ok(json!({ "title": title, "url": current_url }))
    }

    async fn tool_browser_eval(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let script = Self::require_str(args, "script")?;

        let session_arc = self.session_handle(&session_id).await?;

        let mut page = session_arc
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Session already released: {session_id}"))
            })?
            .browser()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
            })?
            .new_page()
            .await?;

        page.navigate(
            "about:blank",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(5),
        )
        .await?;

        let result: Value = page.eval(&script).await?;
        page.close().await?;

        Ok(json!({ "result": result }))
    }

    async fn tool_browser_screenshot(&self, args: &Value) -> Result<Value> {
        use base64::Engine as _;
        let session_id = Self::require_str(args, "session_id")?;

        let session_arc = self.session_handle(&session_id).await?;

        let mut page = session_arc
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Session already released: {session_id}"))
            })?
            .browser()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
            })?
            .new_page()
            .await?;

        page.navigate(
            "about:blank",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(5),
        )
        .await?;

        let png_bytes = page.screenshot().await?;
        page.close().await?;

        let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        Ok(json!({ "data": encoded, "mimeType": "image/png", "bytes": png_bytes.len() }))
    }

    async fn tool_browser_content(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;

        let session_arc = self.session_handle(&session_id).await?;

        let mut page = session_arc
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Session already released: {session_id}"))
            })?
            .browser()
            .ok_or_else(|| {
                BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
            })?
            .new_page()
            .await?;

        page.navigate(
            "about:blank",
            WaitUntil::Selector("body".to_string()),
            Duration::from_secs(5),
        )
        .await?;

        let html = page.content().await?;
        page.close().await?;

        Ok(json!({ "html": html, "bytes": html.len() }))
    }

    async fn tool_browser_release(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;

        // Remove Arc from the map — brief lock
        let session_arc = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .remove(&session_id)
                .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))?
                .handle
        };

        // Take and release the handle without holding the map lock
        let handle = session_arc.lock().await.take();
        if let Some(h) = handle {
            h.release().await;
        }

        info!(%session_id, "MCP session released");
        Ok(json!({ "released": true, "session_id": session_id }))
    }

    fn tool_pool_stats(&self) -> Value {
        let stats = self.pool.stats();
        json!({
            "active": stats.active,
            "max": stats.max,
            "available": stats.available
        })
    }

    // ── resources/list ────────────────────────────────────────────────────────

    async fn handle_resources_list(&self, id: Value) -> JsonRpcResponse {
        let resources: Vec<Value> = self
            .sessions
            .lock()
            .await
            .keys()
            .map(|sid| {
                json!({
                    "uri": format!("browser://session/{sid}"),
                    "name": format!("Browser session {sid}"),
                    "mimeType": "application/json"
                })
            })
            .collect();

        JsonRpcResponse::ok(id, json!({ "resources": resources }))
    }

    // ── resources/read ────────────────────────────────────────────────────────

    async fn handle_resources_read(&self, id: Value, params: Value) -> JsonRpcResponse {
        let uri = match params.get("uri").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => return JsonRpcResponse::err(id, -32602, "Missing 'uri'"),
        };

        // Parse browser://session/<session_id>
        let session_id = uri
            .strip_prefix("browser://session/")
            .unwrap_or("")
            .to_string();

        // Read session config while holding the map lock, then release.
        let session_config: Option<Value> = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).map(|s| {
                json!({
                    "stealth_level": format!("{:?}", s.stealth_level).to_lowercase(),
                    "tls_profile": s.tls_profile,
                    "webrtc_policy": s.webrtc_policy,
                    "cdp_fix_mode": s.cdp_fix_mode,
                    "proxy": s.proxy
                })
            })
        };

        if let Some(config) = session_config {
            let pool_stats = self.pool.stats();
            JsonRpcResponse::ok(
                id,
                json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&json!({
                            "session_id": session_id,
                            "config": config,
                            "pool_active": pool_stats.active,
                            "pool_max": pool_stats.max
                        })).unwrap_or_default()
                    }]
                }),
            )
        } else {
            JsonRpcResponse::err(id, -32002, format!("Resource not found: {uri}"))
        }
    }

    // ── Helper ────────────────────────────────────────────────────────────────

    async fn session_handle(&self, session_id: &str) -> Result<Arc<Mutex<Option<BrowserHandle>>>> {
        Ok(self
            .sessions
            .lock()
            .await
            .get(session_id)
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))?
            .handle
            .clone())
    }

    async fn session_handle_and_stealth(
        &self,
        session_id: &str,
    ) -> Result<(Arc<Mutex<Option<BrowserHandle>>>, String)> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|s| {
                (
                    s.handle.clone(),
                    format!("{:?}", s.stealth_level).to_lowercase(),
                )
            })
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))
    }

    fn require_str(args: &Value, key: &str) -> Result<String> {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| BrowserError::ConfigError(format!("Missing required argument: {key}")))
    }
}

/// Returns `true` if `value` is a truthy string (`"true"`, `"1"`, or `"yes"`,
/// case-insensitive).
fn mcp_enabled_from(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "true" | "1" | "yes")
}

/// Returns `true` if the MCP server is enabled via the `STYGIAN_MCP_ENABLED`
/// environment variable.
///
/// Set `STYGIAN_MCP_ENABLED=true` to enable the server.
pub fn is_mcp_enabled() -> bool {
    mcp_enabled_from(&std::env::var("STYGIAN_MCP_ENABLED").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_response_ok_serializes() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let r = JsonRpcResponse::ok(json!(1), json!({ "hello": "world" }));
        let s = serde_json::to_string(&r)?;
        assert!(s.contains("\"hello\""));
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(!s.contains("\"error\""));
        Ok(())
    }

    #[test]
    fn jsonrpc_response_err_serializes() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let r = JsonRpcResponse::err(json!(2), -32601, "Method not found");
        let s = serde_json::to_string(&r)?;
        assert!(s.contains("-32601"));
        assert!(s.contains("Method not found"));
        assert!(!s.contains("\"result\""));
        Ok(())
    }

    #[test]
    fn mcp_env_disabled_by_default() {
        // If STYGIAN_MCP_ENABLED is not "true"/"1"/"yes", function returns false
        let cases = ["false", "0", "no", "", "off"];
        for val in cases {
            assert!(!mcp_enabled_from(val), "expected disabled for {val:?}");
        }
    }

    #[test]
    fn mcp_env_enabled_values() {
        let cases = ["true", "True", "TRUE", "1", "yes", "YES"];
        for val in cases {
            assert!(mcp_enabled_from(val), "expected enabled for {val:?}");
        }
    }
}
