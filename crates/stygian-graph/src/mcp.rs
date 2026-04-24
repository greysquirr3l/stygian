//! MCP (Model Context Protocol) server for graph-based scraping.
//!
//! Exposes `stygian-graph` scraping and pipeline capabilities as an MCP server
//! over stdin/stdout using the JSON-RPC 2.0 protocol. External tools (LLM
//! agents, IDE plugins) can scrape URLs, query REST/GraphQL APIs, parse feeds,
//! and execute full pipeline DAGs via the standardised MCP interface.
//!
//! ## Enabling
//!
//! ```toml
//! [dependencies]
//! stygian-graph = { version = "*", features = ["mcp"] }
//! ```
//!
//! ## Running the server
//!
//! Add `stygian-graph` as a dependency with the `mcp` feature and call
//! `McpGraphServer::run()` from your own binary:
//!
//! ```rust,no_run
//! use stygian_graph::mcp::McpGraphServer;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     McpGraphServer::run().await
//! }
//! ```
//!
//! ## Protocol
//!
//! Implements MCP 2025-11-25 over JSON-RPC 2.0 on stdin/stdout.
//!
//! | MCP Method | Description |
//! | ----------- | ------------- |
//! | `initialize` | Handshake, return server capabilities |
//! | `tools/list` | List available scraping and pipeline tools |
//! | `tools/call` | Execute a scraping or pipeline tool |
//!
//! ## Tools
//!
//! | Tool | Key Parameters | Returns |
//! | ------ | -------------- | ------- |
//! | `scrape` | `url`, `timeout_secs?`, `proxy_url?`, `rotate_ua?` | `data`, `metadata` |
//! | `scrape_rest` | `url`, `method?`, `auth?`, `query?`, `body?`, `headers?`, `pagination?`, `data_path?` | `data`, `metadata` |
//! | `scrape_graphql` | `url`, `query`, `variables?`, `auth?`, `data_path?` | `data`, `metadata` |
//! | `scrape_sitemap` | `url`, `max_depth?` | `data` (JSON array of entries), `metadata` |
//! | `scrape_rss` | `url` | `data` (JSON array of items), `metadata` |
//! | `pipeline_validate` | `toml` | `nodes`, `services`, `execution_order`, `valid` |
//! | `pipeline_run` | `toml`, `timeout_secs?` | per-node `outputs`, `skipped`, `errors` |

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(feature = "acquisition-runner")]
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

#[cfg(feature = "acquisition-runner")]
use stygian_browser::{
    AcquisitionMode, AcquisitionRequest, AcquisitionRunner, BrowserConfig, BrowserPool,
};

use crate::{
    adapters::{
        graphql::{GraphQlConfig, GraphQlService},
        http::{HttpAdapter, HttpConfig},
        rest_api::RestApiAdapter,
        rss_feed::RssFeedAdapter,
        sitemap::SitemapAdapter,
    },
    application::pipeline_parser::{NodeDecl, PipelineParser, ServiceDecl},
    ports::{ScrapingService, ServiceInput},
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
    let mut map = serde_json::Map::new();
    map.insert("jsonrpc".to_owned(), json!("2.0"));
    map.insert("id".to_owned(), id.clone());
    map.insert("result".to_owned(), result);
    Value::Object(map)
}

// ─── Server ───────────────────────────────────────────────────────────────────

/// MCP server exposing `stygian-graph` scraping and pipeline tools.
///
/// All tools are stateless — each invocation builds and runs the appropriate
/// adapter directly without maintaining any server-side session state.
///
/// # Example
///
/// ```no_run
/// use stygian_graph::mcp::McpGraphServer;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     McpGraphServer::run().await
/// }
/// ```
pub struct McpGraphServer;

impl McpGraphServer {
    /// Create a new MCP graph server.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run the MCP server, reading JSON-RPC requests from stdin and writing
    /// responses to stdout until EOF.
    ///
    /// # Errors
    ///
    /// Returns an `Err` if the underlying I/O fails unrecoverably.
    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        info!("stygian-graph MCP server starting");

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
                    let response = Self::handle(&req).await;
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

        info!("stygian-graph MCP server stopped");
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
    /// use stygian_graph::mcp::McpGraphServer;
    /// use serde_json::json;
    ///
    /// # tokio_test::block_on(async {
    /// let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    /// let resp = McpGraphServer::handle_request(&req).await;
    /// assert_eq!(
    ///     resp.pointer("/result/protocolVersion").and_then(serde_json::Value::as_str),
    ///     Some("2025-11-25")
    /// );
    /// # });
    /// ```
    pub async fn handle_request(req: &Value) -> Value {
        Self::handle(req).await
    }

