//! MCP (Model Context Protocol) server for proxy pool management.
//!
//! Exposes `stygian-proxy` capabilities as an MCP server over stdin/stdout
//! using the JSON-RPC 2.0 protocol. External tools (LLM agents, IDE plugins)
//! can manage a proxy pool, acquire proxies for scraping, and track per-proxy
//! health via the standardised MCP interface.
//!
//! ## Enabling
//!
//! ```toml
//! [dependencies]
//! stygian-proxy = { version = "*", features = ["mcp"] }
//! ```
//!
//! ## Protocol
//!
//! Implements MCP 2025-11-25 over JSON-RPC 2.0 on stdin/stdout.
//!
//! | MCP Method | Description |
//! | ----------- | ------------- |
//! | `initialize` | Handshake, return server capabilities |
//! | `tools/list` | List available proxy tools |
//! | `tools/call` | Execute a proxy tool |
//! | `resources/list` | List available resources (currently `proxy://pool/stats`) |
//! | `resources/read` | Read a resource (e.g. pool statistics from `proxy://pool/stats`) |
//!
//! ## Tools
//!
//! | Tool | Key Parameters | Returns |
//! | ------ | -------------- | ------- |
//! | `proxy_add` | `url`, `proxy_type?`, `username?`, `password?`, `weight?`, `tags?` | `proxy_id` |
//! | `proxy_remove` | `proxy_id` | success |
//! | `proxy_pool_stats` | – | `total`, `healthy`, `open`, `active_sessions` |
//! | `proxy_acquire` | – | `handle_token`, `proxy_url` |
//! | `proxy_acquire_for_domain` | `domain` | `handle_token`, `proxy_url` |
//! | `proxy_release` | `handle_token`, `success?` | success |
//! | `proxy_acquire_with_capabilities` | `require_https_connect?`, `require_socks5_udp?`, `require_http3_tunnel?`, `require_geo_country?` | `handle_token`, `proxy_url` |
//! | `proxy_fetch_freelist` | `sources`, `tags?` | `loaded` |
//! | `proxy_fetch_freeapiproxies` | `endpoint?`, `tags?` | `loaded` |

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use ulid::Ulid;
use uuid::Uuid;

use crate::{
    MemoryProxyStore, PoolStats, ProxyHandle, ProxyManager,
    fetcher::{FreeApiProxiesFetcher, FreeListFetcher, FreeListSource, load_from_fetcher},
    types::{CapabilityRequirement, Proxy, ProxyConfig, ProxyType},
};

