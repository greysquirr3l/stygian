//! Cross-context stealth coherence probes.
//!
//! Verifies that the browser's identity surface (user agent,
//! platform, languages, `navigator.webdriver`, screen metrics) is
//! **coherent across the top-level document, same-origin iframes,
//! and dedicated/shared workers**. Drift between contexts is a
//! strong anti-bot detection signal — most anti-bot vendors probe
//! at least one of these auxiliary contexts and compare.
//!
//! ## What is a "context"?
//!
//! The browser exposes three isolation levels that an anti-bot
//! script can probe:
//!
//! - **Top-level document** — `window` of the main frame.
//! - **Same-origin iframe** — a `<iframe srcdoc=…>` injected into
//!   the document at probe time. `srcdoc` iframes inherit the
//!   parent's origin and provide a deterministic, network-free
//!   sub-context.
//! - **Dedicated/shared worker** — a `Worker` constructed from a
//!   `Blob` URL. The worker has its own `WorkerGlobalScope` and
//!   its own `navigator`. Worker probes are **best-effort**: when
//!   the runtime does not expose `Worker` / `Blob` /
//!   `URL.createObjectURL`, the worker slot in the report is
//!   populated with a [`ContextObservation::Skipped`] marker
//!   rather than panicking.
//!
//! ## Feature flag
//!
//! This module is **default-on** and is always compiled as part of
//! the `stygian-browser` crate. It requires CDP (the
//! `stygian-browser` default), since the probe runner uses
//! `PageHandle::eval` to send JavaScript to the page.
//!
//! ## Hard failures vs known limitations
//!
//! Drift is split into two severity bands (see
//! [`report::DriftSeverity`]):
//!
//! - **Hard** — `user_agent`, `platform`, `languages`, and
//!   `navigator.webdriver` MUST be identical across all
//!   observed contexts.
//! - **Known limitation** — `hardware_concurrency`,
//!   `device_memory`, screen metrics, and timezone are documented
//!   to differ between Document and Worker contexts in some
//!   browser engines.
//!
//! ## Idempotence
//!
//! All probe methods are safely re-runnable. The iframe is removed
//! from the DOM at the end of the probe, the worker is
//! `terminate()`-ed, and the `Blob` URL is `revokeObjectURL`-ed
//! before the probe returns.
//!
//! # Example
//!
//! ```no_run
//! # async fn run() -> stygian_browser::error::Result<()> {
//! use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use stygian_browser::coherence::CoherenceProbe;
//! use std::time::Duration;
//!
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let handle = pool.acquire().await?;
//! let mut page = handle
//!     .browser()
//!     .expect("valid browser")
//!     .new_page()
//!     .await?;
//! page.navigate(
//!     "https://example.com",
//!     WaitUntil::DomContentLoaded,
//!     Duration::from_secs(30),
//! )
//! .await?;
//!
//! let probe = CoherenceProbe::new();
//! let report = probe.run(&page).await?;
//! println!(
//!     "coherent={} hard_drift={} contexts={}/3",
//!     report.is_coherent(),
//!     report.has_hard_drift(),
//!     report.observed_context_count(),
//! );
//! # Ok(())
//! # }
//! ```

pub mod probes;
pub mod report;

pub use probes::CoherenceProbe;
pub use report::{
    CoherenceDriftReport, ContextKind, ContextObservation, ContextPair, DriftDiagnostic,
    DriftSeverity, IdentitySurface, build_report, diff_surfaces, field_severity, surface_signature,
};