    async fn handle(req: &Value) -> Value {
        let null = Value::Null;
        let id = req.get("id").unwrap_or(&null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        match method {
            "initialize" => Self::handle_initialize(id),
            "initialized" | "notifications/initialized" | "ping" => {
                json!({"jsonrpc":"2.0","id":id,"result":{}})
            }
            "tools/list" => Self::handle_tools_list(id),
            "tools/call" => Self::handle_tools_call(id, req).await,
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
                    "resources": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "stygian-graph",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    fn scraping_tool_defs() -> Vec<Value> {
        vec![
            json!({
                "name": "scrape",
                "description": "Fetch a URL with anti-bot UA rotation and retry logic. Returns raw HTML/JSON content and response metadata.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url":          { "type": "string",  "description": "Target URL" },
                        "timeout_secs": { "type": "integer", "description": "Request timeout in seconds (default: 30)" },
                        "proxy_url":    { "type": "string",  "description": "HTTP/SOCKS5 proxy URL (e.g. socks5://user:pass@host:1080)" },
                        "rotate_ua":    { "type": "boolean", "description": "Rotate User-Agent on each request (default: true)" }
                    },
                    "required": ["url"]
                }
            }),
            json!({
                "name": "scrape_rest",
                "description": "Call a REST/JSON API. Supports bearer/API-key auth, arbitrary HTTP methods, query parameters, request bodies, pagination, and response path extraction.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url":        { "type": "string", "description": "API endpoint URL" },
                        "method":     { "type": "string", "description": "HTTP method (GET, POST, PUT, PATCH, DELETE — default: GET)" },
                        "auth":       {
                            "type": "object",
                            "description": "Authentication config",
                            "properties": {
                                "type":  { "type": "string", "description": "bearer | api_key | basic | header" },
                                "token": { "type": "string", "description": "Token or credential value" },
                                "header":{ "type": "string", "description": "Custom header name (for type=header)" }
                            }
                        },
                        "query":      { "type": "object", "description": "URL query parameters as key-value pairs" },
                        "body":       { "type": "object", "description": "Request body (JSON)" },
                        "headers":    { "type": "object", "description": "Custom request headers" },
                        "pagination": {
                            "type": "object",
                            "description": "Pagination config",
                            "properties": {
                                "strategy":  { "type": "string", "description": "link_header | offset | cursor" },
                                "max_pages": { "type": "integer", "description": "Maximum pages to fetch (default: 1)" }
                            }
                        },
                        "data_path":  { "type": "string", "description": "Dot-separated JSON path to extract (e.g. data.items)" }
                    },
                    "required": ["url"]
                }
            }),
            json!({
                "name": "scrape_graphql",
                "description": "Execute a GraphQL query against any spec-compliant endpoint. Supports bearer/API-key auth, variables, and dot-path data extraction.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url":       { "type": "string", "description": "GraphQL endpoint URL" },
                        "query":     { "type": "string", "description": "GraphQL query or mutation string" },
                        "variables": { "type": "object", "description": "Query variables (JSON object)" },
                        "auth": {
                            "type": "object",
                            "description": "Auth config",
                            "properties": {
                                "kind":        { "type": "string", "description": "bearer | api_key | header | none" },
                                "token":       { "type": "string", "description": "Auth token or key" },
                                "header_name": { "type": "string", "description": "Custom header name (default: X-Api-Key)" }
                            }
                        },
                        "data_path":     { "type": "string", "description": "Dot-separated path to extract from response (e.g. data.countries)" },
                        "timeout_secs":  { "type": "integer", "description": "Request timeout in seconds (default: 30)" }
                    },
                    "required": ["url", "query"]
                }
            }),
            json!({
                "name": "scrape_sitemap",
                "description": "Parse a sitemap.xml or sitemap index and return all discovered URLs with their priorities and change frequencies.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url":       { "type": "string",  "description": "Sitemap URL (sitemap.xml or sitemap index)" },
                        "max_depth": { "type": "integer", "description": "Maximum sitemap index recursion depth (default: 5)" }
                    },
                    "required": ["url"]
                }
            }),
            json!({
                "name": "scrape_rss",
                "description": "Parse an RSS or Atom feed and return all entries as structured JSON.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "RSS/Atom feed URL" }
                    },
                    "required": ["url"]
                }
            }),
        ]
    }

    fn graph_tool_defs() -> Vec<Value> {
        vec![
            json!({
                "name": "pipeline_validate",
                "description": "Parse and validate a TOML pipeline definition without executing it. Returns the node list, service declarations, and computed execution order.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml": { "type": "string", "description": "TOML pipeline definition string" }
                    },
                    "required": ["toml"]
                }
            }),
            json!({
                "name": "pipeline_run",
                "description": "Parse, validate, and execute a TOML pipeline DAG. HTTP, REST, GraphQL, sitemap, and RSS nodes are executed. AI nodes and browser nodes without opt-in acquisition config are recorded in the skipped list.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml":         { "type": "string",  "description": "TOML pipeline definition string" },
                        "timeout_secs": { "type": "integer", "description": "Per-node timeout in seconds (default: 30)" }
                    },
                    "required": ["toml"]
                }
            }),
            json!({
                "name": "inspect",
                "description": "Get a complete snapshot of a pipeline's graph structure including nodes, edges, execution waves, critical path, and connectivity metrics.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml": { "type": "string", "description": "TOML pipeline definition string" }
                    },
                    "required": ["toml"]
                }
            }),
            json!({
                "name": "node_info",
                "description": "Get detailed information about a specific node in the pipeline graph, including its service type, depth, predecessors, and successors.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml":    { "type": "string", "description": "TOML pipeline definition string" },
                        "node_id": { "type": "string", "description": "Node ID to inspect" }
                    },
                    "required": ["toml", "node_id"]
                }
            }),
            json!({
                "name": "impact",
                "description": "Analyze what would be affected by changing a node. Returns all upstream dependencies and downstream dependents.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml":    { "type": "string", "description": "TOML pipeline definition string" },
                        "node_id": { "type": "string", "description": "Node ID to analyze impact for" }
                    },
                    "required": ["toml", "node_id"]
                }
            }),
            json!({
                "name": "query_nodes",
                "description": "Query nodes in the pipeline graph by various criteria: service type, root/leaf status, depth range, or ID pattern.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "toml":       { "type": "string",  "description": "TOML pipeline definition string" },
                        "service":    { "type": "string",  "description": "Filter by service type (http, ai, browser, etc.)" },
                        "id_pattern": { "type": "string",  "description": "Filter by node ID substring match" },
                        "is_root":    { "type": "boolean", "description": "Only return root nodes (no predecessors)" },
                        "is_leaf":    { "type": "boolean", "description": "Only return leaf nodes (no successors)" },
                        "min_depth":  { "type": "integer", "description": "Minimum depth from root nodes" },
                        "max_depth":  { "type": "integer", "description": "Maximum depth from root nodes" }
                    },
                    "required": ["toml"]
                }
            }),
        ]
    }

    fn handle_tools_list(id: &Value) -> Value {
        let mut tools = Self::scraping_tool_defs();
        tools.extend(Self::graph_tool_defs());
        ok_response(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(id: &Value, req: &Value) -> Value {
        let null = Value::Null;
        let params = req.get("params").unwrap_or(&null);
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);

        match name {
            "scrape" => Self::tool_scrape(id, &args).await,
            "scrape_rest" => Self::tool_scrape_rest(id, &args).await,
            "scrape_graphql" => Self::tool_scrape_graphql(id, &args).await,
            "scrape_sitemap" => Self::tool_scrape_sitemap(id, &args).await,
            "scrape_rss" => Self::tool_scrape_rss(id, &args).await,
            "pipeline_validate" => Self::tool_pipeline_validate(id, &args),
            "pipeline_run" => Self::tool_pipeline_run(id, &args).await,
            "inspect" => Self::tool_graph_inspect(id, &args),
            "node_info" => Self::tool_graph_node_info(id, &args),
            "impact" => Self::tool_graph_impact(id, &args),
            "query_nodes" => Self::tool_graph_query(id, &args),
            _ => error_response(id, -32602, &format!("Unknown tool: {name}")),
        }
    }

    // ── scrape ───────────────────────────────────────────────────────────────

    async fn tool_scrape(id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(30);
        let proxy_url = args
            .get("proxy_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let rotate_ua = args
            .get("rotate_ua")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let config = HttpConfig {
            timeout: std::time::Duration::from_secs(timeout_secs),
            proxy_url,
            rotate_user_agent: rotate_ua,
            ..HttpConfig::default()
        };
        let adapter = HttpAdapter::with_config(config);
        let input = ServiceInput {
            url: url.to_string(),
            params: json!({}),
        };

        match adapter.execute(input).await {
            Ok(output) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "data": output.data,
                            "metadata": output.metadata
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Scrape failed: {e}")),
        }
    }

    // ── scrape_rest ──────────────────────────────────────────────────────────

    async fn tool_scrape_rest(id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        // Build params JSON from explicit fields only; extra keys in `args` are intentionally
        // not forwarded — the REST adapter only reads the fields it recognises.
        let mut map = serde_json::Map::new();
        if let Some(method) = args.get("method").and_then(Value::as_str) {
            map.insert("method".to_owned(), json!(method));
        }
        if let Some(auth) = args.get("auth").filter(|v| !v.is_null()) {
            map.insert("auth".to_owned(), auth.clone());
        }
        if let Some(query) = args.get("query").filter(|v| !v.is_null()) {
            map.insert("query".to_owned(), query.clone());
        }
        if let Some(body) = args.get("body").filter(|v| !v.is_null()) {
            map.insert("body".to_owned(), body.clone());
        }
        if let Some(headers) = args.get("headers").filter(|v| !v.is_null()) {
            map.insert("headers".to_owned(), headers.clone());
        }
        if let Some(pagination) = args.get("pagination").filter(|v| !v.is_null()) {
            map.insert("pagination".to_owned(), pagination.clone());
        }
        if let Some(dp) = args.get("data_path").and_then(Value::as_str) {
            map.insert("response".to_owned(), json!({ "data_path": dp }));
        }
        let params = Value::Object(map);

        let adapter = RestApiAdapter::new();
        let input = ServiceInput {
            url: url.to_string(),
            params,
        };

        match adapter.execute(input).await {
            Ok(output) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "data": output.data,
                            "metadata": output.metadata
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("REST scrape failed: {e}")),
        }
    }

    // ── scrape_graphql ───────────────────────────────────────────────────────

    async fn tool_scrape_graphql(id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };
        let Some(query) = args.get("query").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: query");
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(30);

        let config = GraphQlConfig {
            timeout_secs,
            ..GraphQlConfig::default()
        };
        let service = GraphQlService::new(config, None);

        let mut gql_map = serde_json::Map::new();
        gql_map.insert("query".to_owned(), json!(query));
        if let Some(variables) = args.get("variables").filter(|v| !v.is_null()) {
            gql_map.insert("variables".to_owned(), variables.clone());
        }
        if let Some(auth) = args.get("auth").filter(|v| !v.is_null()) {
            gql_map.insert("auth".to_owned(), auth.clone());
        }
        if let Some(dp) = args.get("data_path").and_then(Value::as_str) {
            gql_map.insert("data_path".to_owned(), json!(dp));
        }
        let params = Value::Object(gql_map);

        let input = ServiceInput {
            url: url.to_string(),
            params,
        };

        match service.execute(input).await {
            Ok(output) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "data": output.data,
                            "metadata": output.metadata
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("GraphQL scrape failed: {e}")),
        }
    }

    // ── scrape_sitemap ───────────────────────────────────────────────────────

    async fn tool_scrape_sitemap(id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let max_depth = args
            .get("max_depth")
            .and_then(Value::as_u64)
            .map_or(5, |v| usize::try_from(v).unwrap_or(5));
        let client = reqwest::Client::new();
        let adapter = SitemapAdapter::new(client, max_depth);
        let input = ServiceInput {
            url: url.to_string(),
            params: json!({}),
        };

        match adapter.execute(input).await {
            Ok(output) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "data": output.data,
                            "metadata": output.metadata
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("Sitemap scrape failed: {e}")),
        }
    }

    // ── scrape_rss ───────────────────────────────────────────────────────────

    async fn tool_scrape_rss(id: &Value, args: &Value) -> Value {
        let Some(url) = args.get("url").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let client = reqwest::Client::new();
        let adapter = RssFeedAdapter::new(client);
        let input = ServiceInput {
            url: url.to_string(),
            params: json!({}),
        };

        match adapter.execute(input).await {
            Ok(output) => ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "data": output.data,
                            "metadata": output.metadata
                        })).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => error_response(id, -32603, &format!("RSS scrape failed: {e}")),
        }
    }

    // ── pipeline_validate ────────────────────────────────────────────────────

    fn tool_pipeline_validate(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return ok_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&json!({
                            "valid": false,
                            "error": e.to_string(),
                            "nodes": def.nodes.len(),
                            "services": def.services.len()
                        })).unwrap_or_default()
                    }]
                }),
            );
        }

        let order = match def.topological_order() {
            Ok(o) => o,
            Err(e) => return error_response(id, -32603, &format!("Topology error: {e}")),
        };

        let node_info: Vec<Value> = def
            .nodes
            .iter()
            .map(|n| {
                json!({
                    "name": n.name,
                    "service": n.service,
                    "url": n.url,
                    "depends_on": n.depends_on
                })
            })
            .collect();

        let svc_info: Vec<Value> = def
            .services
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "kind": s.kind,
                    "model": s.model
                })
            })
            .collect();

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "valid": true,
                        "node_count": def.nodes.len(),
                        "service_count": def.services.len(),
                        "execution_order": order,
                        "nodes": node_info,
                        "services": svc_info
                    })).unwrap_or_default()
                }]
            }),
        )
    }

    // ── pipeline_run ─────────────────────────────────────────────────────────

    async fn tool_pipeline_run(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(30);

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return error_response(id, -32603, &format!("Validation error: {e}"));
        }

        let order = match def.topological_order() {
            Ok(o) => o,
            Err(e) => return error_response(id, -32603, &format!("Topology error: {e}")),
        };

        let svc_kinds: HashMap<String, ServiceDecl> = def
            .services
            .iter()
            .map(|s| (s.name.clone(), s.clone()))
            .collect();

        let mut outputs: HashMap<String, Value> = HashMap::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut errors: HashMap<String, String> = HashMap::new();

        for node_name in &order {
            let Some(node) = def.nodes.iter().find(|n| n.name == *node_name) else {
                continue;
            };

            let kind = svc_kinds
                .get(&node.service)
                .map_or(node.service.as_str(), |s| s.kind.as_str());

            // Nodes without a URL are AI/transform nodes — skip.
            let Some(url) = node.url.as_deref() else {
                skipped.push(node_name.clone());
                continue;
            };

            match execute_pipeline_node(kind, url, node_name, node, timeout_secs).await {
                Some(Ok(out)) => {
                    outputs.insert(node_name.clone(), out);
                }
                Some(Err(e)) => {
                    errors.insert(node_name.clone(), e);
                }
                None => {
                    skipped.push(node_name.clone());
                }
            }
        }

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "execution_order": order,
                        "outputs": outputs,
                        "skipped": skipped,
                        "errors": errors
                    })).unwrap_or_default()
                }]
            }),
        )
    }

    // ── Graph introspection tools ─────────────────────────────────────────────

    fn tool_graph_inspect(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return error_response(id, -32603, &format!("Validation error: {e}"));
        }

        // Build a Pipeline from the definition
        let mut pipeline = crate::domain::graph::Pipeline::new("pipeline");
        for node in &def.nodes {
            pipeline.add_node(crate::domain::graph::Node::with_metadata(
                &node.name,
                &node.service,
                serde_json::json!({
                    "url": node.url,
                    "params": toml_to_json(&toml::Value::Table(
                        node.params.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    ))
                }),
                serde_json::Value::Null,
            ));
            for dep in &node.depends_on {
                pipeline.add_edge(crate::domain::graph::Edge::new(dep, &node.name));
            }
        }

        let executor = match crate::domain::graph::DagExecutor::from_pipeline(&pipeline) {
            Ok(e) => e,
            Err(e) => return error_response(id, -32603, &format!("Graph build error: {e}")),
        };

        let snapshot = executor.snapshot();

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&snapshot).unwrap_or_default()
                }]
            }),
        )
    }

    fn tool_graph_node_info(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };
        let Some(node_id) = args.get("node_id").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: node_id");
        };

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return error_response(id, -32603, &format!("Validation error: {e}"));
        }

        let mut pipeline = crate::domain::graph::Pipeline::new("pipeline");
        for node in &def.nodes {
            pipeline.add_node(crate::domain::graph::Node::with_metadata(
                &node.name,
                &node.service,
                serde_json::json!({
                    "url": node.url,
                    "params": toml_to_json(&toml::Value::Table(
                        node.params.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    ))
                }),
                serde_json::Value::Null,
            ));
            for dep in &node.depends_on {
                pipeline.add_edge(crate::domain::graph::Edge::new(dep, &node.name));
            }
        }

        let executor = match crate::domain::graph::DagExecutor::from_pipeline(&pipeline) {
            Ok(e) => e,
            Err(e) => return error_response(id, -32603, &format!("Graph build error: {e}")),
        };

        executor.node_info(node_id).map_or_else(
            || error_response(id, -32602, &format!("Node not found: {node_id}")),
            |info| {
                ok_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&info).unwrap_or_default()
                        }]
                    }),
                )
            },
        )
    }

    fn tool_graph_impact(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };
        let Some(node_id) = args.get("node_id").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: node_id");
        };

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return error_response(id, -32603, &format!("Validation error: {e}"));
        }

        let mut pipeline = crate::domain::graph::Pipeline::new("pipeline");
        for node in &def.nodes {
            pipeline.add_node(crate::domain::graph::Node::with_metadata(
                &node.name,
                &node.service,
                serde_json::json!({
                    "url": node.url,
                    "params": toml_to_json(&toml::Value::Table(
                        node.params.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    ))
                }),
                serde_json::Value::Null,
            ));
            for dep in &node.depends_on {
                pipeline.add_edge(crate::domain::graph::Edge::new(dep, &node.name));
            }
        }

        let executor = match crate::domain::graph::DagExecutor::from_pipeline(&pipeline) {
            Ok(e) => e,
            Err(e) => return error_response(id, -32603, &format!("Graph build error: {e}")),
        };

        let impact = executor.impact_analysis(node_id);

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&impact).unwrap_or_default()
                }]
            }),
        )
    }

    fn tool_graph_query(id: &Value, args: &Value) -> Value {
        let Some(toml) = args.get("toml").and_then(Value::as_str) else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };

        let def = match PipelineParser::from_str(toml) {
            Ok(d) => d,
            Err(e) => return error_response(id, -32603, &format!("Parse error: {e}")),
        };

        if let Err(e) = def.validate() {
            return error_response(id, -32603, &format!("Validation error: {e}"));
        }

        let mut pipeline = crate::domain::graph::Pipeline::new("pipeline");
        for node in &def.nodes {
            pipeline.add_node(crate::domain::graph::Node::with_metadata(
                &node.name,
                &node.service,
                serde_json::json!({
                    "url": node.url,
                    "params": toml_to_json(&toml::Value::Table(
                        node.params.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    ))
                }),
                serde_json::Value::Null,
            ));
            for dep in &node.depends_on {
                pipeline.add_edge(crate::domain::graph::Edge::new(dep, &node.name));
            }
        }

        let executor = match crate::domain::graph::DagExecutor::from_pipeline(&pipeline) {
            Ok(e) => e,
            Err(e) => return error_response(id, -32603, &format!("Graph build error: {e}")),
        };

        // Build the query from args
        let query = crate::domain::introspection::NodeQuery {
            service: args
                .get("service")
                .and_then(Value::as_str)
                .map(String::from),
            id: None,
            id_pattern: args
                .get("id_pattern")
                .and_then(Value::as_str)
                .map(String::from),
            is_root: args.get("is_root").and_then(Value::as_bool),
            is_leaf: args.get("is_leaf").and_then(Value::as_bool),
            min_depth: args
                .get("min_depth")
                .and_then(Value::as_u64)
                .map(|v| usize::try_from(v).unwrap_or(0)),
            max_depth: args
                .get("max_depth")
                .and_then(Value::as_u64)
                .map(|v| usize::try_from(v).unwrap_or(0)),
        };

        let results = executor.query_nodes(&query);

        ok_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&results).unwrap_or_default()
                }]
            }),
        )
    }
}

