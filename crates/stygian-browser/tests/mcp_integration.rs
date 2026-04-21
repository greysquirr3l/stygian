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

#[tokio::test]
#[ignore = "requires Chrome and external network"]
async fn mcp_session_save_restore_and_humanize_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = BrowserPool::new(test_config()).await?;
    let server = McpBrowserServer::new(pool);

    let acquire_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "browser_acquire",
                "arguments": {"target_profile": "reddit"}
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
            "id": 12,
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
    let _ = parse_tools_call_text(&navigate_resp)?;

    let save_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "tools/call",
            "params": {
                "name": "browser_session_save",
                "arguments": {
                    "session_id": session_id,
                    "ttl_secs": 3600
                }
            }
        }))
        .await;
    let save_payload = parse_tools_call_text(&save_resp)?;
    assert_eq!(
        save_payload.get("ttl_secs").and_then(Value::as_u64),
        Some(3600),
        "saved snapshot should persist requested ttl"
    );

    let restore_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 14,
            "method": "tools/call",
            "params": {
                "name": "browser_session_restore",
                "arguments": {
                    "session_id": session_id,
                    "use_saved": true,
                    "navigate_to_origin": false
                }
            }
        }))
        .await;
    let restore_payload = parse_tools_call_text(&restore_resp)?;
    assert_eq!(
        restore_payload.get("source").and_then(Value::as_str),
        Some("saved"),
        "restore should use in-memory saved snapshot by default"
    );

    let humanize_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 15,
            "method": "tools/call",
            "params": {
                "name": "browser_humanize",
                "arguments": {
                    "session_id": session_id,
                    "level": "none"
                }
            }
        }))
        .await;
    let humanize_payload = parse_tools_call_text(&humanize_resp)?;
    assert_eq!(
        humanize_payload.get("applied").and_then(Value::as_bool),
        Some(true),
        "humanize should report applied=true"
    );

    let attach_contract_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 16,
            "method": "tools/call",
            "params": {
                "name": "browser_attach",
                "arguments": {
                    "mode": "cdp_ws",
                    "endpoint": "ws://127.0.0.1:9222/devtools/browser/mock"
                }
            }
        }))
        .await;
    let attach_contract_payload = parse_tools_call_text(&attach_contract_resp)?;
    assert_eq!(
        attach_contract_payload
            .get("supported")
            .and_then(Value::as_bool),
        Some(false),
        "attach contract tool should clearly report unsupported until backend is implemented"
    );

    let auth_capture_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 17,
            "method": "tools/call",
            "params": {
                "name": "browser_auth_session",
                "arguments": {
                    "session_id": session_id,
                    "mode": "capture",
                    "ttl_secs": 1800,
                    "interaction_level": "none"
                }
            }
        }))
        .await;
    let auth_capture_payload = parse_tools_call_text(&auth_capture_resp)?;
    assert_eq!(
        auth_capture_payload.get("mode").and_then(Value::as_str),
        Some("capture"),
        "auth session wrapper should report capture mode"
    );

    let auth_resume_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 18,
            "method": "tools/call",
            "params": {
                "name": "browser_auth_session",
                "arguments": {
                    "session_id": session_id,
                    "mode": "resume",
                    "navigate_to_origin": false,
                    "interaction_level": "none"
                }
            }
        }))
        .await;
    let auth_resume_payload = parse_tools_call_text(&auth_resume_resp)?;
    assert_eq!(
        auth_resume_payload.get("mode").and_then(Value::as_str),
        Some("resume"),
        "auth session wrapper should report resume mode"
    );

    let release_resp = server
        .dispatch(&json!({
            "jsonrpc": "2.0",
            "id": 19,
            "method": "tools/call",
            "params": {
                "name": "browser_release",
                "arguments": {"session_id": session_id}
            }
        }))
        .await;
    let release_payload = parse_tools_call_text(&release_resp)?;
    assert_eq!(
        release_payload.get("released").and_then(Value::as_bool),
        Some(true),
        "browser_release should return released=true"
    );

    Ok(())
}
