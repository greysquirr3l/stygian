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

fn parse_tools_call_text(resp: &Value) -> Result<Value, Box<dyn std::error::Error>> {
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