impl Default for McpGraphServer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Build a [`GraphQlService`] and the corresponding [`ServiceInput`] from a pipeline node.
///
/// Extracts `query`, `variables`, `auth`, and `data_path` from the node's TOML params.
fn build_graphql_node_request(
    node: &NodeDecl,
    url: &str,
    timeout_secs: u64,
) -> (GraphQlService, ServiceInput) {
    let query = node
        .params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let config = GraphQlConfig {
        timeout_secs,
        ..GraphQlConfig::default()
    };
    let service = GraphQlService::new(config, None);
    let mut gql_map = serde_json::Map::new();
    gql_map.insert("query".to_owned(), json!(query));
    if let Some(variables) = node.params.get("variables") {
        gql_map.insert("variables".to_owned(), toml_to_json(variables));
    }
    if let Some(auth) = node.params.get("auth") {
        gql_map.insert("auth".to_owned(), toml_to_json(auth));
    }
    if let Some(dp) = node.params.get("data_path").and_then(|v| v.as_str()) {
        gql_map.insert("data_path".to_owned(), json!(dp));
    }
    (
        service,
        ServiceInput {
            url: url.to_string(),
            params: Value::Object(gql_map),
        },
    )
}

#[derive(Debug, Clone, PartialEq)]
struct AcquisitionNodeConfig {
    mode: String,
    wait_for_selector: Option<String>,
    extraction_js: Option<String>,
    total_timeout: Option<Duration>,
}

