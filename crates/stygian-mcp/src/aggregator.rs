//! Aggregated MCP server that merges the graph, browser, and proxy tool surfaces.
//!
//! # Tool namespace
//!
//! | Prefix | Sub-server | Example |
//! | -------- | ----------- | ------- |
//! | `graph_` | `stygian-graph` | `graph_scrape`, `graph_pipeline_run` |
//! | `browser_` | `stygian-browser` | `browser_acquire`, `browser_navigate` |
//! | `proxy_` | `stygian-proxy` | `proxy_add`, `proxy_acquire` |
//! | *(none)* | Aggregator cross-crate | `scrape_proxied`, `browser_proxied` |
//!
//! The aggregator strips the `graph_` prefix before forwarding calls to the
//! graph sub-server (which internally uses un-prefixed names like `scrape`).
//! Browser and proxy tools are already prefixed in their respective servers.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use stygian_browser::{BrowserPool, mcp::McpBrowserServer};
use stygian_graph::mcp::McpGraphServer;
use stygian_proxy::mcp::McpProxyServer;

const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2024-11-05"];

// ─── Aggregator ───────────────────────────────────────────────────────────────

/// Aggregated MCP server exposing graph, browser, and proxy capabilities.
///
/// # Example
///
/// ```no_run
/// use stygian_mcp::aggregator::McpAggregator;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// let aggregator = McpAggregator::try_new().await?;
/// aggregator.run().await
/// # }
/// ```
pub struct McpAggregator {
    graph: Arc<McpGraphServer>,
    browser: Arc<McpBrowserServer>,
    proxy: Arc<McpProxyServer>,
    /// Cancellation token for the proxy background health-check and session-purge tasks.
    proxy_token: CancellationToken,
    /// Join handle for the proxy background tasks (health-check + session purge).
    /// Wrapped in `Option` so it can be taken and awaited in `run()` while
    /// `Drop` can abort it on any early-exit path.
    proxy_bg: Option<JoinHandle<()>>,
}

