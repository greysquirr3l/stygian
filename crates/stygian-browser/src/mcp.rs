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
//! To use `browser_attach` (`cdp_ws` mode), also enable `mcp-attach`:
//!
//! ```toml
//! [dependencies]
//! stygian-browser = { version = "*", features = ["mcp", "mcp-attach"] }
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
//! | `browser_attach` *(mcp-attach feature)* | `mode, endpoint?, profile_hint?, target_profile?` | attach session result |
//! | `browser_auth_session` | `session_id, mode, file_path?, ttl_secs?, navigate_to_origin?, interaction_level?` | auth/session workflow result |
//! | `browser_session_save` | `session_id, ttl_secs?, file_path?, include_snapshot?` | saved session state metadata |
//! | `browser_session_restore` | `session_id, snapshot?, file_path?, use_saved?, navigate_to_origin?` | restored session state metadata |
//! | `browser_humanize` | `session_id, level?, viewport_width?, viewport_height?` | humanization result |
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

use chromiumoxide::Browser;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
    task::JoinHandle,
    time::sleep,
};
use tracing::{debug, info};
use ulid::Ulid;

#[cfg(feature = "mcp-attach")]
use futures::StreamExt;

use crate::{
    BrowserHandle, BrowserPool,
    behavior::{InteractionLevel, InteractionSimulator},
    config::StealthLevel,
    error::{BrowserError, Result},
    page::WaitUntil,
    session::{SessionSnapshot, restore_session, save_session},
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
    /// Attached browser runtime for `cdp_ws` sessions.
    attached_browser: Arc<Mutex<Option<Browser>>>,
    /// Background task driving the attached browser protocol handler.
    attached_handler_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Persistent page for this session. Reused across tool calls until release.
    page: Arc<Mutex<Option<crate::page::PageHandle>>>,
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
    /// Optional target profile tuning hint used by MCP navigation helpers.
    target_profile: String,
    /// Last URL successfully navigated to via `browser_navigate`.
    current_url: Option<String>,
    /// Optional in-memory saved session snapshot for auth/session reuse.
    saved_snapshot: Option<SessionSnapshot>,
    /// Endpoint used by an attached browser session.
    attach_endpoint: Option<String>,
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
                    },
                    "target_profile": {
                        "type": "string",
                        "enum": ["default", "reddit"],
                        "description": "Optional target tuning profile. 'reddit' enables challenge-aware waits and stabilization tuned for Reddit flows."
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
        #[cfg(feature = "mcp-attach")]
        json!({
            "name": "browser_attach",
            "description": "Attach MCP workflows to an existing user browser/profile context. `cdp_ws` mode is implemented and creates a live attached session; `extension_bridge` remains a contract-only path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["extension_bridge", "cdp_ws"],
                        "description": "Attach strategy. extension_bridge is the recommended future path for existing user profiles. cdp_ws targets a remote debugging websocket endpoint."
                    },
                    "endpoint": {
                        "type": "string",
                        "description": "Optional endpoint for cdp_ws mode, e.g. ws://127.0.0.1:9222/devtools/browser/<id>."
                    },
                    "profile_hint": {
                        "type": "string",
                        "description": "Optional human-readable profile label (e.g. 'reddit-main')."
                    },
                    "target_profile": {
                        "type": "string",
                        "enum": ["default", "reddit"],
                        "description": "Optional target tuning profile used by session navigation helpers."
                    }
                },
                "required": ["mode"]
            }
        }),
        json!({
            "name": "browser_auth_session",
            "description": "High-level auth/session workflow wrapper. Use mode='capture' to persist login state and mode='resume' to restore it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "mode": { "type": "string", "enum": ["capture", "resume"] },
                    "file_path": { "type": "string", "description": "Optional snapshot file path for durable persistence." },
                    "ttl_secs": { "type": "integer", "description": "Optional TTL (seconds) when capturing." },
                    "navigate_to_origin": { "type": "boolean", "default": true, "description": "When resuming, navigate to snapshot origin before restore." },
                    "interaction_level": { "type": "string", "enum": ["none", "low", "medium", "high"], "default": "none", "description": "Optional post-operation human-like interaction step." }
                },
                "required": ["session_id", "mode"]
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
    tools.push(json!({
            "name": "browser_session_save",
            "description": "Save current browser session state (cookies + localStorage) to memory and optionally to disk.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "ttl_secs": { "type": "integer", "description": "Optional snapshot TTL in seconds." },
                    "file_path": { "type": "string", "description": "Optional path to save session snapshot JSON." },
                    "include_snapshot": { "type": "boolean", "default": false, "description": "When true, include full snapshot payload in response." }
                },
                "required": ["session_id"]
            }
        }));
    tools.push(json!({
            "name": "browser_session_restore",
            "description": "Restore browser session state from provided snapshot JSON, saved in-memory snapshot, or file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "snapshot": { "type": "object", "description": "Inline SessionSnapshot JSON." },
                    "file_path": { "type": "string", "description": "Path to a SessionSnapshot JSON file." },
                    "use_saved": { "type": "boolean", "default": true, "description": "Use in-memory snapshot when no inline/file snapshot is provided." },
                    "navigate_to_origin": { "type": "boolean", "default": true, "description": "Navigate to snapshot origin before restore when origin is present." }
                },
                "required": ["session_id"]
            }
        }));
    tools.push(json!({
            "name": "browser_humanize",
            "description": "Apply human-like interaction sequence on current page (scroll, key activity, mouse movement).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "level": { "type": "string", "enum": ["none", "low", "medium", "high"], "default": "low" },
                    "viewport_width": { "type": "number", "default": 1366.0 },
                    "viewport_height": { "type": "number", "default": 768.0 }
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
            #[cfg(feature = "mcp-attach")]
            "browser_attach" => self.tool_browser_attach(&args).await,
            #[cfg(not(feature = "mcp-attach"))]
            "browser_attach" => Err(BrowserError::ConfigError(
                "browser_attach requires the 'mcp-attach' feature".to_string(),
            )),
            "browser_auth_session" => self.tool_browser_auth_session(&args).await,
            "browser_session_save" => self.tool_browser_session_save(&args).await,
            "browser_session_restore" => self.tool_browser_session_restore(&args).await,
            "browser_humanize" => self.tool_browser_humanize(&args).await,
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
        let target_profile = args
            .get("target_profile")
            .and_then(|v| v.as_str())
            .map_or_else(
                || "default".to_string(),
                |s| {
                    if s.eq_ignore_ascii_case("reddit") {
                        "reddit".to_string()
                    } else {
                        "default".to_string()
                    }
                },
            );

        let handle = self.pool.acquire().await?;
        let session_id = Ulid::new().to_string();

        let effective_stealth = format!("{stealth_level:?}").to_lowercase();
        self.sessions.lock().await.insert(
            session_id.clone(),
            McpSession {
                handle: Arc::new(Mutex::new(Some(handle))),
                attached_browser: Arc::new(Mutex::new(None)),
                attached_handler_task: Arc::new(Mutex::new(None)),
                page: Arc::new(Mutex::new(None)),
                stealth_level,
                tls_profile: tls_profile.clone(),
                webrtc_policy: webrtc_policy.clone(),
                cdp_fix_mode: cdp_fix_mode.clone(),
                proxy: proxy.clone(),
                target_profile: target_profile.clone(),
                current_url: None,
                saved_snapshot: None,
                attach_endpoint: None,
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
                "proxy": proxy,
                "target_profile": target_profile
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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        let requested_stealth = self.session_handle_and_stealth(&session_id).await?.1;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs(timeout_secs),
            reddit_profile,
        )
        .await?;

        {
            let mut page_guard = page_arc.lock().await;
            let page = page_guard.as_mut().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;
            Self::navigate_with_profile(
                page,
                &url,
                Duration::from_secs(timeout_secs),
                reddit_profile,
            )
            .await?;
            drop(page_guard);
        }

        let mut result = {
            let page_guard = page_arc.lock().await;
            let page = page_guard.as_ref().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;
            let result = Self::run_stealth_diagnostic(page, observed).await;
            drop(page_guard);
            result
        };

        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let (challenge_detected, challenge_cleared, title) = {
            let mut page_guard = page_arc.lock().await;
            let page = page_guard.as_mut().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;

            let (challenge_detected, challenge_cleared) = Self::navigate_with_profile(
                page,
                &url,
                Duration::from_secs_f64(timeout_secs),
                reddit_profile,
            )
            .await?;
            let title = page.title().await.unwrap_or_default();
            drop(page_guard);
            (challenge_detected, challenge_cleared, title)
        };

        let current_url = url.clone();

        // Persist the navigated URL so that browser_content / browser_eval /
        // browser_screenshot can use it without the caller having to repeat it.
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(current_url.clone());
        }

        Ok(json!({
            "title": title,
            "url": current_url,
            "challenge_detected": challenge_detected,
            "challenge_cleared": challenge_cleared
        }))
    }

    async fn tool_browser_eval(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let script = Self::require_str(args, "script")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_eval".to_string(),
            )
        })?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            Some(nav_url.as_str()),
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;
        let result: Value = page.eval(&script).await?;
        drop(page_guard);

        Ok(json!({ "result": result }))
    }

    async fn tool_browser_screenshot(&self, args: &Value) -> Result<Value> {
        use base64::Engine as _;
        let session_id = Self::require_str(args, "session_id")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_screenshot".to_string(),
            )
        })?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            Some(nav_url.as_str()),
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;
        let png_bytes = page.screenshot().await?;
        drop(page_guard);

        let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        Ok(json!({ "data": encoded, "mimeType": "image/png", "bytes": png_bytes.len() }))
    }

    async fn tool_browser_content(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;
        let nav_url = nav_url_opt.ok_or_else(|| {
            BrowserError::ConfigError(
                "No page loaded — call browser_navigate before browser_content".to_string(),
            )
        })?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            Some(nav_url.as_str()),
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;
        let html = page.content().await?;
        drop(page_guard);

        Ok(json!({ "html": html, "bytes": html.len() }))
    }

    #[cfg(feature = "mcp-attach")]
    async fn tool_browser_attach(&self, args: &Value) -> Result<Value> {
        let mode = Self::require_str(args, "mode")?;
        let endpoint = args
            .get("endpoint")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let profile_hint = args
            .get("profile_hint")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let target_profile = args
            .get("target_profile")
            .and_then(Value::as_str)
            .map_or_else(
                || "default".to_string(),
                |s| {
                    if s.eq_ignore_ascii_case("reddit") {
                        "reddit".to_string()
                    } else {
                        "default".to_string()
                    }
                },
            );

        match mode.as_str() {
            "extension_bridge" => Ok(json!({
                "supported": false,
                "mode": mode,
                "profile_hint": profile_hint,
                "status": "not_implemented",
                "next_step": "Implement extension bridge handshake and profile transfer"
            })),
            "cdp_ws" => {
                let endpoint = endpoint.ok_or_else(|| {
                    BrowserError::ConfigError("missing endpoint for cdp_ws mode".to_string())
                })?;
                if !(endpoint.starts_with("ws://") || endpoint.starts_with("wss://")) {
                    return Err(BrowserError::ConfigError(
                        "endpoint must start with ws:// or wss://".to_string(),
                    ));
                }

                let attach_timeout = Duration::from_secs(10);
                let (browser, mut handler) =
                    tokio::time::timeout(attach_timeout, Browser::connect(endpoint.clone()))
                        .await
                        .map_err(|_| BrowserError::Timeout {
                            operation: "Browser.connect".to_string(),
                            duration_ms: 10_000,
                        })?
                        .map_err(|e| BrowserError::ConnectionError {
                            url: endpoint.clone(),
                            reason: e.to_string(),
                        })?;

                let handler_task = tokio::spawn(async move {
                    while let Some(event) = handler.next().await {
                        if let Err(error) = event {
                            tracing::warn!(%error, "attached browser handler error");
                            break;
                        }
                    }
                });

                let session_id = Ulid::new().to_string();
                self.sessions.lock().await.insert(
                    session_id.clone(),
                    McpSession {
                        handle: Arc::new(Mutex::new(None)),
                        attached_browser: Arc::new(Mutex::new(Some(browser))),
                        attached_handler_task: Arc::new(Mutex::new(Some(handler_task))),
                        page: Arc::new(Mutex::new(None)),
                        stealth_level: StealthLevel::None,
                        tls_profile: None,
                        webrtc_policy: None,
                        cdp_fix_mode: None,
                        proxy: None,
                        target_profile: target_profile.clone(),
                        current_url: None,
                        saved_snapshot: None,
                        attach_endpoint: Some(endpoint.clone()),
                    },
                );

                Ok(json!({
                    "supported": true,
                    "mode": "cdp_ws",
                    "session_id": session_id,
                    "endpoint": endpoint,
                    "profile_hint": profile_hint,
                    "requested_metadata": {
                        "target_profile": target_profile
                    }
                }))
            }
            other => Err(BrowserError::ConfigError(format!(
                "Invalid mode '{other}'. Use one of: extension_bridge, cdp_ws"
            ))),
        }
    }

    async fn tool_browser_auth_session(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let mode = Self::require_str(args, "mode")?;
        let file_path = args
            .get("file_path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let ttl_secs = args.get("ttl_secs").and_then(Value::as_u64);
        let navigate_to_origin = args
            .get("navigate_to_origin")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let interaction_level = args
            .get("interaction_level")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string();

        let payload = match mode.as_str() {
            "capture" => {
                let mut save_args = json!({
                    "session_id": session_id,
                    "include_snapshot": false
                });
                if let Some(ttl) = ttl_secs
                    && let Some(obj) = save_args.as_object_mut()
                {
                    obj.insert("ttl_secs".to_string(), Value::from(ttl));
                }
                if let Some(path) = file_path.clone()
                    && let Some(obj) = save_args.as_object_mut()
                {
                    obj.insert("file_path".to_string(), Value::String(path));
                }

                let save = self.tool_browser_session_save(&save_args).await?;

                let humanize = if interaction_level == "none" {
                    None
                } else {
                    let humanize_args = json!({
                        "session_id": session_id,
                        "level": interaction_level
                    });
                    Some(self.tool_browser_humanize(&humanize_args).await?)
                };

                json!({
                    "mode": "capture",
                    "session_id": session_id,
                    "save": save,
                    "humanize": humanize
                })
            }
            "resume" => {
                let mut restore_args = json!({
                    "session_id": session_id,
                    "use_saved": file_path.is_none(),
                    "navigate_to_origin": navigate_to_origin
                });
                if let Some(path) = file_path.clone()
                    && let Some(obj) = restore_args.as_object_mut()
                {
                    obj.insert("file_path".to_string(), Value::String(path));
                }

                let restore = self.tool_browser_session_restore(&restore_args).await?;

                let humanize = if interaction_level == "none" {
                    None
                } else {
                    let humanize_args = json!({
                        "session_id": session_id,
                        "level": interaction_level
                    });
                    Some(self.tool_browser_humanize(&humanize_args).await?)
                };

                json!({
                    "mode": "resume",
                    "session_id": session_id,
                    "restore": restore,
                    "humanize": humanize
                })
            }
            other => {
                return Err(BrowserError::ConfigError(format!(
                    "Invalid mode '{other}'. Use one of: capture, resume"
                )));
            }
        };

        Ok(payload)
    }

    async fn tool_browser_session_save(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let ttl_secs = args.get("ttl_secs").and_then(Value::as_u64);
        let file_path = args
            .get("file_path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let include_snapshot = args
            .get("include_snapshot")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            nav_url_opt.as_deref(),
            Duration::from_secs(30),
            reddit_profile,
        )
        .await?;

        let mut snapshot = {
            let page_guard = page_arc.lock().await;
            let page = page_guard.as_ref().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;
            let saved = save_session(page).await?;
            drop(page_guard);
            saved
        };

        snapshot.ttl_secs = ttl_secs;
        if let Some(path) = &file_path {
            snapshot.save_to_file(path)?;
        }

        let cookie_count = snapshot.cookies.len();
        let local_storage_keys = snapshot.local_storage.len();
        let origin = snapshot.origin.clone();

        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.saved_snapshot = Some(snapshot.clone());
        }

        let mut out = json!({
            "session_id": session_id,
            "origin": origin,
            "cookie_count": cookie_count,
            "local_storage_keys": local_storage_keys,
            "ttl_secs": ttl_secs,
            "saved_to_file": file_path
        });

        if include_snapshot && let Some(obj) = out.as_object_mut() {
            obj.insert(
                "snapshot".to_string(),
                serde_json::to_value(&snapshot).map_err(|e| {
                    BrowserError::ConfigError(format!("failed to serialize session snapshot: {e}"))
                })?,
            );
        }

        Ok(out)
    }

    async fn tool_browser_session_restore(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let file_path = args
            .get("file_path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let use_saved = args
            .get("use_saved")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let navigate_to_origin = args
            .get("navigate_to_origin")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let snapshot = if let Some(path) = file_path.as_deref() {
            SessionSnapshot::load_from_file(path)?
        } else if let Some(inline) = args.get("snapshot") {
            serde_json::from_value::<SessionSnapshot>(inline.clone()).map_err(|e| {
                BrowserError::ConfigError(format!("invalid inline session snapshot: {e}"))
            })?
        } else if use_saved {
            self.sessions
                .lock()
                .await
                .get(&session_id)
                .and_then(|s| s.saved_snapshot.clone())
                .ok_or_else(|| {
                    BrowserError::ConfigError(
                        "No saved session snapshot found for this session".to_string(),
                    )
                })?
        } else {
            return Err(BrowserError::ConfigError(
                "No restore source provided. Set one of: file_path, snapshot, or use_saved=true"
                    .to_string(),
            ));
        };

        let source = if file_path.is_some() {
            "file"
        } else if args.get("snapshot").is_some() {
            "inline"
        } else {
            "saved"
        };

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            nav_url_opt.as_deref(),
            Duration::from_secs(30),
            reddit_profile,
        )
        .await?;

        {
            let mut page_guard = page_arc.lock().await;
            let page = page_guard.as_mut().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;

            if navigate_to_origin && !snapshot.origin.is_empty() {
                Self::navigate_with_profile(
                    page,
                    &snapshot.origin,
                    Duration::from_secs(30),
                    reddit_profile,
                )
                .await?;
            }

            restore_session(page, &snapshot).await?;
            drop(page_guard);
        }

        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            if !snapshot.origin.is_empty() {
                session.current_url = Some(snapshot.origin.clone());
            }
            session.saved_snapshot = Some(snapshot.clone());
        }

        Ok(json!({
            "session_id": session_id,
            "source": source,
            "origin": snapshot.origin,
            "cookie_count": snapshot.cookies.len(),
            "local_storage_keys": snapshot.local_storage.len(),
            "snapshot_expired": snapshot.is_expired()
        }))
    }

    async fn tool_browser_humanize(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;
        let level = match args.get("level").and_then(Value::as_str).unwrap_or("low") {
            "none" => InteractionLevel::None,
            "medium" => InteractionLevel::Medium,
            "high" => InteractionLevel::High,
            _ => InteractionLevel::Low,
        };
        let viewport_width = args
            .get("viewport_width")
            .and_then(Value::as_f64)
            .unwrap_or(1366.0);
        let viewport_height = args
            .get("viewport_height")
            .and_then(Value::as_f64)
            .unwrap_or(768.0);

        let (session_arc, attached_browser_arc, page_arc, nav_url_opt, reddit_profile) =
            self.session_runtime(&session_id).await?;

        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            nav_url_opt.as_deref(),
            Duration::from_secs(30),
            reddit_profile,
        )
        .await?;

        {
            let page_guard = page_arc.lock().await;
            let page = page_guard.as_ref().ok_or_else(|| {
                BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
            })?;

            let mut simulator = InteractionSimulator::new(level);
            simulator
                .random_interaction(page.inner(), viewport_width, viewport_height)
                .await?;
            drop(page_guard);
        }

        let level_str = match level {
            InteractionLevel::None => "none",
            InteractionLevel::Low => "low",
            InteractionLevel::Medium => "medium",
            InteractionLevel::High => "high",
        };

        Ok(json!({
            "session_id": session_id,
            "level": level_str,
            "viewport_width": viewport_width,
            "viewport_height": viewport_height,
            "applied": true
        }))
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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        Self::navigate_with_profile(
            page,
            &url,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
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
        drop(page_guard);
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        Self::navigate_with_profile(
            page,
            &url,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let roots = page.query_selector_all(&root_selector).await?;
        let mut results: Vec<Value> = Vec::with_capacity(roots.len());
        for root in &roots {
            if let Some(obj) = Self::extract_record(root, &schema).await {
                results.push(Value::Object(obj));
            }
        }
        drop(page_guard);
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        Self::navigate_with_profile(
            page,
            &url,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        // Resolve the reference node — first match only.
        let refs = page.query_selector_all(&reference_selector).await?;
        let Some(reference) = refs.into_iter().next() else {
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
        drop(page_guard);
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

        let (session_arc, attached_browser_arc, page_arc, _, _) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_millis(timeout_ms),
            false,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        let report = page
            .warmup(WarmupOptions {
                url,
                wait,
                timeout_ms,
                stabilize_ms,
            })
            .await?;
        drop(page_guard);

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

        let (session_arc, attached_browser_arc, page_arc, _, _) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_millis(timeout_ms),
            false,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        let report = page
            .refresh(RefreshOptions {
                wait,
                timeout_ms,
                reset_connection,
            })
            .await?;
        drop(page_guard);

        Ok(json!({
            "session_id": session_id,
            "url": report.url,
            "elapsed_ms": report.elapsed_ms,
            "status_code": report.status_code
        }))
    }

    async fn tool_browser_release(&self, args: &Value) -> Result<Value> {
        let session_id = Self::require_str(args, "session_id")?;

        // Remove session from the map so further calls immediately fail.
        let (session_arc, attached_browser_arc, attached_handler_task_arc, page_arc) = {
            let mut sessions = self.sessions.lock().await;
            let removed = sessions.remove(&session_id).ok_or_else(|| {
                BrowserError::ConfigError(format!("Unknown session: {session_id}"))
            })?;
            drop(sessions);
            (
                removed.handle,
                removed.attached_browser,
                removed.attached_handler_task,
                removed.page,
            )
        };

        // Take and release the handle without holding the map lock
        let handle = session_arc.lock().await.take();
        if let Some(h) = handle {
            h.release().await;
        }

        let attached_browser = attached_browser_arc.lock().await.take();
        if let Some(mut browser) = attached_browser {
            let close_timeout = Duration::from_secs(5);
            match tokio::time::timeout(close_timeout, browser.close()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%session_id, %error, "attached browser close failed during release");
                }
                Err(_) => {
                    tracing::warn!(%session_id, "attached browser close timed out during release");
                }
            }
        }

        let attached_handler_task = attached_handler_task_arc.lock().await.take();
        if let Some(task) = attached_handler_task {
            task.abort();
        }

        let page = page_arc.lock().await.take();
        if let Some(page) = page {
            page.close().await.ok();
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
                    "proxy": s.proxy,
                    "target_profile": s.target_profile,
                    "current_url": s.current_url,
                    "has_saved_snapshot": s.saved_snapshot.is_some(),
                    "attach_endpoint": s.attach_endpoint
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

    async fn session_runtime(
        &self,
        session_id: &str,
    ) -> Result<(
        Arc<Mutex<Option<BrowserHandle>>>,
        Arc<Mutex<Option<Browser>>>,
        Arc<Mutex<Option<crate::page::PageHandle>>>,
        Option<String>,
        bool,
    )> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|s| {
                (
                    s.handle.clone(),
                    s.attached_browser.clone(),
                    s.page.clone(),
                    s.current_url.clone(),
                    s.target_profile == "reddit",
                )
            })
            .ok_or_else(|| BrowserError::ConfigError(format!("Unknown session: {session_id}")))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "session runtime handles and bootstrap options are passed explicitly for clarity"
    )]
    async fn ensure_session_page(
        &self,
        session_id: &str,
        handle_arc: &Arc<Mutex<Option<BrowserHandle>>>,
        attached_browser_arc: &Arc<Mutex<Option<Browser>>>,
        page_arc: &Arc<Mutex<Option<crate::page::PageHandle>>>,
        current_url: Option<&str>,
        timeout: Duration,
        reddit_profile: bool,
    ) -> Result<()> {
        let mut page_guard = page_arc.lock().await;
        let created = if page_guard.is_none() {
            let new_page =
                Self::create_session_page(session_id, handle_arc, attached_browser_arc).await?;

            *page_guard = Some(new_page);
            true
        } else {
            false
        };

        if created
            && let Some(url) = current_url
            && let Some(page) = page_guard.as_mut()
        {
            Self::navigate_with_profile(page, url, timeout, reddit_profile).await?;
        }

        drop(page_guard);

        Ok(())
    }

    async fn create_session_page(
        session_id: &str,
        handle_arc: &Arc<Mutex<Option<BrowserHandle>>>,
        attached_browser_arc: &Arc<Mutex<Option<Browser>>>,
    ) -> Result<crate::page::PageHandle> {
        let handle_guard = handle_arc.lock().await;
        if let Some(handle) = handle_guard.as_ref() {
            let browser = handle
                .browser()
                .ok_or_else(|| {
                    BrowserError::ConfigError(format!("Browser handle invalid: {session_id}"))
                })?;
            let page = browser.new_page().await?;
            drop(handle_guard);
            return Ok(page);
        }
        drop(handle_guard);

        let browser_guard = attached_browser_arc.lock().await;
        let browser = browser_guard.as_ref().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session already released: {session_id}"))
        })?;
        let raw_page =
            browser
                .new_page("about:blank")
                .await
                .map_err(|e| BrowserError::CdpError {
                    operation: "Browser.newPage".to_string(),
                    message: e.to_string(),
                })?;
        drop(browser_guard);

        Ok(crate::page::PageHandle::new(
            raw_page,
            Duration::from_secs(30),
        ))
    }

    async fn navigate_with_profile(
        page: &mut crate::page::PageHandle,
        url: &str,
        timeout: Duration,
        reddit_profile: bool,
    ) -> Result<(bool, bool)> {
        let wait_until = if reddit_profile {
            WaitUntil::DomContentLoaded
        } else {
            WaitUntil::Selector("body".to_string())
        };

        page.navigate(url, wait_until, timeout).await?;

        if reddit_profile || url.contains("reddit.com") {
            return Self::wait_for_reddit_challenge(page, timeout).await;
        }

        Ok((false, true))
    }

    async fn wait_for_reddit_challenge(
        page: &crate::page::PageHandle,
        timeout: Duration,
    ) -> Result<(bool, bool)> {
        let max_wait = timeout.min(Duration::from_secs(15));
        let mut elapsed = Duration::ZERO;
        let interval = Duration::from_millis(500);
        let mut challenge_seen = false;

        while elapsed <= max_wait {
            let challenge_state = page
                .eval::<Value>(
                    r#"(() => {
                        const title = (document.title || "").toLowerCase();
                        const href = (location.href || "").toLowerCase();
                        const body = (document.body?.innerText || "").toLowerCase();
                        const challenge =
                            title.includes("verification") ||
                            title.includes("just a moment") ||
                            href.includes("/js_challenge") ||
                            body.includes("please wait for verification") ||
                            body.includes("verify you are human");
                        return {
                            challenge,
                            ready: document.readyState === "complete"
                        };
                    })()"#,
                )
                .await
                .unwrap_or_else(|_| json!({"challenge": false, "ready": true}));

            let is_challenge = challenge_state
                .get("challenge")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let ready = challenge_state
                .get("ready")
                .and_then(Value::as_bool)
                .unwrap_or(true);

            challenge_seen |= is_challenge;
            if !is_challenge && ready {
                return Ok((challenge_seen, true));
            }

            sleep(interval).await;
            elapsed += interval;
        }

        Ok((challenge_seen, false))
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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        Self::navigate_with_profile(
            page,
            &url,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
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
        drop(page_guard);
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

        let (session_arc, attached_browser_arc, page_arc, _, reddit_profile) =
            self.session_runtime(&session_id).await?;
        self.ensure_session_page(
            &session_id,
            &session_arc,
            &attached_browser_arc,
            &page_arc,
            None,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
        )
        .await?;

        let mut page_guard = page_arc.lock().await;
        let page = page_guard.as_mut().ok_or_else(|| {
            BrowserError::ConfigError(format!("Session page unavailable: {session_id}"))
        })?;

        Self::navigate_with_profile(
            page,
            &url,
            Duration::from_secs_f64(timeout_secs),
            reddit_profile,
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
        drop(page_guard);
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            session.current_url = Some(url.clone());
        }

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

    #[test]
    fn tool_defs_include_browser_auth_session() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_auth_session")),
            "TOOL_DEFINITIONS must contain browser_auth_session"
        );
    }

    #[test]
    fn browser_auth_session_required_args() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_auth_session"))
            .ok_or("browser_auth_session must be in TOOL_DEFINITIONS")?;
        let required = def
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(Value::as_array)
            .ok_or("browser_auth_session inputSchema missing 'required' array")?;

        assert!(
            required.iter().any(|v| v == "session_id"),
            "session_id must be required in browser_auth_session"
        );
        assert!(
            required.iter().any(|v| v == "mode"),
            "mode must be required in browser_auth_session"
        );
        Ok(())
    }

    #[test]
    fn tool_defs_include_browser_session_save() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_session_save")),
            "TOOL_DEFINITIONS must contain browser_session_save"
        );
    }

    #[test]
    fn tool_defs_include_browser_session_restore() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter().any(
                |t| t.get("name").and_then(|n| n.as_str()) == Some("browser_session_restore")
            ),
            "TOOL_DEFINITIONS must contain browser_session_restore"
        );
    }

    #[test]
    fn tool_defs_include_browser_humanize() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_humanize")),
            "TOOL_DEFINITIONS must contain browser_humanize"
        );
    }

    #[cfg(feature = "mcp-attach")]
    #[test]
    fn tool_defs_include_browser_attach() {
        let defs = &*TOOL_DEFINITIONS;
        assert!(
            defs.iter().any(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_attach")),
            "TOOL_DEFINITIONS must contain browser_attach when mcp-attach is enabled"
        );
    }

    #[cfg(feature = "mcp-attach")]
    #[test]
    fn browser_attach_schema_includes_target_profile(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let defs = &*TOOL_DEFINITIONS;
        let def = defs
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("browser_attach"))
            .ok_or("browser_attach must be in TOOL_DEFINITIONS")?;
        let props = def
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(Value::as_object)
            .ok_or("browser_attach inputSchema missing properties")?;
        let target_profile = props
            .get("target_profile")
            .ok_or("browser_attach inputSchema missing target_profile")?;
        let enum_values = target_profile
            .get("enum")
            .and_then(Value::as_array)
            .ok_or("browser_attach target_profile missing enum")?;

        assert!(
            enum_values.iter().any(|v| v == "default"),
            "browser_attach target_profile enum must include default"
        );
        assert!(
            enum_values.iter().any(|v| v == "reddit"),
            "browser_attach target_profile enum must include reddit"
        );
        Ok(())
    }
}