fn parse_optional_positive_secs(value: &toml::Value) -> Result<Duration, String> {
    const MAX_ACQUISITION_TIMEOUT_SECS: u64 = 86_400;

    if let Some(seconds) = value.as_float() {
        if seconds.is_finite() && seconds > 0.0 && seconds <= MAX_ACQUISITION_TIMEOUT_SECS as f64
        {
            return Ok(Duration::from_secs_f64(seconds));
        }
        return Err(format!(
            "acquisition.total_timeout_secs must be a positive finite number <= {MAX_ACQUISITION_TIMEOUT_SECS}"
        ));
    }

    if let Some(seconds) = value.as_integer() {
        if seconds > 0 && seconds <= i64::try_from(MAX_ACQUISITION_TIMEOUT_SECS).unwrap_or(i64::MAX)
        {
            return Ok(Duration::from_secs(
                u64::try_from(seconds).map_err(|_| {
                    "acquisition.total_timeout_secs must fit into an unsigned integer".to_string()
                })?,
            ));
        }
        return Err(format!(
            "acquisition.total_timeout_secs must be an integer in 1..={MAX_ACQUISITION_TIMEOUT_SECS}"
        ));
    }

    Err("acquisition.total_timeout_secs must be a number".to_string())
}

fn acquisition_config_from_node(node: &NodeDecl) -> Result<Option<AcquisitionNodeConfig>, String> {
    let Some(raw) = node.params.get("acquisition") else {
        return Ok(None);
    };

    let table = raw
        .as_table()
        .ok_or_else(|| "acquisition must be a TOML table".to_string())?;

    let enabled = table
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);

    if !enabled {
        return Ok(None);
    }

    let mode = table
        .get("mode")
        .and_then(toml::Value::as_str)
        .unwrap_or("resilient")
        .to_string();

    let wait_for_selector = table
        .get("wait_for_selector")
        .or_else(|| table.get("selector_wait"))
        .and_then(toml::Value::as_str)
        .map(ToString::to_string);

    let extraction_js = table
        .get("extraction_js")
        .and_then(toml::Value::as_str)
        .map(ToString::to_string);

    let total_timeout = table
        .get("total_timeout_secs")
        .map(parse_optional_positive_secs)
        .transpose()?;

    Ok(Some(AcquisitionNodeConfig {
        mode,
        wait_for_selector,
        extraction_js,
        total_timeout,
    }))
}

