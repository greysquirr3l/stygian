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
    types::{Proxy, ProxyConfig, ProxyType},
};

// ─── Error response helpers ───────────────────────────────────────────────────

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn ok_response(id: &Value, result: Value) -> Value {
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
                    let is_well_formed_notification = req.get("id").is_none()
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
        let id = &req["id"];
        let method = req["method"].as_str().unwrap_or("");

        match method {
            "initialize" => self.handle_initialize(id),
            "initialized" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "notifications/initialized" | "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req).await,
            "resources/list" => self.handle_resources_list(id).await,
            "resources/read" => self.handle_resources_read(id, req).await,
            _ => error_response(id, -32601, &format!("Method not found: {method}")),
        }
    }

    fn handle_initialize(&self, id: &Value) -> Value {
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

    fn handle_tools_list(&self, id: &Value) -> Value {
        ok_response(
            id,
            json!({
                "tools": [
                    {
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
                    },
                    {
                        "name": "proxy_remove",
                        "description": "Remove a proxy from the pool by its UUID.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "proxy_id": { "type": "string", "description": "UUID of the proxy to remove (returned by proxy_add)" }
                            },
                            "required": ["proxy_id"]
                        }
                    },
                    {
                        "name": "proxy_pool_stats",
                        "description": "Return a health snapshot of the proxy pool: total count, healthy count, open circuit-breaker count, and active sticky-session count.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    },
                    {
                        "name": "proxy_acquire",
                        "description": "Lease one proxy from the pool using the configured rotation strategy. Returns a handle_token (opaque string) and the proxy URL. Call proxy_release when done.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    },
                    {
                        "name": "proxy_acquire_for_domain",
                        "description": "Lease a proxy for a specific domain, honouring sticky-session policy. The same proxy is returned for repeated calls with the same domain during the TTL. Returns handle_token and proxy_url.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "domain": { "type": "string", "description": "Target domain (e.g. example.com)" }
                            },
                            "required": ["domain"]
                        }
                    },
                    {
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
                    }
                ]
            }),
        )
    }

    async fn handle_tools_call(&self, id: &Value, req: &Value) -> Value {
        let name = req["params"]["name"].as_str().unwrap_or("");
        let args = &req["params"]["arguments"];

        match name {
            "proxy_add" => self.tool_proxy_add(id, args).await,
            "proxy_remove" => self.tool_proxy_remove(id, args).await,
            "proxy_pool_stats" => self.tool_proxy_pool_stats(id).await,
            "proxy_acquire" => self.tool_proxy_acquire(id).await,
            "proxy_acquire_for_domain" => self.tool_proxy_acquire_for_domain(id, args).await,
            "proxy_release" => self.tool_proxy_release(id, args).await,
            _ => error_response(id, -32602, &format!("Unknown tool: {name}")),
        }
    }

    async fn handle_resources_list(&self, id: &Value) -> Value {
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
        let uri = req["params"]["uri"].as_str().unwrap_or("");
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

    async fn tool_proxy_add(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let proxy_type = {
            // Prefer the explicit `proxy_type` argument; otherwise infer from the URL scheme.
            let explicit = args["proxy_type"].as_str();
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
        let username = args["username"].as_str().map(str::to_string);
        let password = args["password"].as_str().map(str::to_string);
        let weight = match args.get("weight") {
            None => 1u32,
            Some(v) => match v.as_u64() {
                Some(w) if w <= u64::from(u32::MAX) => w as u32,
                Some(_) => {
                    return error_response(id, -32602, "Invalid parameter: weight out of range");
                }
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
        let Some(proxy_id_str) = args["proxy_id"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: proxy_id");
        };

        let proxy_id = match Uuid::parse_str(proxy_id_str) {
            Ok(u) => u,
            Err(_) => return error_response(id, -32602, "Invalid proxy_id: must be a UUID"),
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
        let Some(domain) = args["domain"].as_str() else {
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
        let Some(token) = args["handle_token"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: handle_token");
        };
        let success = args["success"].as_bool().unwrap_or(true);

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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn server_builds() {
        let _ = McpProxyServer::new().unwrap();
    }

    #[test]
    fn initialize_response_has_version() {
        let server = McpProxyServer::new().unwrap();
        let id = json!(1);
        let resp = server.handle_initialize(&id);
        assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(resp["result"]["serverInfo"]["name"], "stygian-proxy");
    }

    #[test]
    fn tools_list_has_all_tools() {
        let server = McpProxyServer::new().unwrap();
        let id = json!(1);
        let resp = server.handle_tools_list(&id);
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"proxy_add"));
        assert!(names.contains(&"proxy_remove"));
        assert!(names.contains(&"proxy_pool_stats"));
        assert!(names.contains(&"proxy_acquire"));
        assert!(names.contains(&"proxy_acquire_for_domain"));
        assert!(names.contains(&"proxy_release"));
    }

    #[tokio::test]
    async fn proxy_add_missing_url_returns_error() {
        let server = McpProxyServer::new().unwrap();
        let id = json!(1);
        let args = json!({});
        let resp = server.tool_proxy_add(&id, &args).await;
        assert!(resp["error"].is_object());
    }

    #[tokio::test]
    async fn pool_stats_returns_empty_on_fresh_manager() {
        let server = McpProxyServer::new().unwrap();
        let id = json!(1);
        let resp = server.tool_proxy_pool_stats(&id).await;
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["total"], 0);
    }

    #[tokio::test]
    async fn acquire_on_empty_pool_returns_error() {
        let server = McpProxyServer::new().unwrap();
        let id = json!(1);
        let resp = server.tool_proxy_acquire(&id).await;
        assert!(resp["error"].is_object(), "empty pool should return error");
    }
}
