#![cfg(feature = "mcp")]

//! MCP integration tests for stygian-browser.
//!
//! These tests exercise the JSON-RPC tool surface directly via
//! `McpBrowserServer::dispatch`.
//!
//! Requirements:
//! - `mcp` feature enabled
//! - real Chrome/Chromium binary available
//! - external network access
//!
//! Run:
//! ```sh
//! cargo test -p stygian-browser --features mcp --test mcp_integration -- --ignored --test-threads=1
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use stygian_browser::BrowserConfig;
use stygian_browser::BrowserPool;
use stygian_browser::config::PoolConfig;
use stygian_browser::mcp::McpBrowserServer;

type DynError = Box<dyn std::error::Error>;

fn unique_user_data_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("stygian-mcp-itest-{pid}-{n}"))
}

fn test_config() -> BrowserConfig {
    let mut cfg = BrowserConfig::builder()
        .headless(true)
        .pool(PoolConfig {
            min_size: 0,
            max_size: 1,
            acquire_timeout: Duration::from_secs(10),
            ..PoolConfig::default()
        })
        .build();

    cfg.launch_timeout = Duration::from_secs(30);
    cfg.cdp_timeout = Duration::from_secs(15);
    cfg.user_data_dir = Some(unique_user_data_dir());

    if let Ok(p) = std::env::var("STYGIAN_CHROME_PATH") {
        cfg.chrome_path = Some(PathBuf::from(p));
    }

    cfg
}

fn parse_tools_call_text(resp: &Value) -> Result<Value, DynError> {
    let is_error = resp
        .get("result")
        .and_then(|r| r.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    if is_error {
        return Err(
            std::io::Error::other(format!("MCP tool returned isError=true: {resp}")).into(),
        );
    }

    let text = resp
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing tools/call text payload: {resp}")))?;

    Ok(serde_json::from_str(text)?)
}

async fn call_tool(
    server: &McpBrowserServer,
    id: u64,
    name: &str,
    arguments: Value,
) -> Result<Value, DynError> {
    let resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        }))
        .await;
    parse_tools_call_text(&resp)
}

fn session_id_from_payload(payload: &Value) -> Result<String, DynError> {
    payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| std::io::Error::other("browser_acquire returned no session_id").into())
}

async fn acquire_session(
    server: &McpBrowserServer,
    id: u64,
    target_profile: &str,
) -> Result<String, DynError> {
    let payload = call_tool(
        server,
        id,
        "browser_acquire",
        json!({ "target_profile": target_profile }),
    )
    .await?;
    session_id_from_payload(&payload)
}

async fn release_session(
    server: &McpBrowserServer,
    id: u64,
    session_id: &str,
) -> Result<Value, DynError> {
    call_tool(
        server,
        id,
        "browser_release",
        json!({ "session_id": session_id }),
    )
    .await
}

async fn assert_session_save_ttl(
    server: &McpBrowserServer,
    id: u64,
    session_id: &str,
    ttl_secs: u64,
) -> Result<(), DynError> {
    let save_payload = call_tool(
        server,
        id,
        "browser_session_save",
        json!({
            "session_id": session_id,
            "ttl_secs": ttl_secs
        }),
    )
    .await?;
    assert_eq!(
        save_payload.get("ttl_secs").and_then(Value::as_u64),
        Some(ttl_secs),
        "saved snapshot should persist requested ttl"
    );
    Ok(())
}

async fn assert_session_restore_saved(
    server: &McpBrowserServer,
    id: u64,
    session_id: &str,
) -> Result<(), DynError> {
    let restore_payload = call_tool(
        server,
        id,
        "browser_session_restore",
        json!({
            "session_id": session_id,
            "use_saved": true,
            "navigate_to_origin": false
        }),
    )
    .await?;
    assert_eq!(
        restore_payload.get("source").and_then(Value::as_str),
        Some("saved"),
        "restore should use in-memory saved snapshot by default"
    );
    Ok(())
}

async fn assert_humanize_applied(
    server: &McpBrowserServer,
    id: u64,
    session_id: &str,
) -> Result<(), DynError> {
    let humanize_payload = call_tool(
        server,
        id,
        "browser_humanize",
        json!({
            "session_id": session_id,
            "level": "none"
        }),
    )
    .await?;
    assert_eq!(
        humanize_payload.get("applied").and_then(Value::as_bool),
        Some(true),
        "humanize should report applied=true"
    );
    Ok(())
}

async fn assert_auth_session_mode(
    server: &McpBrowserServer,
    id: u64,
    session_id: &str,
    mode: &str,
    ttl_secs: Option<u64>,
) -> Result<(), DynError> {
    let mut args = json!({
        "session_id": session_id,
        "mode": mode,
        "navigate_to_origin": false,
        "interaction_level": "none"
    });
    if mode == "capture"
        && let Some(ttl) = ttl_secs
        && let Some(obj) = args.as_object_mut()
    {
        obj.insert("ttl_secs".to_string(), Value::from(ttl));
    }

    let payload = call_tool(server, id, "browser_auth_session", args).await?;
    assert_eq!(
        payload.get("mode").and_then(Value::as_str),
        Some(mode),
        "auth session wrapper should report {mode} mode"
    );
    Ok(())
}