#[cfg(feature = "acquisition-runner")]
fn parse_acquisition_mode(raw: &str) -> Result<AcquisitionMode, String> {
    match raw {
        "fast" => Ok(AcquisitionMode::Fast),
        "resilient" => Ok(AcquisitionMode::Resilient),
        "hostile" => Ok(AcquisitionMode::Hostile),
        "investigate" => Ok(AcquisitionMode::Investigate),
        other => Err(format!(
            "Invalid acquisition mode '{other}'. Use one of: fast, resilient, hostile, investigate"
        )),
    }
}

#[cfg(feature = "acquisition-runner")]
static ACQUISITION_BRIDGE_POOL: OnceCell<Arc<BrowserPool>> = OnceCell::const_new();

#[cfg(feature = "acquisition-runner")]
async fn acquisition_bridge_pool() -> Result<Arc<BrowserPool>, String> {
    let pool = ACQUISITION_BRIDGE_POOL
        .get_or_try_init(|| async {
            BrowserPool::new(BrowserConfig::default())
                .await
                .map_err(|e| format!("acquisition bridge browser pool init failed: {e}"))
        })
        .await?;
    Ok(Arc::clone(pool))
}

#[cfg(feature = "acquisition-runner")]
async fn run_acquisition_bridge(url: &str, cfg: &AcquisitionNodeConfig) -> Result<Value, String> {
    let mode = parse_acquisition_mode(&cfg.mode)?;
    let pool = acquisition_bridge_pool().await?;

    let runner = AcquisitionRunner::new(pool);
    let request = AcquisitionRequest {
        url: url.to_string(),
        mode,
        wait_for_selector: cfg.wait_for_selector.clone(),
        extraction_js: cfg.extraction_js.clone(),
        total_timeout: cfg
            .total_timeout
            .unwrap_or_else(|| AcquisitionRequest::default().total_timeout),
        ..AcquisitionRequest::default()
    };

    let result = runner.run(request).await;
    let strategy_used = serde_json::to_value(result.strategy_used).unwrap_or(Value::Null);
    let attempted = serde_json::to_value(&result.attempted).unwrap_or(Value::Array(Vec::new()));
    let failures = serde_json::to_value(&result.failures).unwrap_or(Value::Array(Vec::new()));

    Ok(json!({
        "data": {
            "success": result.success,
            "strategy_used": strategy_used,
            "final_url": result.final_url,
            "status_code": result.status_code,
            "extracted": result.extracted,
            "html_excerpt": result.html_excerpt,
        },
        "metadata": {
            "acquisition_runner": true,
            "diagnostics": {
                "attempted": attempted,
                "timed_out": result.timed_out,
                "failure_count": result.failures.len(),
                "failures": failures,
            }
        }
    }))
}

