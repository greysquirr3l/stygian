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
//! stygian-graph = { version = "0.6.0", features = ["mcp"] }
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
//!     McpGraphServer::new().run().await
//! }
//! ```
//!
//! ## Protocol
//!
//! Implements MCP 2024-11-05 over JSON-RPC 2.0 on stdin/stdout.
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

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

use crate::{
    adapters::{
        graphql::{GraphQlConfig, GraphQlService},
        http::{HttpAdapter, HttpConfig},
        rest_api::RestApiAdapter,
        rss_feed::RssFeedAdapter,
        sitemap::SitemapAdapter,
    },
    application::pipeline_parser::{PipelineParser, ServiceDecl},
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
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
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
///     McpGraphServer::new().run().await
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
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
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
                Ok(req) => self.handle(&req).await,
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
    /// let server = McpGraphServer::new();
    /// let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    /// let resp = server.handle_request(&req).await;
    /// assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    /// # });
    /// ```
    pub async fn handle_request(&self, req: &Value) -> Value {
        self.handle(req).await
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
            _ => error_response(id, -32601, &format!("Method not found: {method}")),
        }
    }

    fn handle_initialize(&self, id: &Value) -> Value {
        ok_response(
            id,
            json!({
                "protocolVersion": "2024-11-05",
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

    fn handle_tools_list(&self, id: &Value) -> Value {
        ok_response(
            id,
            json!({
                "tools": [
                    {
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
                    },
                    {
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
                    },
                    {
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
                    },
                    {
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
                    },
                    {
                        "name": "scrape_rss",
                        "description": "Parse an RSS or Atom feed and return all entries as structured JSON.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "url": { "type": "string", "description": "RSS/Atom feed URL" }
                            },
                            "required": ["url"]
                        }
                    },
                    {
                        "name": "pipeline_validate",
                        "description": "Parse and validate a TOML pipeline definition without executing it. Returns the node list, service declarations, and computed execution order.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "toml": { "type": "string", "description": "TOML pipeline definition string" }
                            },
                            "required": ["toml"]
                        }
                    },
                    {
                        "name": "pipeline_run",
                        "description": "Parse, validate, and execute a TOML pipeline DAG. HTTP, REST, GraphQL, sitemap, and RSS nodes are executed. AI/browser nodes are recorded in the skipped list.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "toml":         { "type": "string",  "description": "TOML pipeline definition string" },
                                "timeout_secs": { "type": "integer", "description": "Per-node timeout in seconds (default: 30)" }
                            },
                            "required": ["toml"]
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
            "scrape" => self.tool_scrape(id, args).await,
            "scrape_rest" => self.tool_scrape_rest(id, args).await,
            "scrape_graphql" => self.tool_scrape_graphql(id, args).await,
            "scrape_sitemap" => self.tool_scrape_sitemap(id, args).await,
            "scrape_rss" => self.tool_scrape_rss(id, args).await,
            "pipeline_validate" => self.tool_pipeline_validate(id, args),
            "pipeline_run" => self.tool_pipeline_run(id, args).await,
            _ => error_response(id, -32602, &format!("Unknown tool: {name}")),
        }
    }

    // ── scrape ───────────────────────────────────────────────────────────────

    async fn tool_scrape(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30);
        let proxy_url = args["proxy_url"].as_str().map(str::to_string);
        let rotate_ua = args["rotate_ua"].as_bool().unwrap_or(true);

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

    async fn tool_scrape_rest(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        // Build params JSON from explicit fields only; extra keys in `args` are intentionally
        // not forwarded — the REST adapter only reads the fields it recognises.
        let mut params = json!({});
        if let Some(method) = args["method"].as_str() {
            params["method"] = json!(method);
        }
        if !args["auth"].is_null() {
            params["auth"] = args["auth"].clone();
        }
        if !args["query"].is_null() {
            params["query"] = args["query"].clone();
        }
        if !args["body"].is_null() {
            params["body"] = args["body"].clone();
        }
        if !args["headers"].is_null() {
            params["headers"] = args["headers"].clone();
        }
        if !args["pagination"].is_null() {
            params["pagination"] = args["pagination"].clone();
        }
        if let Some(dp) = args["data_path"].as_str() {
            params["response"] = json!({ "data_path": dp });
        }

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

    async fn tool_scrape_graphql(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: url");
        };
        let Some(query) = args["query"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: query");
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30);

        let config = GraphQlConfig {
            timeout_secs,
            ..GraphQlConfig::default()
        };
        let service = GraphQlService::new(config, None);

        let mut params = json!({ "query": query });
        if !args["variables"].is_null() {
            params["variables"] = args["variables"].clone();
        }
        if !args["auth"].is_null() {
            params["auth"] = args["auth"].clone();
        }
        if let Some(dp) = args["data_path"].as_str() {
            params["data_path"] = json!(dp);
        }

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

    async fn tool_scrape_sitemap(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: url");
        };

        let max_depth = args["max_depth"].as_u64().unwrap_or(5) as usize;
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

    async fn tool_scrape_rss(&self, id: &Value, args: &Value) -> Value {
        let Some(url) = args["url"].as_str() else {
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

    fn tool_pipeline_validate(&self, id: &Value, args: &Value) -> Value {
        let Some(toml) = args["toml"].as_str() else {
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

    async fn tool_pipeline_run(&self, id: &Value, args: &Value) -> Value {
        let Some(toml) = args["toml"].as_str() else {
            return error_response(id, -32602, "Missing required parameter: toml");
        };

        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(30);

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

        // Build a service-kind lookup by service name.
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
                .map(|s| s.kind.as_str())
                .unwrap_or(node.service.as_str());

            // Nodes without a URL are AI/transform nodes — skip.
            let Some(url) = node.url.as_deref() else {
                skipped.push(node_name.clone());
                continue;
            };

            match kind {
                "http" => {
                    let config = HttpConfig {
                        timeout: std::time::Duration::from_secs(timeout_secs),
                        ..HttpConfig::default()
                    };
                    let adapter = HttpAdapter::with_config(config);
                    let input = ServiceInput {
                        url: url.to_string(),
                        params: json!({}),
                    };
                    match adapter.execute(input).await {
                        Ok(out) => {
                            outputs.insert(
                                node_name.clone(),
                                json!({ "data": out.data, "metadata": out.metadata }),
                            );
                        }
                        Err(e) => {
                            errors.insert(node_name.clone(), e.to_string());
                        }
                    }
                }
                "rest" => {
                    let params = build_rest_params_from_node(node);
                    let adapter = RestApiAdapter::new();
                    let input = ServiceInput {
                        url: url.to_string(),
                        params,
                    };
                    match adapter.execute(input).await {
                        Ok(out) => {
                            outputs.insert(
                                node_name.clone(),
                                json!({ "data": out.data, "metadata": out.metadata }),
                            );
                        }
                        Err(e) => {
                            errors.insert(node_name.clone(), e.to_string());
                        }
                    }
                }
                "graphql" => {
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
                    let mut gql_params = json!({ "query": query });
                    if let Some(variables) = node.params.get("variables") {
                        gql_params["variables"] = toml_to_json(variables);
                    }
                    if let Some(auth) = node.params.get("auth") {
                        gql_params["auth"] = toml_to_json(auth);
                    }
                    if let Some(dp) = node.params.get("data_path").and_then(|v| v.as_str()) {
                        gql_params["data_path"] = json!(dp);
                    }
                    let input = ServiceInput {
                        url: url.to_string(),
                        params: gql_params,
                    };
                    match service.execute(input).await {
                        Ok(out) => {
                            outputs.insert(
                                node_name.clone(),
                                json!({ "data": out.data, "metadata": out.metadata }),
                            );
                        }
                        Err(e) => {
                            errors.insert(node_name.clone(), e.to_string());
                        }
                    }
                }
                "sitemap" => {
                    let max_depth = node
                        .params
                        .get("max_depth")
                        .and_then(|v| v.as_integer())
                        .map_or(5, |v| v as usize);
                    let client = reqwest::Client::new();
                    let adapter = SitemapAdapter::new(client, max_depth);
                    let input = ServiceInput {
                        url: url.to_string(),
                        params: json!({}),
                    };
                    match adapter.execute(input).await {
                        Ok(out) => {
                            outputs.insert(
                                node_name.clone(),
                                json!({ "data": out.data, "metadata": out.metadata }),
                            );
                        }
                        Err(e) => {
                            errors.insert(node_name.clone(), e.to_string());
                        }
                    }
                }
                "rss" => {
                    let client = reqwest::Client::new();
                    let adapter = RssFeedAdapter::new(client);
                    let input = ServiceInput {
                        url: url.to_string(),
                        params: json!({}),
                    };
                    match adapter.execute(input).await {
                        Ok(out) => {
                            outputs.insert(
                                node_name.clone(),
                                json!({ "data": out.data, "metadata": out.metadata }),
                            );
                        }
                        Err(e) => {
                            errors.insert(node_name.clone(), e.to_string());
                        }
                    }
                }
                other => {
                    warn!(
                        kind = other,
                        node = node_name,
                        "skipping unsupported service kind in pipeline_run"
                    );
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
}

impl Default for McpGraphServer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a [`toml::Value`] to a [`serde_json::Value`] for adapter params.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
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
fn build_rest_params_from_node(node: &crate::application::pipeline_parser::NodeDecl) -> Value {
    let mut params = json!({});

    if let Some(method) = node.params.get("method").and_then(|v| v.as_str()) {
        params["method"] = json!(method);
    }
    if let Some(auth) = node.params.get("auth") {
        params["auth"] = toml_to_json(auth);
    }
    if let Some(headers) = node.params.get("headers") {
        params["headers"] = toml_to_json(headers);
    }
    if let Some(query) = node.params.get("query") {
        params["query"] = toml_to_json(query);
    }
    if let Some(body) = node.params.get("body") {
        params["body"] = toml_to_json(body);
    }
    if let Some(pagination) = node.params.get("pagination") {
        params["pagination"] = toml_to_json(pagination);
    }
    if let Some(dp) = node.params.get("data_path").and_then(|v| v.as_str()) {
        params["response"] = json!({ "data_path": dp });
    }

    params
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
        let server = McpGraphServer::new();
        let id = json!(1);
        let resp = server.handle_initialize(&id);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn tools_list_contains_all_tools() {
        let server = McpGraphServer::new();
        let id = json!(1);
        let resp = server.handle_tools_list(&id);
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
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
        let server = McpGraphServer::new();
        let id = json!(1);
        let args = json!({ "toml": "this is not valid toml [[[[" });
        let resp = server.tool_pipeline_validate(&id, &args);
        assert!(
            resp["error"].is_object()
                || resp["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("false")
        );
    }

    #[test]
    fn pipeline_validate_accepts_valid_pipeline() {
        let server = McpGraphServer::new();
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
        let resp = server.tool_pipeline_validate(&id, &args);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["valid"], true);
        assert_eq!(parsed["node_count"], 2);
    }

    #[test]
    fn pipeline_validate_missing_toml_returns_error() {
        // Test is sync — just check param validation path
        let server = McpGraphServer::new();
        let id = json!(1);
        let args = json!({});
        // We can't await in a non-async test context without a runtime,
        // so just verify the tool_pipeline_validate path for missing param.
        let resp = server.tool_pipeline_validate(&id, &args);
        assert!(resp["error"].is_object());
    }
}
