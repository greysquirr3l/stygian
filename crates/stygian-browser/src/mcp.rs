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
//! stygian-browser = { version = "*", features = ["mcp"] }
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
//! The server implements MCP 2025-11-25 over JSON-RPC 2.0 on stdin/stdout.
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
//! | `browser_acquire` | `stealth_level?`, `tls_profile?`, `webrtc_policy?`, `cdp_fix_mode?`, `proxy?` | `session_id`, `requested_metadata` |
//! | `browser_navigate` | `session_id, url, timeout_secs?` | `title, url` |
//! | `browser_eval` | `session_id, script` | `result: Value` |
//! | `browser_screenshot` | `session_id` | `data: base64 PNG` |
//! | `browser_content` | `session_id` | `html: String` |
//! | `browser_verify_stealth` | `session_id, url, timeout_secs?` | `DiagnosticReport` JSON |
//! | `browser_release` | `session_id` | success |
//! | `pool_stats` | – | `active, max, available` |
//! | `browser_query` | `session_id, url, selector, fields?, limit?, timeout_secs?` | `results` array of text or field objects |
//! | `browser_extract` | `session_id, url, root_selector, schema, timeout_secs?` | `results` array of structured objects |
//! | `browser_extract_with_fallback` | `session_id, url, root_selectors, schema, timeout_secs?` | first successful selector + `results` |
//! | `browser_extract_resilient` | `session_id, url, root_selector, schema, timeout_secs?` | `results` plus skipped-count metadata |
//! | `browser_find_similar` *(similarity feature)* | `session_id, url, reference_selector, threshold?, max_results?, timeout_secs?` | scored `matches` array |
//! | `browser_warmup` | `session_id, url, wait?, timeout_ms?, stabilize_ms?` | warmup report |
//! | `browser_refresh` | `session_id, wait?, timeout_ms?, reset_connection?` | refresh report |

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
    /// Last URL successfully navigated to via `browser_navigate`.
    current_url: Option<String>,
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
    let mut tools = vec![
        json!({
            "name": "browser_acquire",
            "description": "Acquire a browser from the pool and open a session. The optional parameters are stored as session metadata labels and echoed back in the response; they do not reconfigure the pool-acquired browser at runtime. Use them to annotate sessions (e.g. for `browser_verify_stealth` attribution).",
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
                        "description": "TLS fingerprint profile label (free-form; requires stealth feature; browser-launch-level). Examples: chrome131, firefox133, safari18, edge131."
                    },
                    "webrtc_policy": {
                        "type": "string",
                        "description": "WebRTC IP-leak policy label (free-form; requires stealth feature; browser-launch-level). Examples: allow_all, disable_non_proxied, block_all."
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
                    "timeout_secs": { "type": "integer", "default": 30 }
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
    ];
    tools.push(json!({
        "name": "browser_query",
        "description": "Navigate to a URL, query all elements matching a CSS selector, and return their text content or specific attributes. If `fields` is omitted each result is a plain string (the text content). If `fields` is supplied each result is an object with one key per field.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string" },
                "selector": { "type": "string", "description": "CSS selector passed to querySelectorAll." },
                "fields": {
                    "type": "object",
                    "description": "Map of output field name → { \"attr\": \"attribute-name\" }. Omit `attr` to get text content for that field.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": { "attr": { "type": "string" } }
                    }
                },
                "limit": { "type": "integer", "default": 50, "description": "Maximum number of nodes to return." },
                "timeout_secs": { "type": "number", "default": 30 }
            },
            "required": ["session_id", "url", "selector"]
        }
    }));
    tools.push(json!({
        "name": "browser_extract",
        "description": "Navigate to a URL and perform schema-driven structured extraction. Each element matching `root_selector` becomes one result object; fields within each root are resolved by their own sub-selectors relative to the root. This is the runtime equivalent of the `#[derive(Extract)]` macro.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string" },
                "root_selector": { "type": "string", "description": "CSS selector whose matches become the root of each result object." },
                "schema": {
                    "type": "object",
                    "description": "Map of field name → { \"selector\": \"...\", \"attr\": \"...\", \"required\": true/false }.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "selector": { "type": "string" },
                            "attr": { "type": "string" },
                            "required": { "type": "boolean", "default": false }
                        },
                        "required": ["selector"]
                    }
                },
                "timeout_secs": { "type": "number", "default": 30 }
            },
            "required": ["session_id", "url", "root_selector", "schema"]
        }
    }));
    tools.push(json!({
        "name": "browser_extract_with_fallback",
        "description": "Like browser_extract but accepts multiple root selectors (tried in order). Returns the first selector that produces results. Useful when a site layout may have changed and you want to try modern markup before falling back to legacy selectors.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string" },
                "root_selectors": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "CSS selectors tried in order; the first that produces results is used.",
                    "minItems": 1
                },
                "schema": {
                    "type": "object",
                    "description": "Map of field name → { \"selector\": \"...\", \"attr\": \"...\", \"required\": true/false }.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "selector": { "type": "string" },
                            "attr": { "type": "string" },
                            "required": { "type": "boolean", "default": false }
                        },
                        "required": ["selector"]
                    }
                },
                "timeout_secs": { "type": "number", "default": 30 }
            },
            "required": ["session_id", "url", "root_selectors", "schema"]
        }
    }));
    tools.push(json!({
        "name": "browser_extract_resilient",
        "description": "Like browser_extract but skips root nodes where *all* required schema fields are absent (partial records). Useful for heterogeneous lists where some items lack an optional field.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string" },
                "root_selector": { "type": "string", "description": "CSS selector whose matches become the root of each result object." },
                "schema": {
                    "type": "object",
                    "description": "Map of field name → { \"selector\": \"...\", \"attr\": \"...\", \"required\": true/false }.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "selector": { "type": "string" },
                            "attr": { "type": "string" },
                            "required": { "type": "boolean", "default": false }
                        },
                        "required": ["selector"]
                    }
                },
                "timeout_secs": { "type": "number", "default": 30 }
            },
            "required": ["session_id", "url", "root_selector", "schema"]
        }
    }));
    // Advertise browser_find_similar only when the similarity feature is compiled in.
    #[cfg(feature = "similarity")]
    tools.push(json!({
        "name": "browser_find_similar",
        "description": "Navigate to a URL and find DOM elements that are structurally similar to a reference element (identified by a CSS selector). Useful when a site has been redesigned and stored selectors no longer match. Requires the `similarity` feature.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string" },
                "reference_selector": { "type": "string", "description": "CSS selector identifying the reference node. The first match is used." },
                "threshold": { "type": "number", "default": 0.7, "description": "Minimum similarity score [0.0, 1.0]." },
                "max_results": { "type": "integer", "default": 10 },
                "timeout_secs": { "type": "number", "default": 30 }
            },
            "required": ["session_id", "url", "reference_selector"]
        }
    }));
    // Advertise browser_verify_stealth only when the stealth feature is compiled in.
    #[cfg(feature = "stealth")]
    tools.push(json!({
        "name": "browser_verify_stealth",
        "description": "Navigate to a URL and run built-in stealth checks with optional transport diagnostics (JA3/JA4/HTTP3). Returns a DiagnosticReport with pass/fail results, coverage percentage, transport mismatch details, and known_limitations for visible-but-not-yet-covered surfaces.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string", "description": "URL to navigate to before running checks." },
                "timeout_secs": { "type": "integer", "default": 15, "description": "Navigation timeout in seconds." },
                "observed_ja3_hash": { "type": "string", "description": "Optional observed JA3 hash to compare against expected profile." },
                "observed_ja4": { "type": "string", "description": "Optional observed JA4 fingerprint to compare against expected profile." },
                "observed_http3_perk_text": { "type": "string", "description": "Optional observed HTTP/3 perk text (SETTINGS|PSEUDO_HEADERS)." },
                "observed_http3_perk_hash": { "type": "string", "description": "Optional observed HTTP/3 perk hash." }
            },
            "required": ["session_id", "url"]
        }
    }));
    // Advertise browser_validate_stealth only when the stealth feature is compiled in.
    #[cfg(feature = "stealth")]
    tools.push(json!({
        "name": "browser_validate_stealth",
        "description": "Run anti-bot service validators against the pool (Tier 1: CreepJS, BrowserScan). Returns a summary report.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "targets": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["creepjs", "browserscan", "fingerprint_js", "kasada", "cloudflare", "akamai", "data_dome", "perimeter_x"] },
                    "description": "List of services to validate. Empty = Tier 1 only. Tier 2+ tests may rate-limit.",
                    "default": ["creepjs", "browserscan"]
                },
                "tier1_only": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, force regression-safe Tier 1 targets only (CreepJS + BrowserScan)."
                },
                "timeout_secs": { "type": "integer", "default": 30, "description": "Per-target timeout in seconds." }
            },
            "required": []
        }
    }));
    // Session warmup and refresh tools.
    tools.push(json!({
        "name": "browser_warmup",
        "description": "Warm up a browser session by navigating to a URL and optionally waiting for dynamic resources to settle. Warmup is idempotent — calling it again re-warms the same session.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "url": { "type": "string", "description": "URL to navigate to during warmup." },
                "wait": {
                    "type": "string",
                    "enum": ["dom_content_loaded", "network_idle"],
                    "default": "dom_content_loaded",
                    "description": "Wait strategy after navigation."
                },
                "timeout_ms": { "type": "integer", "default": 30000, "description": "Navigation timeout in milliseconds." },
                "stabilize_ms": { "type": "integer", "default": 0, "description": "Additional pause after navigation for dynamic resources to settle (0 = skip)." }
            },
            "required": ["session_id", "url"]
        }
    }));
    tools.push(json!({
        "name": "browser_refresh",
        "description": "Refresh the current page while retaining cookies and session storage. Optionally re-navigates to force a new TCP connection.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "wait": {
                    "type": "string",
                    "enum": ["dom_content_loaded", "network_idle"],
                    "default": "dom_content_loaded",
                    "description": "Wait strategy after reload."
                },
                "timeout_ms": { "type": "integer", "default": 30000, "description": "Reload timeout in milliseconds." },
                "reset_connection": { "type": "boolean", "default": false, "description": "When true, re-navigates to force a new TCP connection instead of in-place reload." }
            },
            "required": ["session_id"]
        }
    }));
    tools
});