#[cfg(not(feature = "acquisition-runner"))]
async fn run_acquisition_bridge(_url: &str, _cfg: &AcquisitionNodeConfig) -> Result<Value, String> {
    Err(
        "acquisition bridge requested but stygian-graph was built without feature 'acquisition-runner'"
            .to_string(),
    )
}

/// Execute a single pipeline node of a given service kind.
///
/// Returns `None` if the kind is not supported (node is skipped);
/// returns `Some(Ok(value))` on success or `Some(Err(message))` on failure.
async fn execute_pipeline_node(
    kind: &str,
    url: &str,
    node_name: &str,
    node: &NodeDecl,
    timeout_secs: u64,
) -> Option<Result<Value, String>> {
    execute_pipeline_node_with(
        kind,
        url,
        node_name,
        node,
        timeout_secs,
        |bridge_url, cfg| async move { run_acquisition_bridge(&bridge_url, &cfg).await },
    )
    .await
}

async fn execute_pipeline_node_with<F, Fut>(
    kind: &str,
    url: &str,
    node_name: &str,
    node: &NodeDecl,
    timeout_secs: u64,
    run_acquisition: F,
) -> Option<Result<Value, String>>
where
    F: Fn(String, AcquisitionNodeConfig) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, String>> + Send,
{
    match kind {
        "http" => {
            let config = HttpConfig {
                timeout: Duration::from_secs(timeout_secs),
                ..HttpConfig::default()
            };
            let adapter = HttpAdapter::with_config(config);
            let input = ServiceInput {
                url: url.to_string(),
                params: json!({}),
            };
            Some(
                adapter
                    .execute(input)
                    .await
                    .map(|out| json!({ "data": out.data, "metadata": out.metadata }))
                    .map_err(|e| e.to_string()),
            )
        }
        "rest" => {
            let params = build_rest_params_from_node(node);
            let adapter = RestApiAdapter::new();
            let input = ServiceInput {
                url: url.to_string(),
                params,
            };
            Some(
                adapter
                    .execute(input)
                    .await
                    .map(|out| json!({ "data": out.data, "metadata": out.metadata }))
                    .map_err(|e| e.to_string()),
            )
        }
        "graphql" => {
            let (service, input) = build_graphql_node_request(node, url, timeout_secs);
            Some(
                service
                    .execute(input)
                    .await
                    .map(|out| json!({ "data": out.data, "metadata": out.metadata }))
                    .map_err(|e| e.to_string()),
            )
        }
        "sitemap" => {
            let max_depth = node
                .params
                .get("max_depth")
                .and_then(toml::Value::as_integer)
                .map_or(5, |v| usize::try_from(v).unwrap_or(5));
            let client = reqwest::Client::new();
            let adapter = SitemapAdapter::new(client, max_depth);
            let input = ServiceInput {
                url: url.to_string(),
                params: json!({}),
            };
            Some(
                adapter
                    .execute(input)
                    .await
                    .map(|out| json!({ "data": out.data, "metadata": out.metadata }))
                    .map_err(|e| e.to_string()),
            )
        }
        "rss" => {
            let client = reqwest::Client::new();
            let adapter = RssFeedAdapter::new(client);
            let input = ServiceInput {
                url: url.to_string(),
                params: json!({}),
            };
            Some(
                adapter
                    .execute(input)
                    .await
                    .map(|out| json!({ "data": out.data, "metadata": out.metadata }))
                    .map_err(|e| e.to_string()),
            )
        }
        "browser" => execute_browser_pipeline_node(node, node_name, url, &run_acquisition).await,
        other => {
            warn!(
                kind = other,
                node = node_name,
                "skipping unsupported service kind in pipeline_run"
            );
            None
        }
    }
}