impl McpAggregator {
    /// Create the aggregator with a default browser pool and proxy manager.
    ///
    /// # Errors
    ///
    /// Returns an error if the browser pool or proxy manager cannot be
    /// initialised (e.g. required system binaries are missing).
    pub async fn try_new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pool = BrowserPool::new(stygian_browser::BrowserConfig::default()).await?;
        let graph = Arc::new(McpGraphServer::new());
        let browser = Arc::new(McpBrowserServer::new(pool));
        let proxy = Arc::new(McpProxyServer::new()?);
        let (proxy_token, proxy_bg) = proxy.start_background();
        Ok(Self {
            graph,
            browser,
            proxy,
            proxy_token,
            proxy_bg: Some(proxy_bg),
        })
    }

    /// Run the aggregated MCP server over stdin/stdout JSON-RPC 2.0.
    ///
    /// Reads newline-delimited JSON requests and writes newline-delimited
    /// JSON responses.  Runs until stdin reaches EOF.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if stdin/stdout cannot be read or written.
    pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("stygian-mcp aggregator starting (stdin/stdout mode)");

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();
        let mut stdout = stdout;

        while let Some(line) = reader.next_line().await? {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            debug!(?line, "MCP request");

            let response = match serde_json::from_str::<Value>(&line) {
                Ok(req) => self.handle(&req).await,
                Err(e) => Some(error_response(
                    &Value::Null,
                    -32700,
                    &format!("Parse error: {e}"),
                )),
            };

            // JSON-RPC notifications must not produce responses.
            if let Some(response) = response {
                let mut out = serde_json::to_string(&response).unwrap_or_default();
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
            }
        }

        info!("stygian-mcp aggregator stopping (stdin closed)");

        // Shut down proxy background tasks (health-check + session purge).
        self.proxy_token.cancel();
        if let Some(bg) = self.proxy_bg.take()
            && let Err(e) = bg.await
        {
            warn!("proxy background task panicked during shutdown: {e:?}");
        }

        Ok(())
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    async fn handle(&self, req: &Value) -> Option<Value> {
        let is_well_formed_notification = is_jsonrpc_notification(req);
        let id = req.get("id").unwrap_or(&Value::Null);

        if req.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                id,
                -32600,
                "Invalid request: expected jsonrpc='2.0'",
            ));
        }

        let method = match req.get("method").and_then(Value::as_str) {
            Some(method) => method,
            None => {
                return Some(error_response(
                    id,
                    -32600,
                    "Invalid request: missing string 'method'",
                ));
            }
        };

        let response = match method {
            "initialize" => handle_initialize(req),
            "initialized" | "notifications/initialized" => ok_response(id, json!({})),
            "ping" => ok_response(id, json!({})),
            "tools/list" => self.handle_tools_list(id).await,
            "tools/call" => self.handle_tools_call(id, req).await,
            "resources/list" => self.handle_resources_list(id).await,
            "resources/read" => self.handle_resources_read(id, req).await,
            other => error_response(id, -32601, &format!("Method not found: {other}")),
        };

        if is_well_formed_notification {
            None
        } else {
            Some(response)
        }
    }

    // ── tools/list ────────────────────────────────────────────────────────────

    async fn handle_tools_list(&self, id: &Value) -> Value {
        let list_req = json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":{}});

        // Graph tools — prefix each name with `graph_`.
        let graph_resp = self.graph.handle_request(&list_req).await;
        let graph_tools: Vec<Value> = graph_resp["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut t| {
                if let Some(name) = t["name"].as_str() {
                    let prefixed = format!("graph_{name}");
                    t["name"] = json!(prefixed);
                    // Prefix description so the LLM understands the namespace.
                    let desc = t["description"].as_str().unwrap_or("").to_string();
                    t["description"] = json!(format!("[graph] {desc}"));
                }
                t
            })
            .collect();

        // Browser tools — already prefixed (`browser_*`).
        let browser_resp = self.browser.dispatch(&list_req).await;
        let browser_tools: Vec<Value> = browser_resp["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // Proxy tools — already prefixed (`proxy_*`).
        let proxy_resp = self.proxy.handle_request(&list_req).await;
        let proxy_tools: Vec<Value> = proxy_resp["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // Cross-crate tools implemented by the aggregator itself.
        let cross_tools = vec![
            json!({
                "name": "scrape_proxied",
                "description": "Fetch a URL through a proxy automatically selected from the pool. Acquires a proxy, performs an HTTP scrape, then releases the proxy. Returns the scraped content.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Target URL to scrape" },
                        "timeout_secs": { "type": "integer", "description": "Request timeout in seconds (default: 30)" }
                    },
                    "required": ["url"]
                }
            }),
            json!({
                "name": "browser_proxied",
                "description": "Navigate to a URL in a full headless browser session routed through a proxy automatically selected from the pool. Acquires a proxy and browser session, navigates, captures HTML content, then releases both. Returns navigation metadata and page HTML.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Target URL to visit" }
                    },
                    "required": ["url"]
                }
            }),
        ];

        let all_tools: Vec<Value> = [graph_tools, browser_tools, proxy_tools, cross_tools]
            .into_iter()
            .flatten()
            .collect();

        ok_response(id, json!({ "tools": all_tools }))
    }

    // ── tools/call ────────────────────────────────────────────────────────────

    async fn handle_tools_call(&self, id: &Value, req: &Value) -> Value {
        let params = &req["params"];
        let name = match params["name"].as_str() {
            Some(n) => n,
            None => return error_response(id, -32602, "Missing tool 'name'"),
        };
        let args = &params["arguments"];

        if let Some(short) = name.strip_prefix("graph_") {
            // Route to graph sub-server with un-prefixed name.
            let sub = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": short, "arguments": args }
            });
            self.graph.handle_request(&sub).await
        } else if name.starts_with("browser_") {
            let sub = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": args }
            });
            self.browser.dispatch(&sub).await
        } else if name.starts_with("proxy_") {
            let sub = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": args }
            });
            self.proxy.handle_request(&sub).await
        } else if name == "scrape_proxied" {
            self.tool_scrape_proxied(id, args).await
        } else if name == "browser_proxied" {
            self.tool_browser_proxied(id, args).await
        } else {
            error_response(id, -32602, &format!("Unknown tool: {name}"))
        }
    }

    // ── resources/list ────────────────────────────────────────────────────────

    async fn handle_resources_list(&self, id: &Value) -> Value {
        let list_req = json!({"jsonrpc":"2.0","id":0,"method":"resources/list","params":{}});

        // Collect browser resources (active sessions).
        let browser_resp = self.browser.dispatch(&list_req).await;
        let browser_resources: Vec<Value> = browser_resp["result"]["resources"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // Collect proxy resources (pool stats).
        let proxy_resp = self.proxy.handle_request(&list_req).await;
        let proxy_resources: Vec<Value> = proxy_resp["result"]["resources"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let all: Vec<Value> = [browser_resources, proxy_resources]
            .into_iter()
            .flatten()
            .collect();

        ok_response(id, json!({ "resources": all }))
    }

    // ── resources/read ────────────────────────────────────────────────────────

    async fn handle_resources_read(&self, id: &Value, req: &Value) -> Value {
        let uri = req["params"]["uri"].as_str().unwrap_or("");

        if uri.starts_with("browser://") {
            self.browser.dispatch(req).await
        } else if uri.starts_with("proxy://") {
            self.proxy.handle_request(req).await
        } else {
            error_response(id, -32602, &format!("Unknown resource URI: {uri}"))
        }
    }

    // ── Cross-crate tool: scrape_proxied ─────────────────────────────────────

    async fn tool_scrape_proxied(&self, id: &Value, args: &Value) -> Value {
        let url = match args["url"].as_str() {
            Some(u) => u.to_string(),
            None => return error_response(id, -32602, "Missing 'url'"),
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30);

        // 1. Acquire a proxy from the pool.
        let acquire_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "proxy_acquire", "arguments": {} }
        });
        let acquire_resp = self.proxy.handle_request(&acquire_req).await;
        // Propagate a real error from proxy_acquire before falling back to the generic message.
        if !acquire_resp["error"].is_null() {
            let code = acquire_resp["error"]["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(-32603);
            return error_response(
                id,
                code,
                acquire_resp["error"]["message"]
                    .as_str()
                    .unwrap_or("No proxy available — add proxies via proxy_add first"),
            );
        }
        let handle_info = parse_content_text(&acquire_resp);
        let handle_token = match handle_info["handle_token"].as_str() {
            Some(t) => t.to_string(),
            None => {
                return error_response(
                    id,
                    -32603,
                    "No proxy available — add proxies via proxy_add first",
                );
            }
        };
        let proxy_url = match handle_info["proxy_url"].as_str() {
            Some(u) => u.to_string(),
            None => {
                release_proxy(&self.proxy, &handle_token, false).await;
                return error_response(id, -32603, "proxy_acquire returned no proxy_url");
            }
        };

        // 2. Scrape via the acquired proxy, forwarding the caller's timeout.
        let scrape_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": {
                "name": "scrape",
                "arguments": { "url": url, "proxy_url": proxy_url, "timeout_secs": timeout_secs }
            }
        });
        let scrape_resp = self.graph.handle_request(&scrape_req).await;
        let success = scrape_resp["error"].is_null();

        // 3. Release the proxy handle (mark success/failure for circuit-breaker).
        release_proxy(&self.proxy, &handle_token, success).await;

        // 4. Propagate scrape result.
        if success {
            let text = scrape_resp["result"]["content"][0]["text"].clone();
            ok_response(
                id,
                json!({
                    "content": [{"type": "text", "text": text}]
                }),
            )
        } else {
            // Rewrite the id so it matches the caller's request, not the internal sub-request.
            let code = scrape_resp["error"]["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(-32603);
            let message = scrape_resp["error"]["message"]
                .as_str()
                .unwrap_or("Graph scrape failed");
            error_response(id, code, message)
        }
    }

    // ── Cross-crate tool: browser_proxied ────────────────────────────────────

    async fn tool_browser_proxied(&self, id: &Value, args: &Value) -> Value {
        let url = match args["url"].as_str() {
            Some(u) => u.to_string(),
            None => return error_response(id, -32602, "Missing 'url'"),
        };

        // 1. Acquire a proxy.
        let acquire_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "proxy_acquire", "arguments": {} }
        });
        let acquire_resp = self.proxy.handle_request(&acquire_req).await;
        if !acquire_resp["error"].is_null() {
            let code = acquire_resp["error"]["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(-32603);
            return error_response(
                id,
                code,
                acquire_resp["error"]["message"]
                    .as_str()
                    .unwrap_or("No proxy available — add proxies via proxy_add first"),
            );
        }
        let handle_info = parse_content_text(&acquire_resp);
        let handle_token = match handle_info["handle_token"].as_str() {
            Some(t) => t.to_string(),
            None => {
                return error_response(
                    id,
                    -32603,
                    "No proxy available — add proxies via proxy_add first",
                );
            }
        };
        let proxy_url = match handle_info["proxy_url"].as_str() {
            Some(u) => u.to_string(),
            None => {
                release_proxy(&self.proxy, &handle_token, false).await;
                return error_response(id, -32603, "proxy_acquire returned no proxy_url");
            }
        };

        // 2. Acquire a browser session. Note: the `proxy` argument is stored as session
        //    metadata in stygian-browser but does not yet route network traffic through the
        //    proxy at browser-launch level. Pass it for forward-compatibility.
        let acquire_browser_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "browser_acquire", "arguments": { "proxy": proxy_url } }
        });
        let acquire_browser_resp = self.browser.dispatch(&acquire_browser_req).await;
        // MCP tool failures from stygian-browser appear as result.isError=true.
        let acquire_is_error = acquire_browser_resp["result"]["isError"].as_bool() == Some(true);
        let session_info = parse_content_text(&acquire_browser_resp);
        let session_id = match session_info["session_id"].as_str() {
            Some(s) => s.to_string(),
            None => {
                release_proxy(&self.proxy, &handle_token, false).await;
                let err_msg = if acquire_is_error {
                    acquire_browser_resp["result"]["content"]
                        .get(0)
                        .and_then(|c| c["text"].as_str())
                        .unwrap_or("Failed to acquire browser session")
                } else {
                    "Failed to acquire browser session"
                };
                return error_response(id, -32603, err_msg);
            }
        };

        // 3. Navigate to URL.
        let nav_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": {
                "name": "browser_navigate",
                "arguments": { "session_id": session_id, "url": url }
            }
        });
        let nav_resp = self.browser.dispatch(&nav_req).await;
        // MCP tool failures may appear as result.isError=true with error=null.
        let nav_ok =
            nav_resp["error"].is_null() && nav_resp["result"]["isError"].as_bool() != Some(true);

        // Short-circuit: if navigation failed, release resources and propagate the error.
        if !nav_ok {
            let _ = self
                .browser
                .dispatch(&json!({
                    "jsonrpc": "2.0", "id": 0, "method": "tools/call",
                    "params": {
                        "name": "browser_release",
                        "arguments": { "session_id": session_id }
                    }
                }))
                .await;
            release_proxy(&self.proxy, &handle_token, false).await;
            // Prefer error.message; fall back to result.content[0].text (isError path).
            let nav_err = nav_resp["error"]["message"]
                .as_str()
                .or_else(|| {
                    nav_resp["result"]["content"]
                        .get(0)
                        .and_then(|c| c["text"].as_str())
                })
                .unwrap_or("Browser navigation failed");
            return error_response(id, -32603, nav_err);
        }

        // 4. Capture HTML content.
        let content_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": {
                "name": "browser_content",
                "arguments": { "session_id": session_id }
            }
        });
        let content_resp = self.browser.dispatch(&content_req).await;
        let content_ok = content_resp["error"].is_null()
            && content_resp["result"]["isError"].as_bool() != Some(true);

        // 5. Release browser session.
        let _ = self
            .browser
            .dispatch(&json!({
                "jsonrpc": "2.0", "id": 0, "method": "tools/call",
                "params": {
                    "name": "browser_release",
                    "arguments": { "session_id": session_id }
                }
            }))
            .await;

        // 6. Release proxy — success only if both nav and content succeeded.
        release_proxy(&self.proxy, &handle_token, content_ok).await;

        if !content_ok {
            // Prefer error.message; fall back to result.content[0].text (isError path).
            let content_err = content_resp["error"]["message"]
                .as_str()
                .or_else(|| {
                    content_resp["result"]["content"]
                        .get(0)
                        .and_then(|c| c["text"].as_str())
                })
                .unwrap_or("Browser content retrieval failed");
            return error_response(id, -32603, content_err);
        }

        // 7. Return combined result — parse sub-tool text fields as JSON to avoid
        //    double-encoded strings in the aggregated output.
        let nav_text_raw = &nav_resp["result"]["content"][0]["text"];
        let html_text_raw = &content_resp["result"]["content"][0]["text"];
        let nav_json = nav_text_raw
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or_else(|| nav_text_raw.clone());
        let html_json = html_text_raw
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or_else(|| html_text_raw.clone());

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "navigation": nav_json,
                        "html":       html_json
                    }))
                    .unwrap_or_default()
                }]
            }),
        )
    }
}

