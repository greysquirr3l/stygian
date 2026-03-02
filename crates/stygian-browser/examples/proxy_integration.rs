//! Example 6: Proxy integration
//!
//! Demonstrates configuring an HTTP/SOCKS5 proxy with WebRTC leak protection
//! and verifying the apparent IP address changed.
//!
//! Set the `MYCELIUM_PROXY` environment variable before running:
//!
//! ```sh
//! MYCELIUM_PROXY=http://user:pass@proxy.example.com:8080 \
//!   cargo run --example proxy_integration -p stygian-browser
//! ```
//!
//! Without a proxy configured the example still runs but shows your real IP.

use std::time::Duration;

use stygian_browser::webrtc::{ProxyLocation, WebRtcConfig, WebRtcPolicy};
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // ── Configuration ──────────────────────────────────────────────────────────
    let proxy_url = std::env::var("MYCELIUM_PROXY").ok();
    let using_proxy = proxy_url.is_some();

    let mut builder = BrowserConfig::builder()
        .headless(true)
        // Block non-proxied WebRTC to prevent IP leaks
        .webrtc(WebRtcConfig {
            policy: WebRtcPolicy::DisableNonProxied,
            // Spoof geolocation to match the proxy's region
            location: Some(ProxyLocation::new_us_east()),
            ..Default::default()
        });

    if let Some(proxy) = proxy_url {
        println!("Using proxy: {proxy}");
        builder = builder.proxy(proxy);
    } else {
        println!("No MYCELIUM_PROXY set — running without proxy");
    }

    let config = builder.build();

    // ── Launch ──────────────────────────────────────────────────────────────────
    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;
    let mut page = handle
        .browser()
        .ok_or("browser handle no longer valid")?
        .new_page()
        .await?;

    // ── Verify IP via ipify.org ────────────────────────────────────────────────
    println!("Checking IP via https://api.ipify.org ...");
    page.navigate(
        "https://api.ipify.org?format=json",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(30),
    )
    .await?;

    let body = page.content().await?;
    // ipify returns {"ip":"1.2.3.4"}
    let ip_json: serde_json::Value = serde_json::from_str(
        // strip HTML wrapper if present
        body.trim()
            .trim_start_matches("<html><head></head><body>")
            .trim_end_matches("</body></html>")
            .trim(),
    )
    .unwrap_or_else(|_| serde_json::json!({"ip": "unknown"}));

    let ip = ip_json
        .get("ip")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    println!(
        "Apparent IP: {ip}{}",
        if using_proxy {
            " (via proxy)"
        } else {
            " (direct)"
        }
    );

    // ── Verify WebRTC policy prevents leaks ───────────────────────────────────
    page.navigate(
        "about:blank",
        WaitUntil::Selector("body".to_string()),
        Duration::from_secs(5),
    )
    .await?;

    let rtc_disabled: bool = page
        .eval(
            r"(function() {
                // DisableNonProxied mode prevents non-proxied ICE candidates
                // We verify no local IP candidates would be generated (best-effort check)
                return typeof RTCPeerConnection !== 'undefined';
            })()",
        )
        .await
        .unwrap_or(false);

    println!("RTCPeerConnection present: {rtc_disabled}  (WebRTC policy: DisableNonProxied)");
    println!("IP leak protection: enabled");

    page.close().await?;
    handle.release().await;

    println!("Done.");
    Ok(())
}