async fn execute_browser_pipeline_node<F, Fut>(
    node: &NodeDecl,
    node_name: &str,
    url: &str,
    run_acquisition: &F,
) -> Option<Result<Value, String>>
where
    F: Fn(String, AcquisitionNodeConfig) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, String>> + Send,
{
    let cfg = match acquisition_config_from_node(node) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return None,
        Err(err) => {
            return Some(Err(format!(
                "Invalid acquisition config for node '{node_name}': {err}"
            )));
        }
    };

    Some(run_acquisition(url.to_string(), cfg).await)
}

/// Convert a [`toml::Value`] to a [`serde_json::Value`] for adapter params.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number)
        }
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => Value::Object(
            tbl.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
    }
}

/// Build a `RestApiAdapter`-compatible `params` JSON value from a pipeline [`NodeDecl`].
///
/// Forwards all recognised REST parameters from the node's TOML declaration:
/// `method`, `auth`, `headers`, `query`, `body`, `pagination`, and `data_path`.
fn build_rest_params_from_node(node: &NodeDecl) -> Value {
    let mut map = serde_json::Map::new();

    if let Some(method) = node.params.get("method").and_then(|v| v.as_str()) {
        map.insert("method".to_owned(), json!(method));
    }
    if let Some(auth) = node.params.get("auth") {
        map.insert("auth".to_owned(), toml_to_json(auth));
    }
    if let Some(headers) = node.params.get("headers") {
        map.insert("headers".to_owned(), toml_to_json(headers));
    }
    if let Some(query) = node.params.get("query") {
        map.insert("query".to_owned(), toml_to_json(query));
    }
    if let Some(body) = node.params.get("body") {
        map.insert("body".to_owned(), toml_to_json(body));
    }
    if let Some(pagination) = node.params.get("pagination") {
        map.insert("pagination".to_owned(), toml_to_json(pagination));
    }
    if let Some(dp) = node.params.get("data_path").and_then(|v| v.as_str()) {
        map.insert("response".to_owned(), json!({ "data_path": dp }));
    }

    Value::Object(map)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn server_builds() {
        let _ = McpGraphServer::new();
    }

    #[test]
    fn initialize_response_contains_version() {
        let id = json!(1);
        let resp = McpGraphServer::handle_initialize(&id);
        assert_eq!(
            resp.pointer("/result/protocolVersion")
                .and_then(Value::as_str),
            Some("2025-11-25")
        );
    }

    #[test]
    fn tools_list_contains_all_tools() {
        let id = json!(1);
        let resp = McpGraphServer::handle_tools_list(&id);
        let tools = resp
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t.get("name").and_then(Value::as_str).unwrap())
            .collect();
        assert!(names.contains(&"scrape"));
        assert!(names.contains(&"scrape_rest"));
        assert!(names.contains(&"scrape_graphql"));
        assert!(names.contains(&"scrape_sitemap"));
        assert!(names.contains(&"scrape_rss"));
        assert!(names.contains(&"pipeline_validate"));
        assert!(names.contains(&"pipeline_run"));
    }

    #[test]
    fn pipeline_validate_rejects_bad_toml() {
        let id = json!(1);
        let args = json!({ "toml": "this is not valid toml [[[[" });
        let resp = McpGraphServer::tool_pipeline_validate(&id, &args);
        assert!(
            resp.get("error").is_some_and(Value::is_object)
                || resp
                    .pointer("/result/content/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .contains("false")
        );
    }

    #[test]
    fn pipeline_validate_accepts_valid_pipeline() {
        let id = json!(1);
        let toml = r#"
[[nodes]]
name = "fetch"
service = "http"
url = "https://example.com"

[[nodes]]
name = "process"
service = "http"
url = "https://example.com/api"
depends_on = ["fetch"]
"#;
        let args = json!({ "toml": toml });
        let resp = McpGraphServer::tool_pipeline_validate(&id, &args);
        let text = resp
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.get("valid"), Some(&json!(true)));
        assert_eq!(parsed.get("node_count"), Some(&json!(2)));
    }

    #[test]
    fn pipeline_validate_missing_toml_returns_error() {
        // Test is sync — just check param validation path
        let id = json!(1);
        let args = json!({});
        // We can't await in a non-async test context without a runtime,
        // so just verify the tool_pipeline_validate path for missing param.
        let resp = McpGraphServer::tool_pipeline_validate(&id, &args);
        assert!(resp.get("error").is_some_and(Value::is_object));
    }

    #[tokio::test]
    async fn pipeline_browser_node_without_acquisition_is_skipped() {
        let node = NodeDecl {
            name: "render".to_string(),
            service: "browser".to_string(),
            depends_on: Vec::new(),
            url: Some("https://example.com".to_string()),
            params: HashMap::new(),
        };

        let result = execute_pipeline_node_with(
            "browser",
            "https://example.com",
            "render",
            &node,
            30,
            |_url, _cfg| async { Ok(json!({"data": "should-not-run"})) },
        )
        .await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn pipeline_browser_node_with_acquisition_uses_bridge_path() {
        let mut acquisition = toml::map::Map::new();
        acquisition.insert("mode".to_string(), toml::Value::String("fast".to_string()));
        acquisition.insert(
            "wait_for_selector".to_string(),
            toml::Value::String("main".to_string()),
        );

        let mut params = HashMap::new();
        params.insert("acquisition".to_string(), toml::Value::Table(acquisition));

        let node = NodeDecl {
            name: "render".to_string(),
            service: "browser".to_string(),
            depends_on: Vec::new(),
            url: Some("https://example.com".to_string()),
            params,
        };

        let result = execute_pipeline_node_with(
            "browser",
            "https://example.com",
            "render",
            &node,
            30,
            |url, cfg| async move {
                Ok(json!({
                    "data": {
                        "url": url,
                        "mode": cfg.mode,
                        "wait_for_selector": cfg.wait_for_selector,
                    },
                    "metadata": {"bridge": "mock"}
                }))
            },
        )
        .await;

        let payload = match result {
            Some(Ok(payload)) => payload,
            other => {
                assert!(
                    matches!(other, Some(Ok(_))),
                    "browser acquisition should return Some(Ok(_))"
                );
                return;
            }
        };

        assert_eq!(
            payload.pointer("/data/url").and_then(Value::as_str),
            Some("https://example.com")
        );
        assert_eq!(
            payload.pointer("/data/mode").and_then(Value::as_str),
            Some("fast")
        );
        assert_eq!(
            payload
                .pointer("/data/wait_for_selector")
                .and_then(Value::as_str),
            Some("main")
        );
    }

    #[tokio::test]
    async fn pipeline_browser_node_invalid_acquisition_timeout_returns_error() {
        let mut acquisition = toml::map::Map::new();
        acquisition.insert("mode".to_string(), toml::Value::String("fast".to_string()));
        acquisition.insert("total_timeout_secs".to_string(), toml::Value::Integer(0));

        let mut params = HashMap::new();
        params.insert("acquisition".to_string(), toml::Value::Table(acquisition));

        let node = NodeDecl {
            name: "render".to_string(),
            service: "browser".to_string(),
            depends_on: Vec::new(),
            url: Some("https://example.com".to_string()),
            params,
        };

        let result = execute_pipeline_node_with(
            "browser",
            "https://example.com",
            "render",
            &node,
            30,
            |_url, _cfg| async { Ok(json!({"data": "unexpected"})) },
        )
        .await;

        let err = match result {
            Some(Err(err)) => err,
            other => {
                assert!(
                    matches!(other, Some(Err(_))),
                    "invalid config should return Some(Err(_))"
                );
                return;
            }
        };

        assert!(
            err.contains("total_timeout_secs") || err.contains("Invalid acquisition config"),
            "unexpected error: {err}"
        );
    }
}