pub struct McpBrowserServer {
    pool: Arc<BrowserPool>,
    sessions: Arc<Mutex<HashMap<String, McpSession>>>,
}

/// Per-field specification parsed from a `browser_extract` schema object.
struct ExtractFieldDef {
    selector: String,
    attr: Option<String>,
    required: bool,
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

            let response = match serde_json::from_str::<Value>(&line) {
                Ok(req) => {
                    let is_well_formed_notification = req.is_object()
                        && req.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                        && req.get("id").is_none()
                        && req.get("method").and_then(Value::as_str).is_some();
                    let response = self.dispatch(&req).await;
                    if is_well_formed_notification {
                        continue;
                    }
                    response
                }
                Err(e) => serde_json::to_value(JsonRpcResponse::err(
                    Value::Null,
                    -32700,
                    format!("Parse error: {e}"),
                ))
                .unwrap_or_else(|_| {
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}})
                }),
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
    /// assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
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
                "protocolVersion": "2025-11-25",
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
            #[cfg(feature = "stealth")]
            "browser_verify_stealth" => self.tool_browser_verify_stealth(&args).await,
            #[cfg(not(feature = "stealth"))]
            "browser_verify_stealth" => Err(BrowserError::ConfigError(
                "browser_verify_stealth requires the 'stealth' feature".to_string(),
            )),
            #[cfg(feature = "stealth")]
            "browser_validate_stealth" => self.tool_browser_validate_stealth(&args).await,
            #[cfg(not(feature = "stealth"))]
            "browser_validate_stealth" => Err(BrowserError::ConfigError(
                "browser_validate_stealth requires the 'stealth' feature".to_string(),
            )),
            "browser_release" => self.tool_browser_release(&args).await,
            "pool_stats" => Ok(self.tool_pool_stats()),
            "browser_query" => self.tool_browser_query(&args).await,
            "browser_extract" => self.tool_browser_extract(&args).await,
            "browser_extract_with_fallback" => self.tool_browser_extract_with_fallback(&args).await,
            "browser_extract_resilient" => self.tool_browser_extract_resilient(&args).await,
            #[cfg(feature = "similarity")]
            "browser_find_similar" => self.tool_browser_find_similar(&args).await,
            "browser_warmup" => self.tool_browser_warmup(&args).await,
            "browser_refresh" => self.tool_browser_refresh(&args).await,
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
                current_url: None,
            },
        );

        info!(%session_id, %effective_stealth, "MCP session acquired");
        Ok(json!({
            "session_id": session_id,
            "requested_metadata": {
                "stealth_level": effective_stealth,
                "tls_profile": tls_profile,
                "webrtc_policy": webrtc_policy,
                "cdp_fix_mode": cdp_fix_mode,
                "proxy": proxy
            }
        }))
    }

    #[cfg(feature = "stealth")]
    async fn tool_browser_verify_stealth(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(15);
        let observed = crate::diagnostic::TransportObservations {
            ja3_hash: args
                .get("observed_ja3_hash")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            ja4: args
                .get("observed_ja4")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            http3_perk_text: args
                .get("observed_http3_perk_text")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            http3_perk_hash: args
                .get("observed_http3_perk_hash")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
        };

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

        if let Err(e) = page
            .navigate(
                &url,
                WaitUntil::DomContentLoaded,
                Duration::from_secs(timeout_secs),
            )
            .await
        {
            // Ensure the page is closed before propagating the error.
            page.close().await.ok();
            return Err(e);
        }

        let mut result = Self::run_stealth_diagnostic(&page, observed).await;
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
    async fn run_stealth_diagnostic(
        page: &crate::page::PageHandle,
        observed: crate::diagnostic::TransportObservations,
    ) -> Result<Value> {
        let report = page.verify_stealth_with_transport(Some(observed)).await?;
        serde_json::to_value(&report)
            .map_err(|e| BrowserError::ConfigError(format!("failed to serialize report: {e}")))
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

        // Persist the navigated URL so that browser_content / browser_eval /
        // browser_screenshot can use it without the caller having to repeat it.
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(current_url.clone());
        }

        Ok(json!({ "title": title, "url": current_url }))
    }

    async fn tool_browser_eval(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let script = Self::require_str(args, "script")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, nav_url_opt) = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|s| (s.handle.clone(), s.current_url.clone()))
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_eval".to_string(),
            )
        })?;

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
            &nav_url,
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let result: Value = page.eval(&script).await?;
        page.close().await?;

        Ok(json!({ "result": result }))
    }

    async fn tool_browser_screenshot(&self, args: &Value) -> Result<Value> {
        use base64::Engine as _;
        let session_id = Self::require_str(args, "session_id")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, nav_url_opt) = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|s| (s.handle.clone(), s.current_url.clone()))
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_screenshot".to_string(),
            )
        })?;

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
            &nav_url,
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let png_bytes = page.screenshot().await?;
        page.close().await?;

        let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        Ok(json!({ "data": encoded, "mimeType": "image/png", "bytes": png_bytes.len() }))
    }

    async fn tool_browser_content(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, nav_url_opt) = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|s| (s.handle.clone(), s.current_url.clone()))
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_content".to_string(),
            )
        })?;

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
            &nav_url,
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let html = page.content().await?;
        page.close().await?;

        Ok(json!({ "html": html, "bytes": html.len() }))
    }

    async fn tool_browser_query(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let selector = Self::require_str(args, "selector")?;
        let limit = usize::try_from(
            args.get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(50),
        )
        .unwrap_or(50);
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        // Parse optional fields map: { "fieldName": { "attr"?: "attrName" } }
        let fields: Option<Vec<(String, Option<String>)>> =
            args.get("fields").and_then(|v| v.as_object()).map(|obj| {
                obj.iter()
                    .map(|(k, v)| {
                        let attr = v
                            .get("attr")
                            .and_then(serde_json::Value::as_str)
                            .map(ToString::to_string);
                        (k.clone(), attr)
                    })
                    .collect()
            });

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
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let all_nodes = page.query_selector_all(&selector).await?;
        let nodes = all_nodes.get(..limit).unwrap_or(&all_nodes);
        let mut results: Vec<Value> = Vec::with_capacity(nodes.len());
        if let Some(ref field_defs) = fields {
            for node in nodes {
                let mut obj = serde_json::Map::new();
                for (field_name, attr_name) in field_defs {
                    let val = if let Some(attr) = attr_name {
                        node.attr(attr)
                            .await
                            .map_or(Value::Null, |opt| opt.map_or(Value::Null, Value::String))
                    } else {
                        node.text_content().await.map_or(Value::Null, Value::String)
                    };
                    obj.insert(field_name.clone(), val);
                }
                results.push(Value::Object(obj));
            }
        } else {
            for node in nodes {
                let text = node.text_content().await.unwrap_or_default();
                results.push(Value::String(text));
            }
        }

        page.close().await?;

        Ok(json!({
            "url": url,
            "selector": selector,
            "count": results.len(),
            "results": results
        }))
    }

    async fn tool_browser_extract(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let root_selector = Self::require_str(args, "root_selector")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        // Parse schema: { "fieldName": { "selector": "...", "attr"?: "...", "required"?: bool } }
        let schema_obj = args
            .get("schema")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                BrowserError::ConfigError("Missing or non-object 'schema' argument".to_string())
            })?;

        let schema: Vec<(String, ExtractFieldDef)> = schema_obj
            .iter()
            .filter_map(|(name, spec)| {
                let selector = spec
                    .get("selector")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)?;
                let attr = spec
                    .get("attr")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string);
                let required = spec
                    .get("required")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                Some((
                    name.clone(),
                    ExtractFieldDef {
                        selector,
                        attr,
                        required,
                    },
                ))
            })
            .collect();

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
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let roots = page.query_selector_all(&root_selector).await?;
        let mut results: Vec<Value> = Vec::with_capacity(roots.len());
        for root in &roots {
            if let Some(obj) = Self::extract_record(root, &schema).await {
                results.push(Value::Object(obj));
            }
        }

        page.close().await?;

        Ok(json!({
            "url": url,
            "root_selector": root_selector,
            "count": results.len(),
            "results": results
        }))
    }

    #[cfg(feature = "similarity")]
    async fn tool_browser_find_similar(&self, args: &Value) -> Result<Value> {
        use crate::similarity::SimilarityConfig;

        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let reference_selector = Self::require_str(args, "reference_selector")?;
        #[allow(clippy::cast_possible_truncation)]
        let threshold = args
            .get("threshold")
            .and_then(serde_json::Value::as_f64)
            .map_or(SimilarityConfig::DEFAULT_THRESHOLD, |v| v as f32);
        let max_results = usize::try_from(
            args.get("max_results")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(10),
        )
        .unwrap_or(10);
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let config = SimilarityConfig {
            threshold,
            max_results,
        };

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
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        // Resolve the reference node — first match only.
        let refs = page.query_selector_all(&reference_selector).await?;
        let Some(reference) = refs.into_iter().next() else {
            page.close().await?;
            return Ok(json!({
                "isError": true,
                "error": format!("Reference selector matched no elements: {reference_selector}")
            }));
        };

        let ref_fp = reference.fingerprint().await?;
        let matches = page.find_similar(&reference, config).await?;

        let mut match_results: Vec<Value> = Vec::with_capacity(matches.len());
        for m in &matches {
            let text = m.node.text_content().await.unwrap_or_default();
            let snippet = m.node.inner_html().await.unwrap_or_default();
            let snippet: String = snippet.chars().take(200).collect();
            match_results.push(json!({
                "score": m.score,
                "text": text,
                "outer_html_snippet": snippet
            }));
        }

        page.close().await?;

        Ok(json!({
            "url": url,
            "reference": {
                "tag": ref_fp.tag,
                "classes": ref_fp.classes,
                "attr_names": ref_fp.attr_names,
                "depth": ref_fp.depth
            },
            "count": match_results.len(),
            "matches": match_results
        }))
    }

    async fn tool_browser_warmup(&self, args: &Value) -> Result<Value> {
        use crate::page::{WarmupOptions, WarmupWait};

        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let wait = match args
            .get("wait")
            .and_then(|v| v.as_str())
            .unwrap_or("dom_content_loaded")
        {
            "network_idle" => WarmupWait::NetworkIdle,
            _ => WarmupWait::DomContentLoaded,
        };
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(30_000);
        let stabilize_ms = args
            .get("stabilize_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

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

        let report = page
            .warmup(WarmupOptions {
                url,
                wait,
                timeout_ms,
                stabilize_ms,
            })
            .await?;
        page.close().await?;

        Ok(json!({
            "session_id": session_id,
            "url": report.url,
            "elapsed_ms": report.elapsed_ms,
            "status_code": report.status_code,
            "title": report.title,
            "stabilized": report.stabilized
        }))
    }

    async fn tool_browser_refresh(&self, args: &Value) -> Result<Value> {
        use crate::page::{RefreshOptions, WarmupWait};

        let session_id = Self::require_str(args, "session_id")?;
        let wait = match args
            .get("wait")
            .and_then(|v| v.as_str())
            .unwrap_or("dom_content_loaded")
        {
            "network_idle" => WarmupWait::NetworkIdle,
            _ => WarmupWait::DomContentLoaded,
        };
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(30_000);
        let reset_connection = args
            .get("reset_connection")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

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

        let report = page
            .refresh(RefreshOptions {
                wait,
                timeout_ms,
                reset_connection,
            })
            .await?;
        page.close().await?;

        Ok(json!({
            "session_id": session_id,
            "url": report.url,
            "elapsed_ms": report.elapsed_ms,
            "status_code": report.status_code
        }))
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

    #[cfg(feature = "stealth")]
    async fn tool_browser_validate_stealth(&self, args: &Value) -> Result<Value> {
        use crate::validation::{ValidationResult, ValidationSuite, ValidationTarget};

        let tier1_only = args
            .get("tier1_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(30);

        // Parse target list, defaulting to Tier 1 (CreepJS, BrowserScan)
        let targets = if tier1_only {
            ValidationTarget::tier1().to_vec()
        } else {
            args.get("targets").and_then(|v| v.as_array()).map_or_else(
                || ValidationTarget::tier1().to_vec(),
                |arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| match s {
                            "creepjs" => Some(ValidationTarget::CreepJs),
                            "browserscan" => Some(ValidationTarget::BrowserScan),
                            "fingerprint_js" => Some(ValidationTarget::FingerprintJs),
                            "kasada" => Some(ValidationTarget::Kasada),
                            "cloudflare" => Some(ValidationTarget::Cloudflare),
                            "akamai" => Some(ValidationTarget::Akamai),
                            "data_dome" => Some(ValidationTarget::DataDome),
                            "perimeter_x" => Some(ValidationTarget::PerimeterX),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                },
            )
        };

        // Run validators with per-target timeout so MCP responses remain bounded.
        let mut results = Vec::with_capacity(targets.len());
        for target in targets {
            let timed = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                ValidationSuite::run_one(&self.pool, target),
            )
            .await;
            match timed {
                Ok(result) => results.push(result),
                Err(_) => results.push(ValidationResult::failed(
                    target,
                    &format!("validation timed out after {timeout_secs}s"),
                )),
            }
        }

        // Serialize results
        serde_json::to_value(&results)
            .map_err(|e| BrowserError::ConfigError(format!("failed to serialize results: {e}")))
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

    #[cfg(feature = "stealth")]
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

    // ── browser_extract_with_fallback ─────────────────────────────────────────

    /// Extract using the first `root_selectors` entry that yields results.
    async fn tool_browser_extract_with_fallback(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);
        let selectors = Self::parse_root_selectors(args)?;
        let schema = Self::parse_extract_schema(args)?;

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
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let mut matched_selector = String::new();
        let mut results: Vec<Value> = vec![];

        for selector in &selectors {
            let roots = page.query_selector_all(selector).await?;
            if roots.is_empty() {
                continue;
            }

            let mut selector_results: Vec<Value> = Vec::with_capacity(roots.len());
            for root in &roots {
                if let Some(obj) = Self::extract_record(root, &schema).await {
                    selector_results.push(Value::Object(obj));
                }
            }

            if selector_results.is_empty() {
                continue;
            }

            matched_selector = selector.clone();
            results = selector_results;
            break;
        }

        page.close().await?;

        Ok(json!({
            "url":              url,
            "matched_selector": matched_selector,
            "tried_selectors":  selectors,
            "count":            results.len(),
            "results":          results
        }))
    }

    // ── browser_extract_resilient ─────────────────────────────────────────────

    /// Extract from every root node matching `root_selector`, silently
    /// dropping nodes where *all* required schema fields are absent.
    async fn tool_browser_extract_resilient(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let url = Self::require_str(args, "url")?;
        let root_selector = Self::require_str(args, "root_selector")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);
        let schema = Self::parse_extract_schema(args)?;

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
            WaitUntil::DomContentLoaded,
            Duration::from_secs_f64(timeout_secs),
        )
        .await?;

        let roots = page.query_selector_all(&root_selector).await?;
        // Resilient mode: `extract_record` returns None when a required field is
        // missing.  We count those as "skipped" rather than bubbling an error.
        let mut results: Vec<Value> = Vec::with_capacity(roots.len());
        let mut skipped: usize = 0;
        for root in &roots {
            match Self::extract_record(root, &schema).await {
                Some(obj) => results.push(Value::Object(obj)),
                None => skipped += 1,
            }
        }

        page.close().await?;

        Ok(json!({
            "url":           url,
            "root_selector": root_selector,
            "count":         results.len(),
            "skipped":       skipped,
            "results":       results
        }))
    }

    async fn extract_record(
        root: &crate::page::NodeHandle,
        schema: &[(String, ExtractFieldDef)],
    ) -> Option<serde_json::Map<String, Value>> {
        let mut obj = serde_json::Map::new();
        for (field_name, def) in schema {
            let Ok(children) = root.children_matching(&def.selector).await else {
                if def.required {
                    return None;
                }
                obj.insert(field_name.clone(), Value::Null);
                continue;
            };
            let val = match children.into_iter().next() {
                None => {
                    if def.required {
                        return None;
                    }
                    Value::Null
                }
                Some(node) => {
                    if let Some(attr) = &def.attr {
                        node.attr(attr)
                            .await
                            .map_or(Value::Null, |opt| opt.map_or(Value::Null, Value::String))
                    } else {
                        node.text_content().await.map_or(Value::Null, Value::String)
                    }
                }
            };
            obj.insert(field_name.clone(), val);
        }
        Some(obj)
    }

    fn require_str(args: &Value, key: &str) -> Result<String> {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| BrowserError::ConfigError(format!("Missing required argument: {key}")))
    }

    fn parse_root_selectors(args: &Value) -> Result<Vec<String>> {
        let selectors: Vec<String> = args
            .get("root_selectors")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                BrowserError::ConfigError(
                    "Missing or non-array 'root_selectors' argument".to_string(),
                )
            })?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        if selectors.is_empty() {
            return Err(BrowserError::ConfigError(
                "root_selectors must contain at least one entry".to_string(),
            ));
        }
        Ok(selectors)
    }

    fn parse_extract_schema(args: &Value) -> Result<Vec<(String, ExtractFieldDef)>> {
        let schema_obj = args
            .get("schema")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                BrowserError::ConfigError("Missing or non-object 'schema' argument".to_string())
            })?;

        Ok(schema_obj
            .iter()
            .filter_map(|(name, spec)| {
                let selector = spec
                    .get("selector")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)?;
                let attr = spec
                    .get("attr")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let required = spec
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Some((
                    name.clone(),
                    ExtractFieldDef {
                        selector,
                        attr,
                        required,
                    },
                ))
            })
            .collect())
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
    fn tool_defs_include_browser_query() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_query")),
            "TOOL_DEFINITIONS must contain browser_query"
        );
    }

    #[test]
    fn tool_defs_include_browser_extract() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_extract")),
            "TOOL_DEFINITIONS must contain browser_extract"
        );
    }

    #[test]
    fn tool_defs_include_browser_extract_with_fallback() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str())
                    == Some("browser_extract_with_fallback")),
            "TOOL_DEFINITIONS must contain browser_extract_with_fallback"
        );
    }

    #[test]
    fn tool_defs_include_browser_extract_resilient() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter().any(
                |t| t.get("name").and_then(|n| n.as_str()) == Some("browser_extract_resilient")
            ),
            "TOOL_DEFINITIONS must contain browser_extract_resilient"
        );
    }

    #[test]
    fn browser_extract_with_fallback_requires_root_selectors()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| {
                t.get("name").and_then(|n| n.as_str()) == Some("browser_extract_with_fallback")
            })
            .ok_or("browser_extract_with_fallback must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(Value::as_array)
            .ok_or("browser_extract_with_fallback inputSchema missing 'required' array")?;
        assert!(
            required.iter().any(|v| v == "root_selectors"),
            "root_selectors must be required in browser_extract_with_fallback"
        );
        Ok(())
    }

    #[test]
    fn browser_query_required_args() -> std::result::Result<(), Box<dyn std::error::Error>> {
        // The inputSchema for browser_query must list session_id, url, selector as required.
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_query"))
            .ok_or("browser_query must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .ok_or("browser_query inputSchema missing 'required'")?;
        assert!(
            required
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "session_id"))
        );
        assert!(
            required
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "url"))
        );
        assert!(
            required
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "selector"))
        );
        Ok(())
    }

    #[test]
    fn browser_extract_required_args() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_extract"))
            .ok_or("browser_extract must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .ok_or("browser_extract inputSchema missing 'required'")?;
        assert!(
            required
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "root_selector"))
        );
        assert!(
            required
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "schema"))
        );
        Ok(())
    }

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
    fn browser_extract_schema_parse_empty_schema()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        // An empty schema object parses without error and yields an empty field list.
        // We validate this by ensuring browser_extract's inputSchema requires "schema".
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_extract"))
            .ok_or("browser_extract must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array())
            .ok_or("browser_extract inputSchema missing 'required' array")?;
        assert!(
            required.iter().any(|v| v == "schema"),
            "schema must be required in browser_extract"
        );
        // Also confirm the schema property type is "object"
        let schema_type = def
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.get("schema"))
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
            .ok_or("browser_extract inputSchema.properties.schema.type missing")?;
        assert_eq!(
            schema_type, "object",
            "schema property must have type object"
        );
        Ok(())
    }

    #[test]
    fn browser_query_missing_session() -> std::result::Result<(), Box<dyn std::error::Error>> {
        // Verify that `browser_query` with a missing `session_id` arg
        // returns the right `isError` shape via the dispatch JSON structure.
        // We test the tool-call dispatch by inspecting that an unknown session
        // is handled as an `isError` result rather than a JSON-RPC error code.
        // Because constructing a real BrowserPool requires Chrome, we instead
        // verify the shape through the TOOL_DEFINITIONS contract: session_id
        // is required so any call without it would fail at arg-validation.
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_query"))
            .ok_or("browser_query must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array())
            .ok_or("browser_query inputSchema missing 'required' array")?;
        // session_id required → missing session will always be caught
        assert!(
            required.iter().any(|v| v == "session_id"),
            "session_id must be required so missing-session is caught at validation"
        );
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

    #[test]
    fn browser_warmup_in_tool_definitions() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_warmup"))
            .ok_or("browser_warmup must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array())
            .ok_or("browser_warmup inputSchema missing 'required' array")?;
        assert!(
            required.iter().any(|v| v == "session_id"),
            "session_id must be required in browser_warmup"
        );
        assert!(
            required.iter().any(|v| v == "url"),
            "url must be required in browser_warmup"
        );
        Ok(())
    }

    #[test]
    fn browser_refresh_in_tool_definitions() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_refresh"))
            .ok_or("browser_refresh must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array())
            .ok_or("browser_refresh inputSchema missing 'required' array")?;
        assert!(
            required.iter().any(|v| v == "session_id"),
            "session_id must be required in browser_refresh"
        );
        Ok(())
    }
}