#[cfg(feature = "mcp-attach")]
async fn assert_attach_contract(server: &McpBrowserServer, id: u64) -> Result<(), DynError> {
    let attach_contract_payload = call_tool(
        server,
        id,
        "browser_attach",
        json!({
            "mode": "extension_bridge",
            "profile_hint": "reddit-main"
        }),
    )
    .await?;
    assert_eq!(
        attach_contract_payload
            .get("supported")
            .and_then(Value::as_bool),
        Some(false),
        "extension bridge mode should clearly report unsupported until backend is implemented"
    );
    Ok(())
}

#[cfg(feature = "mcp-attach")]
async fn attach_cdp_ws_session(
    server: &McpBrowserServer,
    id: u64,
    endpoint: &str,
) -> Result<String, DynError> {
    let payload = call_tool(
        server,
        id,
        "browser_attach",
        json!({
            "mode": "cdp_ws",
            "endpoint": endpoint,
            "target_profile": "default"
        }),
    )
    .await?;

    assert_eq!(
        payload.get("supported").and_then(Value::as_bool),
        Some(true),
        "cdp_ws attach should return supported=true when connect succeeds"
    );

    session_id_from_payload(&payload)
}

#[tokio::test]
#[ignore = "requires Chrome and external network"]
async fn mcp_acquire_navigate_release_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let pool = BrowserPool::new(test_config()).await?;
    let server = McpBrowserServer::new(pool);

    let acquire_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "browser_acquire",
                "arguments": {
                    "stealth_level": "advanced",
                    "target_profile": "default"
                }
            }
        }))
        .await;
    let acquire_payload = parse_tools_call_text(&acquire_resp)?;
    let session_id = acquire_payload
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("browser_acquire returned no session_id"))?
        .to_string();

    let navigate_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "browser_navigate",
                "arguments": {
                    "session_id": session_id,
                    "url": "https://example.com",
                    "timeout_secs": 30
                }
            }
        }))
        .await;
    let navigate_payload = parse_tools_call_text(&navigate_resp)?;

    let title = navigate_payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        title.contains("example"),
        "expected navigation title to contain 'example', got payload: {navigate_payload}"
    );

    let release_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "browser_release",
                "arguments": {
                    "session_id": session_id
                }
            }
        }))
        .await;
    let release_payload = parse_tools_call_text(&release_resp)?;

    assert_eq!(
        release_payload.get("released").and_then(Value::as_bool),
        Some(true),
        "browser_release should return released=true; got payload: {release_payload}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires Chrome and external network"]
async fn mcp_session_save_restore_and_humanize_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = BrowserPool::new(test_config()).await?;
    let server = McpBrowserServer::new(pool);

    let session_id = acquire_session(&server, 11, "reddit").await?;

    let _ = call_tool(
        &server,
        12,
        "browser_navigate",
        json!({
            "session_id": session_id,
            "url": "https://example.com",
            "timeout_secs": 30
        }),
    )
    .await?;

    assert_session_save_ttl(&server, 13, &session_id, 3600).await?;
    assert_session_restore_saved(&server, 14, &session_id).await?;
    assert_humanize_applied(&server, 15, &session_id).await?;

    #[cfg(feature = "mcp-attach")]
    {
        assert_attach_contract(&server, 16).await?;
    }

    assert_auth_session_mode(&server, 17, &session_id, "capture", Some(1800)).await?;
    assert_auth_session_mode(&server, 18, &session_id, "resume", None).await?;

    let release_payload = release_session(&server, 19, &session_id).await?;
    assert_eq!(
        release_payload.get("released").and_then(Value::as_bool),
        Some(true),
        "browser_release should return released=true"
    );

    Ok(())
}

#[cfg(feature = "mcp-attach")]
#[tokio::test]
#[ignore = "requires STYGIAN_ATTACH_WS_ENDPOINT and reachable DevTools websocket"]
async fn mcp_attach_cdp_ws_navigate_release_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = match std::env::var("STYGIAN_ATTACH_WS_ENDPOINT") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!(
                "Skipping mcp_attach_cdp_ws_navigate_release_round_trip: STYGIAN_ATTACH_WS_ENDPOINT is not set"
            );
            return Ok(());
        }
    };

    let pool = BrowserPool::new(test_config()).await?;
    let server = McpBrowserServer::new(pool);

    let session_id = attach_cdp_ws_session(&server, 31, &endpoint).await?;

    let navigate_payload = call_tool(
        &server,
        32,
        "browser_navigate",
        json!({
            "session_id": session_id,
            "url": "https://example.com",
            "timeout_secs": 30
        }),
    )
    .await?;
    let title = navigate_payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        title.contains("example"),
        "expected attached session navigation title to contain 'example', got payload: {navigate_payload}"
    );

    let release_payload = release_session(&server, 33, &session_id).await?;
    assert_eq!(
        release_payload.get("released").and_then(Value::as_bool),
        Some(true),
        "browser_release should return released=true for attached session"
    );

    Ok(())
}
