//! # stygian-browser
//!
#![doc = include_str!("../README.md")]
#![allow(clippy::multiple_crate_versions)]
#![deny(unsafe_code)] // All unsafe usage is confined to #[cfg(test)] modules with explicit #[allow]
//!
//! Browser automation and stealth tooling for sites protected by Cloudflare,
//! `DataDome`, `PerimeterX`, and Akamai Bot Manager.
//!
//! ## Features
//!
//! - **Browser pooling** — warm pool with min/max sizing, LRU eviction, and backpressure;
//!   sub-100 ms acquire from the warm queue
//! - **Anti-detection** — User-Agent patching and plugin population
//! - **Human behaviour** — Bézier-curve mouse paths, human-paced typing with typos,
//!   random scroll and micro-interactions
//! - **Fingerprint generation** — statistically-weighted device profiles matching
//!   real-world browser market share distributions
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use std::time::Duration;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
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
//!         WaitUntil::DomContentLoaded,
//!         Duration::from_secs(30),
//!     ).await?;
//!
//!     println!("Title: {}", page.title().await?);
//!
//!     handle.release().await;
//!     Ok(())
//! # }
//! ```
//!
//! ## Stealth Levels
//!
//! | Level | Navigator spoof | Canvas noise | WebGL random | CDP protection | Human behavior |
//! | ----- | --------------- | ------------ | ------------ | -------------- | -------------- |
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
//! | [`fingerprint`] | [`DeviceProfile`], [`BrowserKind`] |
//! | [`webrtc`] | [`WebRtcConfig`], [`WebRtcPolicy`], [`ProxyLocation`] |
//! | [`cdp_protection`] | CDP leak protection modes |

pub mod acquisition;
pub mod behavior_adapter;
pub mod browser;
pub mod cdp_protection;
pub mod coherence;
pub mod config;
pub mod error;
pub mod freshness;
pub mod integrity_canary;
pub mod interstitial_router;
pub mod page;
pub mod pool;
pub mod proxy;
pub mod replay_defense;

#[cfg(feature = "extract")]
pub mod extract;

#[cfg(feature = "extract")]
pub use extract::Extractable;

#[cfg(feature = "similarity")]
pub mod similarity;

#[cfg(feature = "similarity")]
pub use similarity::{ElementFingerprint, SimilarMatch, SimilarityConfig};

#[cfg(feature = "stealth")]
pub mod stealth;

#[cfg(feature = "stealth")]
pub mod behavior;

#[cfg(feature = "stealth")]
pub mod fingerprint;

#[cfg(feature = "stealth")]
pub mod tls;

#[cfg(feature = "stealth")]
pub mod webrtc;

#[cfg(feature = "stealth")]
pub mod noise;

#[cfg(feature = "stealth")]
pub mod canvas_noise;

#[cfg(feature = "stealth")]
pub mod webgl_noise;

#[cfg(feature = "stealth")]
pub mod audio_noise;

#[cfg(feature = "stealth")]
pub mod rects_noise;

#[cfg(feature = "stealth")]
pub mod cdp_hardening;

#[cfg(feature = "stealth")]
pub mod peripheral_stealth;

#[cfg(feature = "stealth")]
pub mod validation;

pub mod tls_validation;

#[cfg(feature = "stealth")]
pub mod profile;

pub mod transport_realism;

#[cfg(feature = "stealth")]
pub mod navigator_coherence;

#[cfg(feature = "stealth")]
pub mod timing_noise;

#[cfg(feature = "stealth")]
pub mod diagnostic;

#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "metrics")]
pub mod metrics;

pub mod session;

pub mod recorder;

pub use acquisition::InterstitialContext;
pub use acquisition::{
    AcquisitionMode, AcquisitionRequest, AcquisitionResult, AcquisitionRunner,
    ReplayDefenseContext, StageFailure, StageFailureKind, StrategyUsed, TransportRealismContext,
};
pub use behavior_adapter::{
    AdapterKind, AppliedBehaviorPlan, BehaviorInteractionLevel, BrowserBehaviorAdapter,
    ExecutionMode, PolymorphicBehaviorAdapter, SessionMode, TelemetryLevel,
};
pub use browser::BrowserInstance;
pub use config::{BrowserConfig, HeadlessMode, StealthLevel};
pub use error::{BrowserError, Result};
pub use freshness::{
    DomainClass, FreshnessCheckInput, FreshnessContract, FreshnessDecision, FreshnessError,
    FreshnessPolicy, FreshnessPolicyKind, FreshnessReport, InvalidationKind, InvalidationReason,
    check, signature_hash, unix_epoch_ms,
};
pub use integrity_canary::{
    IntegrityCanaryPolicy, IntegrityCanaryReport, IntegrityProbe, IntegrityProbeId,
    IntegrityProbeOutcome, IntegrityRiskClassification, IntegrityRiskScore, ProbeFinding,
    RISK_CONFIRMED_THRESHOLD_DEFAULT, RISK_SUSPECTED_THRESHOLD_DEFAULT,
    all_probes as all_integrity_probes, probe_by_id as integrity_probe_by_id,
};
pub use interstitial_router::{
    InterstitialClassifier, InterstitialKind, InterstitialPolicy, InterstitialRoute,
    InterstitialRouter, InterstitialSeverity, PageSignature, RouterDecision, classify_and_route,
    route,
};
pub use page::{
    NodeHandle, OuterHtmlResult, OuterHtmlStrategy, PageHandle, ResourceFilter, WaitUntil,
};
pub use pool::{BrowserHandle, BrowserPool, PoolStats};
pub use proxy::{DirectLease, ProxyLease, ProxySource};
pub use replay_defense::{
    ReplayDefenseCheckInput, ReplayDefenseDecision, ReplayDefenseError,
    ReplayDefenseInvalidationKind, ReplayDefensePolicy, ReplayDefenseReason, ReplayDefenseReport,
    ReplayDefenseState,
};
pub use transport_realism::{
    HEADER_ORDER_CHROME_136, HEADER_ORDER_FIREFOX_130, Http2CheckKind, Http2CheckResult,
    PSEUDO_HEADER_ORDER_CHROME_136, TransportCompatibility, TransportObservation, TransportProfile,
    TransportRealismReport, score as score_transport_realism,
};

#[cfg(feature = "stealth")]
pub use stealth::{NavigatorProfile, StealthConfig, StealthProfile};

#[cfg(feature = "stealth")]
pub use behavior::InteractionLevel;
#[cfg(feature = "stealth")]
pub use behavior::RequestPacer;
#[cfg(feature = "stealth")]
pub use fingerprint::{BrowserKind, DeviceProfile};

#[cfg(feature = "stealth")]
pub use webrtc::{ProxyLocation, WebRtcConfig, WebRtcPolicy};

pub mod prelude {
    pub use crate::config::BrowserConfig;
    pub use crate::error::{BrowserError, Result};
    pub use crate::pool::{BrowserHandle, BrowserPool, PoolStats};

    #[cfg(feature = "stealth")]
    pub use crate::stealth::{NavigatorProfile, StealthConfig, StealthProfile};
}