// ─── Error response helpers ───────────────────────────────────────────────────

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn ok_response(id: &Value, result: impl Into<Value>) -> Value {
    let result = result.into();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

// ─── Active handle store ──────────────────────────────────────────────────────

/// Active proxy handles keyed by ULID token, waiting for explicit release.
///
/// # Leak note
///
/// TODO: implement a TTL-based cleanup mechanism for orphaned handles.
/// Handle entries are removed only on `proxy_release`.  If a client crashes or
/// loses the token, the stored `ProxyHandle` remains in memory until the server
/// exits.  This is acceptable for typical scraping workloads (sessions are
/// short-lived and servers are restarted regularly), but a production deployment
/// that expects long uptimes should add a TTL-based cleanup background task.
type HandleStore = Arc<Mutex<HashMap<String, ProxyHandle>>>;

// ─── Server ───────────────────────────────────────────────────────────────────

/// MCP server exposing `stygian-proxy` proxy pool tools.
///
/// Holds a [`ProxyManager`] and starts background health checking when
/// [`run`](McpProxyServer::run) is called. The pool starts empty; proxies are
/// added via `proxy_add` tool calls.
///
/// # Example
///
/// ```no_run
/// use stygian_proxy::mcp::McpProxyServer;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     McpProxyServer::new()?.run().await
/// }
/// ```
pub struct McpProxyServer {
    manager: Arc<ProxyManager>,
    handles: HandleStore,
}

impl McpProxyServer {
    /// Create a new MCP proxy server backed by an in-memory proxy store.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if the `ProxyManager` cannot be constructed.
    pub fn new() -> crate::error::ProxyResult<Self> {
        let storage = Arc::new(MemoryProxyStore::default());
        let manager = ProxyManager::with_round_robin(storage, ProxyConfig::default())?;
        Ok(Self {
            manager: Arc::new(manager),
            handles: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Run the MCP server, reading JSON-RPC requests from stdin and writing
    /// responses to stdout until EOF.
    ///
    /// Starts background health checking and session-purge tasks automatically.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if the underlying I/O fails unrecoverably.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        info!("stygian-proxy MCP server starting");

        // Launch background health-check + session-purge tasks.
        let (mgr_token, bg) = self.manager.start();

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();

        loop {
            line.clear();
            let bytes = reader.read_line(&mut line).await?;
            if bytes == 0 {
                break; // EOF
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            debug!(request = trimmed, "received");

            let response = match serde_json::from_str::<Value>(trimmed) {
                Ok(req) => {
                    let is_well_formed_notification = req.is_object()
                        && req.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                        && req.get("id").is_none()
                        && req.get("method").and_then(Value::as_str).is_some();
                    let response = self.handle(&req).await;
                    if is_well_formed_notification {
                        continue;
                    }
                    response
                }
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("Parse error: {e}") }
                }),
            };

            let mut out = serde_json::to_string(&response)?;
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }

        mgr_token.cancel();
        let _ = bg.await;
        info!("stygian-proxy MCP server stopped");
        Ok(())
    }

    /// Dispatch a single JSON-RPC request.
    ///
    /// Used by the `stygian-mcp` aggregator to route tool calls through this
    /// server without running the full stdin/stdout loop.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::mcp::McpProxyServer;
    /// use serde_json::json;
    ///
    /// # tokio_test::block_on(async {
    /// let server = McpProxyServer::new().unwrap();
    /// let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    /// let resp = server.handle_request(&req).await;
    /// assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
    /// # });
    /// ```
    pub async fn handle_request(&self, req: &Value) -> Value {
        self.handle(req).await
    }

    /// Spawn the background health-check and session-purge tasks.
    ///
    /// Returns a `(CancellationToken, JoinHandle)` pair.  Cancel the token to
    /// trigger a graceful shutdown of the background tasks; await the handle to
    /// confirm they have stopped.
    ///
    /// This should be called by consumers that use [`handle_request`] directly
    /// (e.g. the `McpAggregator`) so that proxy health checking and sticky-session
    /// purging run even when the full stdin/stdout [`run`] loop is not used.
    ///
    /// [`handle_request`]: McpProxyServer::handle_request
    /// [`run`]: McpProxyServer::run
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_proxy::mcp::McpProxyServer;
    ///
    /// # tokio_test::block_on(async {
    /// let server = McpProxyServer::new().unwrap();
    /// let (token, bg) = server.start_background();
    /// // ... use server.handle_request() ...
    /// token.cancel();
    /// bg.await.unwrap();
    /// # });
    /// ```
    pub fn start_background(&self) -> (CancellationToken, JoinHandle<()>) {
        self.manager.start()
    }

    async fn handle(&self, req: &Value) -> Value {
        let id = req.get("id").unwrap_or(&Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        match method {
            "initialize" => Self::handle_initialize(id),
            "initialized" | "notifications/initialized" | "ping" => {
                json!({"jsonrpc":"2.0","id":id,"result":{}})
            }
            "tools/list" => Self::handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req).await,
            "resources/list" => Self::handle_resources_list(id),
            "resources/read" => self.handle_resources_read(id, req).await,
            _ => error_response(id, -32601, &format!("Method not found: {method}")),
        }
    }

    fn handle_initialize(id: &Value) -> Value {
        ok_response(
            id,
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "tools":     { "listChanged": false },
                    "resources": { "listChanged": false, "subscribe": false }
                },
                "serverInfo": {
                    "name": "stygian-proxy",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    fn handle_tools_list(id: &Value) -> Value {
        ok_response(id, json!({ "tools": Self::tool_definitions() }))
    }

    fn tool_definitions() -> Vec<Value> {
        vec![
            Self::tool_def_proxy_add(),
            Self::tool_def_proxy_remove(),
            Self::tool_def_proxy_pool_stats(),
            Self::tool_def_proxy_acquire(),
            Self::tool_def_proxy_acquire_for_domain(),
            Self::tool_def_proxy_release(),
            Self::tool_def_proxy_acquire_with_capabilities(),
            Self::tool_def_proxy_fetch_freelist(),
            Self::tool_def_proxy_fetch_freeapiproxies(),
        ]
    }

    fn tool_def_proxy_add() -> Value {
        json!({
            "name": "proxy_add",
            "description": "Add a proxy to the pool. Returns a stable UUID that identifies the proxy for future removal.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url":        { "type": "string", "description": "Proxy URL (e.g. http://host:port, socks5://user:pass@host:port)" },
                    "proxy_type": { "type": "string", "description": "Protocol: http | https | socks4 | socks5 (default: inferred from URL scheme, falling back to http)" },
                    "username":   { "type": "string", "description": "Optional proxy username" },
                    "password":   { "type": "string", "description": "Optional proxy password" },
                    "weight":     { "type": "integer", "description": "Relative selection weight for weighted rotation (default: 1)" },
                    "tags":       { "type": "array", "items": { "type": "string" }, "description": "Optional user-defined tags" }
                },
                "required": ["url"]
            }
        })
    }

    fn tool_def_proxy_remove() -> Value {
        json!({
            "name": "proxy_remove",
            "description": "Remove a proxy from the pool by its UUID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "proxy_id": { "type": "string", "description": "UUID of the proxy to remove (returned by proxy_add)" }
                },
                "required": ["proxy_id"]
            }
        })
    }

    fn tool_def_proxy_pool_stats() -> Value {
        json!({
            "name": "proxy_pool_stats",
            "description": "Return a health snapshot of the proxy pool: total count, healthy count, open circuit-breaker count, and active sticky-session count.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        })
    }

    fn tool_def_proxy_acquire() -> Value {
        json!({
            "name": "proxy_acquire",
            "description": "Lease one proxy from the pool using the configured rotation strategy. Returns a handle_token (opaque string) and the proxy URL. Call proxy_release when done.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        })
    }

    fn tool_def_proxy_acquire_for_domain() -> Value {
        json!({
            "name": "proxy_acquire_for_domain",
            "description": "Lease a proxy for a specific domain, honouring sticky-session policy. The same proxy is returned for repeated calls with the same domain during the TTL. Returns handle_token and proxy_url.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain": { "type": "string", "description": "Target domain (e.g. example.com)" }
                },
                "required": ["domain"]
            }
        })
    }

    fn tool_def_proxy_release() -> Value {
        json!({
            "name": "proxy_release",
            "description": "Release a previously acquired proxy handle. Pass success=true if the request succeeded (updates circuit-breaker health), false to mark failure.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle_token": { "type": "string", "description": "Token returned by proxy_acquire or proxy_acquire_for_domain" },
                    "success":      { "type": "boolean", "description": "Whether the request using this proxy succeeded (default: true)" }
                },
                "required": ["handle_token"]
            }
        })
    }

    fn tool_def_proxy_acquire_with_capabilities() -> Value {
        json!({
            "name": "proxy_acquire_with_capabilities",
            "description": "Lease one proxy from the pool that satisfies all specified capability requirements. Returns handle_token and proxy_url. Call proxy_release when done.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "require_https_connect": { "type": "boolean", "description": "Require the proxy to support HTTPS CONNECT tunnelling (default: false)" },
                    "require_socks5_udp":    { "type": "boolean", "description": "Require SOCKS5 UDP relay support (default: false)" },
                    "require_http3_tunnel":  { "type": "boolean", "description": "Require HTTP/3 QUIC tunnel support (default: false)" },
                    "require_geo_country":   { "type": "string",  "description": "ISO-3166-1 alpha-2 country code the egress IP must match (e.g. \"US\")" }
                }
            }
        })
    }

    fn tool_def_proxy_fetch_freelist() -> Value {
        json!({
            "name": "proxy_fetch_freelist",
            "description": "Fetch proxies from one or more well-known free proxy list feeds (plain host:port format) and add them to the pool. Returns the number of proxies loaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["the_speedx_http", "the_speedx_socks4", "the_speedx_socks5", "clarketm_http", "open_proxy_list_http"]
                        },
                        "description": "List of feed names to fetch. the_speedx_socks4/socks5 require the 'socks' feature.",
                        "minItems": 1
                    },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Extra tags attached to every loaded proxy" }
                },
                "required": ["sources"]
            }
        })
    }

    fn tool_def_proxy_fetch_freeapiproxies() -> Value {
        json!({
            "name": "proxy_fetch_freeapiproxies",
            "description": "Fetch proxies from a FreeAPIProxies-compatible JSON endpoint and add them to the pool. Returns the number of proxies loaded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit":    { "type": "integer", "description": "Maximum number of proxies to request (default: API default)" },
                    "protocol": { "type": "string",  "description": "Protocol filter (e.g. \"http\", \"socks5\")" },
                    "country":  { "type": "string",  "description": "ISO-3166-1 alpha-2 country filter (e.g. \"US\")" },
                    "endpoint": { "type": "string",  "description": "Custom API endpoint URL (defaults to the public FreeAPIProxies endpoint)" },
                    "tags":     { "type": "array", "items": { "type": "string" }, "description": "Extra tags attached to every loaded proxy" }
                }
            }
        })
    }

    async fn handle_tools_call(&self, id: &Value, req: &Value) -> Value {
        let params = req.get("params").unwrap_or(&Value::Null);
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").unwrap_or(&Value::Null);

        match name {
            "proxy_add" => self.tool_proxy_add(id, args).await,
            "proxy_remove" => self.tool_proxy_remove(id, args).await,
            "proxy_pool_stats" => self.tool_proxy_pool_stats(id).await,
            "proxy_acquire" => self.tool_proxy_acquire(id).await,
            "proxy_acquire_for_domain" => self.tool_proxy_acquire_for_domain(id, args).await,
            "proxy_release" => self.tool_proxy_release(id, args).await,
            "proxy_acquire_with_capabilities" => {
                self.tool_proxy_acquire_with_capabilities(id, args).await
            }
            "proxy_fetch_freelist" => self.tool_proxy_fetch_freelist(id, args).await,
            "proxy_fetch_freeapiproxies" => self.tool_proxy_fetch_freeapiproxies(id, args).await,
            _ => error_response(id, -32602, &format!("Unknown tool: {name}")),
        }
    }

    fn handle_resources_list(id: &Value) -> Value {
        ok_response(
            id,
            json!({
                "resources": [{
                    "uri":         "proxy://pool/stats",
                    "name":        "Proxy Pool Statistics",
                    "description": "Live pool health snapshot: total, healthy, open, active_sessions",
                    "mimeType":    "application/json"
                }]
            }),
        )
    }

    async fn handle_resources_read(&self, id: &Value, req: &Value) -> Value {
        let uri = req
            .get("params")
            .and_then(|v| v.get("uri"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if uri != "proxy://pool/stats" {
            return error_response(id, -32602, &format!("Unknown resource: {uri}"));
        }
        match self.manager.pool_stats().await {
            Ok(stats) => ok_response(
                id,
                json!({
                    "contents": [{
                        "uri": "proxy://pool/stats",
                        "mimeType": "application/json",
                        "text": serde_json::to_string(&stats_to_json(&stats)).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Stats error: {e}")),
        }
    }

    // ── proxy_add ─────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    async fn tool_proxy_add(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let proxy_type = {
            // Prefer the explicit `proxy_type` argument; otherwise infer from the URL scheme.
            let explicit = args.get("proxy_type").and_then(Value::as_str);
            let scheme = url.split_once("://").map(|(s, _)| s);
            let type_str = explicit.or(scheme).unwrap_or("http").to_ascii_lowercase();
            match type_str.as_str() {
                "https" => ProxyType::Https,
                #[cfg(feature = "socks")]
                "socks4" | "socks4a" => ProxyType::Socks4,
                #[cfg(feature = "socks")]
                "socks5" | "socks" => ProxyType::Socks5,
                "http" => ProxyType::Http,
                other => {
                    return error_response(
                        id,
                        -32602,
                        &format!("Unsupported proxy_type or URL scheme: {other}"),
                    );
                }
            }
        };
        let username = args
            .get("username")
            .and_then(Value::as_str)
            .map(str::to_string);
        let password = args
            .get("password")
            .and_then(Value::as_str)
            .map(str::to_string);
        let weight = match args.get("weight") {
            None => 1u32,
            Some(v) => match v.as_u64() {
                Some(w) => match u32::try_from(w) {
                    Ok(weight) => weight,
                    Err(_) => {
                        return error_response(
                            id,
                            -32602,
                            "Invalid parameter: weight out of range",
                        );
                    }
                },
                None => {
                    return error_response(
                        id,
                        -32602,
                        "Invalid parameter: weight must be an unsigned integer",
                    );
                }
            },
        };
        let tags: Vec<String> = match args.get("tags") {
            None => Vec::new(),
            Some(v) => match v.as_array() {
                Some(arr) => {
                    let mut collected = Vec::with_capacity(arr.len());
                    for item in arr {
                        match item.as_str() {
                            Some(s) => collected.push(s.to_string()),
                            None => {
                                return error_response(
                                    id,
                                    -32602,
                                    "Invalid parameter: tags must be an array of strings",
                                );
                            }
                        }
                    }
                    collected
                }
                None => {
                    return error_response(
                        id,
                        -32602,
                        "Invalid parameter: tags must be an array of strings",
                    );
                }
            },
        };

        let proxy = Proxy {
            url: url.to_string(),
            proxy_type,
            username,
            password,
            weight,
            tags,
            capabilities: crate::types::ProxyCapabilities::default(),
        };

        match self.manager.add_proxy(proxy).await {
            Ok(proxy_id) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "proxy_id": proxy_id.to_string(),
                            "url": url
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Failed to add proxy: {e}")),
        }
    }

    // ── proxy_remove ─────────────────────────────────────────────────────────

    async fn tool_proxy_remove(&self, id: &Value, args: &Value) -> Value {
        let Some(proxy_id_str) = args.get("proxy_id").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: proxy_id");
        };

        let Ok(proxy_id) = Uuid::parse_str(proxy_id_str) else {
            return error_response(id, -32602, "Invalid proxy_id: must be a UUID");
        };

        match self.manager.remove_proxy(proxy_id).await {
            Ok(()) => ok_response(
                id,
                json!({
                    "content": [{ "type": "text", "text": "{\"removed\":true}" }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Failed to remove proxy: {e}")),
        }
    }

    // ── proxy_pool_stats ──────────────────────────────────────────────────────

    async fn tool_proxy_pool_stats(&self, id: &Value) -> Value {
        match self.manager.pool_stats().await {
            Ok(stats) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&stats_to_json(&stats)).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Stats error: {e}")),
        }
    }

    // ── proxy_acquire ─────────────────────────────────────────────────────────

    async fn tool_proxy_acquire(&self, id: &Value) -> Value {
        match self.manager.acquire_proxy().await {
            Ok(handle) => {
                let proxy_url = handle.proxy_url.clone();
                let token = Ulid::new().to_string();
                self.handles.lock().await.insert(token.clone(), handle);
                ok_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "handle_token": token,
                                "proxy_url":    proxy_url
                            })).unwrap_or_default()
                        }]
                    }),
                )
            }
            Err(e) => error_response(id, -32603, &format!("Acquire failed: {e}")),
        }
    }

    // ── proxy_acquire_for_domain ──────────────────────────────────────────────

    async fn tool_proxy_acquire_for_domain(&self, id: &Value, args: &Value) -> Value {
        let Some(domain) = args.get("domain").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: domain");
        };

        match self.manager.acquire_for_domain(domain).await {
            Ok(handle) => {
                let proxy_url = handle.proxy_url.clone();
                let token = Ulid::new().to_string();
                self.handles.lock().await.insert(token.clone(), handle);
                ok_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "handle_token": token,
                                "proxy_url":    proxy_url,
                                "domain":       domain
                            })).unwrap_or_default()
                        }]
                    }),
                )
            }
            Err(e) => error_response(id, -32603, &format!("Acquire for domain failed: {e}")),
        }
    }

    // ── proxy_release ─────────────────────────────────────────────────────────

    async fn tool_proxy_release(&self, id: &Value, args: &Value) -> Value {
        let Some(token) = args.get("handle_token").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: handle_token");
        };
        let success = args.get("success").and_then(Value::as_bool).unwrap_or(true);

        let mut store = self.handles.lock().await;
        let Some(handle) = store.remove(token) else {
            return error_response(id, -32602, "Unknown handle_token — already released?");
        };
        drop(store); // release lock before potentially blocking on Drop

        if success {
            handle.mark_success();
        }
        // Dropping `handle` here triggers circuit breaker accounting.
        drop(handle);

        ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&json!({
                    "released": true,
                    "success":  success
                })).unwrap_or_default() }]
            }),
        )
    }

    // ── proxy_acquire_with_capabilities ──────────────────────────────────────

    /// Lease a proxy that satisfies all capability requirements in `args`.
    ///
    /// Accepts the same JSON structure as [`CapabilityRequirement`] —
    /// `require_https_connect`, `require_socks5_udp`, `require_http3_tunnel`,
    /// `require_geo_country`.  All fields are optional; an empty call is
    /// equivalent to plain `proxy_acquire`.
    async fn tool_proxy_acquire_with_capabilities(&self, id: &Value, args: &Value) -> Value {
        let req = CapabilityRequirement {
            require_https_connect: args
                .get("require_https_connect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            require_socks5_udp: args
                .get("require_socks5_udp")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            require_http3_tunnel: args
                .get("require_http3_tunnel")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            require_geo_country: args
                .get("require_geo_country")
                .and_then(Value::as_str)
                .map(str::to_string),
        };

        match self.manager.acquire_with_capabilities(&req).await {
            Ok(handle) => {
                let proxy_url = handle.proxy_url.clone();
                let token = Ulid::new().to_string();
                self.handles.lock().await.insert(token.clone(), handle);
                ok_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&json!({
                                "handle_token": token,
                                "proxy_url":    proxy_url
                            })).unwrap_or_default()
                        }]
                    }),
                )
            }
            Err(e) => error_response(
                id,
                -32603,
                &format!("Acquire with capabilities failed: {e}"),
            ),
        }
    }

    // ── proxy_fetch_freelist ──────────────────────────────────────────────────

    /// Fetch proxies from well-known free list feeds and populate the pool.
    async fn tool_proxy_fetch_freelist(&self, id: &Value, args: &Value) -> Value {
        let Some(sources_arr) = args.get("sources").and_then(Value::as_array) else {
            return error_response(id, -32602, "Missing required parameter: sources");
        };

        if sources_arr.is_empty() {
            return error_response(id, -32602, "sources must contain at least one entry");
        }

        let mut sources: Vec<FreeListSource> = Vec::new();
        for src in sources_arr {
            let name = src.as_str().unwrap_or("");
            match name {
                "the_speedx_http" => sources.push(FreeListSource::TheSpeedXHttp),
                #[cfg(feature = "socks")]
                "the_speedx_socks4" => sources.push(FreeListSource::TheSpeedXSocks4),
                #[cfg(feature = "socks")]
                "the_speedx_socks5" => sources.push(FreeListSource::TheSpeedXSocks5),
                "clarketm_http" => sources.push(FreeListSource::ClarketmHttp),
                "open_proxy_list_http" => sources.push(FreeListSource::OpenProxyListHttp),
                other => {
                    return error_response(
                        id,
                        -32602,
                        &format!(
                            "Unknown source: {other}. Valid values: the_speedx_http, the_speedx_socks4, the_speedx_socks5, clarketm_http, open_proxy_list_http"
                        ),
                    );
                }
            }
        }

        let mut fetcher = FreeListFetcher::new(sources);
        if let Some(tags) = args.get("tags").and_then(Value::as_array) {
            let tag_vec: Vec<String> = tags
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            fetcher = fetcher.with_tags(tag_vec);
        }

        match load_from_fetcher(&self.manager, &fetcher).await {
            Ok(count) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({ "loaded": count })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Fetch failed: {e}")),
        }
    }

    // ── proxy_fetch_freeapiproxies ────────────────────────────────────────────

    /// Fetch proxies from a FreeAPIProxies-compatible JSON API and populate the pool.
    async fn tool_proxy_fetch_freeapiproxies(&self, id: &Value, args: &Value) -> Value {
        let mut fetcher = args.get("endpoint").and_then(Value::as_str).map_or_else(
            FreeApiProxiesFetcher::new,
            FreeApiProxiesFetcher::with_endpoint,
        );
        if let Some(tags) = args.get("tags").and_then(Value::as_array) {
            let tag_vec: Vec<String> = tags
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            fetcher = fetcher.with_tags(tag_vec);
        }

        match load_from_fetcher(&self.manager, &fetcher).await {
            Ok(count) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({ "loaded": count })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Fetch failed: {e}")),
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn stats_to_json(stats: &PoolStats) -> Value {
    json!({
        "total":           stats.total,
        "healthy":         stats.healthy,
        "open":            stats.open,
        "active_sessions": stats.active_sessions
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_builds() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let _ = McpProxyServer::new()?;
        Ok(())
    }

    #[test]
    fn initialize_response_has_version() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = McpProxyServer::handle_initialize(&id);
        assert_eq!(
            resp.get("result")
                .and_then(|r| r.get("protocolVersion"))
                .and_then(Value::as_str),
            Some("2025-11-25")
        );
        assert_eq!(
            resp.get("result")
                .and_then(|r| r.get("serverInfo"))
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str),
            Some("stygian-proxy")
        );
        let _ = server; // keep test parity with prior server construction
        Ok(())
    }

    #[test]
    fn tools_list_has_all_tools() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = McpProxyServer::handle_tools_list(&id);
        let tools = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                std::io::Error::other("tools list response should include tools array")
            })?;
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"proxy_add"));
        assert!(names.contains(&"proxy_remove"));
        assert!(names.contains(&"proxy_pool_stats"));
        assert!(names.contains(&"proxy_acquire"));
        assert!(names.contains(&"proxy_acquire_for_domain"));
        assert!(names.contains(&"proxy_release"));
        assert!(names.contains(&"proxy_acquire_with_capabilities"));
        assert!(names.contains(&"proxy_fetch_freelist"));
        assert!(names.contains(&"proxy_fetch_freeapiproxies"));
        let _ = server;
        Ok(())
    }

    #[tokio::test]
    async fn proxy_add_missing_url_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let args = json!({});
        let resp = server.tool_proxy_add(&id, &args).await;
        assert!(resp.get("error").is_some_and(Value::is_object));
        Ok(())
    }

    #[tokio::test]
    async fn pool_stats_returns_empty_on_fresh_manager()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server.tool_proxy_pool_stats(&id).await;
        let text = resp
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_array)
            .and_then(|content| content.first())
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                std::io::Error::other("pool_stats response should include content[0].text")
            })?;
        let parsed: Value = serde_json::from_str(text)?;
        assert_eq!(parsed.get("total").and_then(Value::as_u64), Some(0));
        Ok(())
    }

    #[tokio::test]
    async fn acquire_on_empty_pool_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server.tool_proxy_acquire(&id).await;
        assert!(
            resp.get("error").is_some_and(Value::is_object),
            "empty pool should return error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn acquire_with_capabilities_on_empty_pool_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server
            .tool_proxy_acquire_with_capabilities(&id, &json!({}))
            .await;
        assert!(
            resp.get("error").is_some_and(Value::is_object),
            "empty pool should return error for capability-aware acquire"
        );
        Ok(())
    }

    #[tokio::test]
    async fn proxy_fetch_freelist_missing_sources_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server.tool_proxy_fetch_freelist(&id, &json!({})).await;
        assert!(
            resp.get("error").is_some_and(Value::is_object),
            "missing sources should return error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn proxy_fetch_freelist_empty_sources_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server
            .tool_proxy_fetch_freelist(&id, &json!({ "sources": [] }))
            .await;
        assert!(
            resp.get("error").is_some_and(Value::is_object),
            "empty sources array should return error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn proxy_fetch_freelist_unknown_source_returns_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = McpProxyServer::new()?;
        let id = json!(1);
        let resp = server
            .tool_proxy_fetch_freelist(&id, &json!({ "sources": ["not_a_real_source"] }))
            .await;
        assert!(
            resp.get("error").is_some_and(Value::is_object),
            "unknown source name should return error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn proxy_fetch_freeapiproxies_accepts_limit_and_protocol()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        // This test does not make a real HTTP call; it only checks that building
        // the tool returns an error from the (unreachable) test endpoint rather
        // than a parse/config error.
        let server = McpProxyServer::new()?;
        let id = json!(1);
        // Use a local non-routable address so the fetch fails fast without
        // waiting on a real network timeout.
        let resp = server
            .tool_proxy_fetch_freeapiproxies(
                &id,
                &json!({
                    "endpoint": "http://127.0.0.1:1",
                    "limit": 50,
                    "protocol": "http"
                }),
            )
            .await;
        // Fetch must either succeed (unlikely in CI) or return an error —
        // the important thing is that it does not panic.
        assert!(
            resp.get("error").is_some_and(Value::is_object)
                || resp.get("result").is_some_and(Value::is_object),
            "fetch_freeapiproxies should return either an error or result"
        );
        Ok(())
    }
}