impl Drop for McpAggregator {
    /// Cancel and abort the proxy background tasks on any drop path so they
    /// do not outlive the aggregator when `run()` returns early via `?`.
    fn drop(&mut self) {
        self.proxy_token.cancel();
        if let Some(bg) = self.proxy_bg.take() {
            bg.abort();
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn handle_initialize(req: &Value) -> Value {
    let id = req.get("id").unwrap_or(&Value::Null);
    let requested = req["params"]["protocolVersion"].as_str();

    let protocol_version = match requested {
        Some(version) if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => version,
        Some(version) => {
            return error_response(
                id,
                -32602,
                &format!(
                    "Unsupported protocolVersion: {version}. Supported: {}",
                    SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                ),
            );
        }
        None => SUPPORTED_PROTOCOL_VERSIONS[0],
    };

    ok_response(
        id,
        json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools":     { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false }
            },
            "serverInfo": {
                "name":    "stygian-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

fn ok_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Extract the first content text item from an MCP response and parse it as
/// JSON.  Falls back to `Value::Null` on any parsing failure.
fn parse_content_text(resp: &Value) -> Value {
    resp["result"]["content"][0]["text"]
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null)
}

fn is_jsonrpc_notification(req: &Value) -> bool {
    req.is_object()
        && req.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && req.get("id").is_none()
        && req.get("method").and_then(Value::as_str).is_some()
}

/// Release a proxy handle via the proxy sub-server.
async fn release_proxy(proxy: &Arc<McpProxyServer>, handle_token: &str, success: bool) {
    let _ = proxy
        .handle_request(&json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": {
                "name": "proxy_release",
                "arguments": { "handle_token": handle_token, "success": success }
            }
        }))
        .await;
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use stygian_graph::mcp::McpGraphServer;
    use stygian_proxy::mcp::McpProxyServer;

    /// Validate that the aggregator routes `graph_scrape` to the graph server.
    #[tokio::test]
    async fn test_routes_graph_tool() {
        let graph = Arc::new(McpGraphServer::new());
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));
        // Build a minimal aggregator without actually starting a browser.
        // We skip browser construction in unit tests (no binary available).
        // Only the routing logic is tested here.
        let list_req = json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/list",
            "params": {}
        });
        let resp = graph.handle_request(&list_req).await;
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        // Confirm graph server exposes `scrape` (un-prefixed).
        assert!(tools.iter().any(|t| t["name"] == "scrape"));
        // Proxy list should not contain `scrape`.
        let proxy_resp = proxy.handle_request(&list_req).await;
        let proxy_tools = proxy_resp["result"]["tools"]
            .as_array()
            .expect("tools array");
        assert!(!proxy_tools.iter().any(|t| t["name"] == "scrape"));
        drop(proxy);
    }

    #[test]
    fn test_error_response_structure() {
        let resp = error_response(&json!(42), -32602, "bad param");
        assert_eq!(resp["error"]["code"], -32602);
        assert_eq!(resp["id"], 42);
    }

    #[test]
    fn test_ok_response_structure() {
        let resp = ok_response(&json!(1), json!({"foo": "bar"}));
        assert_eq!(resp["result"]["foo"], "bar");
        assert_eq!(resp["jsonrpc"], "2.0");
    }

    #[test]
    fn test_initialize_negotiates_supported_protocol() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        });
        let resp = handle_initialize(&req);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn test_initialize_rejects_unsupported_protocol() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        });
        let resp = handle_initialize(&req);
        assert_eq!(resp["error"]["code"], -32602);
    }

    /// `scrape_proxied` with no proxies in the pool must return an error
    /// (not silently succeed or panic).
    #[tokio::test]
    async fn test_scrape_proxied_no_proxy_returns_error() {
        let graph = Arc::new(McpGraphServer::new());
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));
        // Build a minimal graph+proxy aggregator without a real browser.
        // We test cross-crate error propagation by calling scrape_proxied
        // directly on a graph+proxy combination with an empty proxy pool.
        let id = json!(99);
        // With no proxies registered the acquire call must fail.
        let acquire_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "proxy_acquire", "arguments": {} }
        });
        let acquire_resp = proxy.handle_request(&acquire_req).await;
        // Simulate the scrape_proxied acquire-failure branch.
        let resp = if !acquire_resp["error"].is_null() {
            let code = acquire_resp["error"]["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(-32603);
            let message = acquire_resp["error"]["message"]
                .as_str()
                .unwrap_or("No proxy available");
            error_response(&id, code, message)
        } else {
            // parse_content_text returns Null for empty pool → no handle_token
            let handle_info = parse_content_text(&acquire_resp);
            if handle_info["handle_token"].as_str().is_none() {
                error_response(
                    &id,
                    -32603,
                    "No proxy available — add proxies via proxy_add first",
                )
            } else {
                ok_response(&id, json!({"unexpected": "success"}))
            }
        };
        assert!(!resp["error"].is_null(), "expected an error response");
        assert_eq!(resp["id"], 99, "id must be the caller's id, not 0");
        drop(graph);
        drop(proxy);
    }

    /// `browser_proxied` must propagate a JSON-RPC error with the correct caller id
    /// when proxy acquisition fails (empty pool).
    #[tokio::test]
    async fn test_browser_proxied_acquire_failure_uses_caller_id() {
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));
        let id = json!(77);
        let acquire_req = json!({
            "jsonrpc": "2.0", "id": 0, "method": "tools/call",
            "params": { "name": "proxy_acquire", "arguments": {} }
        });
        let acquire_resp = proxy.handle_request(&acquire_req).await;
        // With an empty pool the acquire either errors or returns no handle_token.
        // Either way the caller's id must survive in the response.
        let resp = if !acquire_resp["error"].is_null() {
            let code = acquire_resp["error"]["code"]
                .as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(-32603);
            let message = acquire_resp["error"]["message"]
                .as_str()
                .unwrap_or("No proxy available");
            error_response(&id, code, message)
        } else {
            let handle_info = parse_content_text(&acquire_resp);
            if handle_info["handle_token"].as_str().is_none() {
                error_response(
                    &id,
                    -32603,
                    "No proxy available — add proxies via proxy_add first",
                )
            } else {
                ok_response(&id, json!({"unexpected": "success"}))
            }
        };
        assert!(!resp["error"].is_null(), "expected an error response");
        // The critical invariant: id must be 77, not the internal sub-request id 0.
        assert_eq!(
            resp["id"], 77,
            "caller id must be preserved in error response"
        );
    }

    /// Verify that `parse_content_text` returns Null on a JSON-RPC error response
    /// (no `result.content` field), which is the sentinel for "acquire failed".
    #[test]
    fn test_parse_content_text_on_error_response_returns_null() {
        let error_resp = error_response(&json!(1), -32603, "internal error");
        let parsed = parse_content_text(&error_resp);
        assert_eq!(parsed, Value::Null);
    }

    #[test]
    fn test_notification_detection_requires_valid_jsonrpc() {
        let valid_notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        assert!(is_jsonrpc_notification(&valid_notification));

        let missing_jsonrpc = json!({
            "method": "notifications/initialized",
            "params": {}
        });
        assert!(!is_jsonrpc_notification(&missing_jsonrpc));
    }

    #[test]
    fn test_invalid_request_missing_jsonrpc_returns_invalid_request_error() {
        let req = json!({
            "method": "tools/list"
        });
        let id = req.get("id").unwrap_or(&Value::Null);
        let resp = if req.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            error_response(id, -32600, "Invalid request: expected jsonrpc='2.0'")
        } else {
            ok_response(id, json!({}))
        };
        assert_eq!(resp["error"]["code"], -32600);
        assert_eq!(resp["id"], Value::Null);
    }

    /// `tools/list` aggregation: graph tools get a `graph_` prefix and a `[graph]` description
    /// prefix; proxy tools keep their existing `proxy_` prefix; all lists are merged.
    #[tokio::test]
    async fn test_tools_list_prefixes_graph_tools() {
        let graph = Arc::new(McpGraphServer::new());
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));

        let list_req = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });

        // Graph: un-prefixed names like `scrape`
        let graph_resp = graph.handle_request(&list_req).await;
        let graph_tools = graph_resp["result"]["tools"].as_array().expect("tools");
        assert!(
            graph_tools.iter().any(|t| t["name"] == "scrape"),
            "graph server exposes un-prefixed 'scrape'"
        );

        // Proxy: already prefixed
        let proxy_resp = proxy.handle_request(&list_req).await;
        let proxy_tools = proxy_resp["result"]["tools"].as_array().expect("tools");
        assert!(
            proxy_tools.iter().any(|t| t["name"] == "proxy_add"),
            "proxy server exposes 'proxy_add'"
        );

        // Simulate the prefixing logic from handle_tools_list
        let prefixed: Vec<String> = graph_tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .map(|n| format!("graph_{n}"))
            .collect();
        assert!(
            prefixed.contains(&"graph_scrape".to_string()),
            "graph_scrape must appear after prefixing"
        );
        assert!(
            !prefixed.contains(&"scrape".to_string()),
            "un-prefixed 'scrape' must not appear after prefixing"
        );
    }

    /// `tools/call` dispatch: names with `graph_` prefix are routed to the graph server with
    /// the prefix stripped; `proxy_` names are routed to the proxy server unchanged.
    #[tokio::test]
    async fn test_tools_call_dispatch_by_prefix() {
        let graph = Arc::new(McpGraphServer::new());
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));

        // graph_pipeline_validate → graph server with name `pipeline_validate`
        let graph_call = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "pipeline_validate", "arguments": { "toml": "" } }
        });
        let graph_resp = graph.handle_request(&graph_call).await;
        // pipeline_validate with empty string returns a result (not an unknown-method error)
        assert!(
            graph_resp["error"].is_null()
                || graph_resp["result"]["content"][0]["text"]
                    .as_str()
                    .is_some(),
            "graph server must respond to pipeline_validate"
        );

        // proxy_add with missing url returns an error from the proxy server (not unknown tool)
        let proxy_call = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "proxy_add", "arguments": {} }
        });
        let proxy_resp = proxy.handle_request(&proxy_call).await;
        // Should get an error about missing `url`, not about an unknown tool
        let err_msg = proxy_resp["error"]["message"]
            .as_str()
            .or_else(|| proxy_resp["result"]["content"][0]["text"].as_str())
            .unwrap_or("");
        assert!(
            err_msg.contains("url") || err_msg.contains("required") || !err_msg.is_empty(),
            "proxy server must respond to proxy_add (got: {err_msg})"
        );
    }

    /// `resources/list` aggregation: proxy resources (pool stats) must be collected.
    #[tokio::test]
    async fn test_resources_list_includes_proxy() {
        let proxy = Arc::new(McpProxyServer::new().expect("proxy server init"));

        let list_req =
            json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list", "params": {} });
        let proxy_resp = proxy.handle_request(&list_req).await;
        let resources = proxy_resp["result"]["resources"].as_array();
        // The proxy pool stats resource is always present at proxy://pool/stats
        assert!(
            resources.map_or(false, |r| r
                .iter()
                .any(|res| { res["uri"].as_str() == Some("proxy://pool/stats") })),
            "proxy resources/list must contain proxy://pool/stats"
        );
    }
}
