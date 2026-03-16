//! # stygian-browser
//!
#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]
#![deny(unsafe_code)] // All unsafe usage is confined to #[cfg(test)] modules with explicit #[allow]
//! High-performance, anti-detection browser automation library for Rust.
//!
//! Built on Chrome `DevTools` Protocol (CDP) via [`chromiumoxide`](https://github.com/mattsse/chromiumoxide)
//! with comprehensive stealth features to bypass modern anti-bot systems:
//! Cloudflare, `DataDome`, `PerimeterX`, and Akamai Bot Manager.
//!
//! ## Features
//!
//! - **Browser pooling** — warm pool with min/max sizing, LRU eviction, and backpressure;
//!   sub-100 ms acquire from the warm queue
//! - **Anti-detection** — `navigator` spoofing, canvas noise, WebGL randomisation,
//!   User-Agent patching, and plugin population
//! - **Human behaviour** — Bézier-curve mouse paths, human-paced typing with typos,
//!   random scroll and micro-interactions
//! - **CDP leak protection** — hides `Runtime.enable` side-effects that expose automation
//! - **WebRTC control** — block, proxy-route, or allow WebRTC to prevent IP leaks
//! - **Fingerprint generation** — statistically-weighted device profiles matching
//!   real-world browser market share distributions
//! - **Stealth levels** — `None` / `Basic` / `Advanced` for tuning evasion vs performance
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Default config: headless, Advanced stealth, pool of 2–10 browsers
//!     let config = BrowserConfig::default();
//!     let pool = BrowserPool::new(config).await?;
//!
//!     // Acquire a browser from the warm pool (< 100 ms)
//!     let handle = pool.acquire().await?;
//!
//!     // Open a tab and navigate
//!     let mut page = handle.browser().expect("valid browser").new_page().await?;
//!     page.navigate(
//!         "https://example.com",
//!         WaitUntil::Selector("body".to_string()),
//!         Duration::from_secs(30),
//!     ).await?;
//!
//!     println!("Title: {}", page.title().await?);
//!
//!     // Return the browser to the pool
//!     handle.release().await;
//!     Ok(())
//! }
//! ```
//!
//! ## Stealth Levels
//!
//! | Level | `navigator` | Canvas | WebGL | CDP protect | Human behavior |
//! | ------- |:-----------:|:------:|:-----:|:-----------:|:--------------:|
//! | `None` | — | — | — | — | — |
//! | `Basic` | ✓ | — | — | ✓ | — |
//! | `Advanced` | ✓ | ✓ | ✓ | ✓ | ✓ |
//!
//! ## Module Overview
//!
//! | Module | Description |
//! | -------- | ------------- |
//! | [`browser`] | [`BrowserInstance`] — launch, health-check, shutdown |
//! | [`pool`] | [`BrowserPool`] + [`BrowserHandle`] — warm pool management |
//! | [`page`] | [`PageHandle`] — navigate, eval, content, cookies |
//! | [`config`] | [`BrowserConfig`] + builder pattern |
//! | [`error`] | [`BrowserError`] and [`Result`] alias |
//! | [`stealth`] | [`StealthProfile`], [`NavigatorProfile`] |
//! | [`fingerprint`] | [`DeviceProfile`], [`BrowserKind`] |
//! | [`behavior`] | [`behavior::MouseSimulator`], [`behavior::TypingSimulator`] |
//! | [`webrtc`] | [`WebRtcConfig`], [`WebRtcPolicy`], [`ProxyLocation`] |
//! | [`cdp_protection`] | CDP leak protection modes |

pub mod browser;
pub mod cdp_protection;
pub mod config;
pub mod error;
pub mod page;
pub mod pool;

#[cfg(feature = "stealth")]
pub mod stealth;

#[cfg(feature = "stealth")]
pub mod behavior;

#[cfg(feature = "stealth")]
pub mod fingerprint;

#[cfg(feature = "stealth")]
pub mod webrtc;

#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "metrics")]
pub mod metrics;

pub mod session;

pub mod recorder;

// Re-exports for convenience
pub use browser::BrowserInstance;
pub use config::{BrowserConfig, HeadlessMode, StealthLevel};
pub use error::{BrowserError, Result};
pub use page::{PageHandle, ResourceFilter, WaitUntil};
pub use pool::{BrowserHandle, BrowserPool, PoolStats};

#[cfg(feature = "stealth")]
pub use stealth::{NavigatorProfile, StealthConfig, StealthProfile};

#[cfg(feature = "stealth")]
pub use behavior::InteractionLevel;
pub use fingerprint::{BrowserKind, DeviceProfile};

#[cfg(feature = "stealth")]
pub use webrtc::{ProxyLocation, WebRtcConfig, WebRtcPolicy};

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::config::BrowserConfig;
    pub use crate::error::{BrowserError, Result};
    pub use crate::pool::{BrowserHandle, BrowserPool, PoolStats};

    #[cfg(feature = "stealth")]
    pub use crate::stealth::{NavigatorProfile, StealthConfig, StealthProfile};
}
