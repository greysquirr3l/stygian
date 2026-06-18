//! JavaScript probe definitions for cross-context identity surfaces.
//!
//! Each probe is a self-contained JavaScript expression that
//! evaluates to a JSON string matching the [`IdentitySurface`]
//! schema (or a `{"skipped":"..."}` envelope when the context is
//! unavailable).
//!
//! All scripts:
//!
//! - Use old-style `function ()` / `var` rather than arrow
//!   functions or `let`/`const` for broadest browser-engine
//!   compatibility.
//! - Wrap reads in `try/catch` so a single missing field never
//!   aborts the whole probe.
//! - Never panic; a context that cannot be probed returns
//!   `{"skipped":"<reason>"}` which [`super::ContextObservation`]
//!   decodes into a [`Skipped`][super::ContextObservation::Skipped]
//!   marker.
//!
//! ## Worker probe synchronous-busy-wait rationale
//!
//! The worker probe posts a message to a `Worker` constructed from
//! a `Blob` URL and then busy-waits on the main thread until the
//! worker posts back its identity surface (or the deadline
//! elapses). The busy-wait is bounded to a few hundred milliseconds
//! — workers always reply in microseconds on a clean browser, and
//! the timeout ensures the runner can never block indefinitely.
//!
//! ## Iframe probe srcdoc rationale
//!
//! The iframe probe creates a same-origin `<iframe>` with a
//! `srcdoc` payload. `srcdoc` iframes inherit the parent's origin
//! and provide a deterministic, network-free context for probing.

use serde::{Deserialize, Serialize};

use crate::error::{BrowserError, Result};
use crate::page::PageHandle;

use super::report::{ContextObservation, IdentitySurface};

// ─── Probed JSON shape ────────────────────────────────────────────────────────

/// Wire format for a probe result: an [`IdentitySurface`] flattened
/// with an optional `skipped` discriminator field.
///
/// The probe script returns either:
///
/// - `{"skipped": "<reason>"}` — probe could not run; the reason
///   decodes to a [`Skipped`][ContextObservation::Skipped] marker.
/// - `{ "user_agent": "...", ... }` — a (possibly partial)
///   [`IdentitySurface`] that decodes to an
///   [`Observed`][ContextObservation::Observed] marker.
///
/// The two cases are distinguished by the presence of a
/// `skipped` field. Using a wire wrapper struct (instead of an
/// `#[serde(untagged)]` enum) keeps the discriminator explicit and
/// avoids the silent-unknown-field behaviour of untagged enums.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct ProbeOutput {
    /// `Some(reason)` when the probe could not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skipped: Option<String>,
    /// Flattened identity surface. `Default::default()` when the
    /// probe returned only a `skipped` marker.
    #[serde(default, flatten)]
    surface: IdentitySurface,
}

impl ProbeOutput {
    fn into_observation(self) -> ContextObservation {
        match self.skipped {
            Some(reason) => ContextObservation::skipped(reason),
            None => ContextObservation::observed(self.surface),
        }
    }
}

// ─── JavaScript probe scripts ─────────────────────────────────────────────────

const TOP_LEVEL_PROBE: &str = concat!(
    "(function(){",
    "var r={};",
    "try{r.userAgent=String(navigator.userAgent||'');}catch(e){r.userAgent='';}",
    "try{r.platform=String(navigator.platform||'');}catch(e){r.platform='';}",
    "try{r.languages=Array.isArray(navigator.languages)?navigator.languages.join(','):'';}catch(e){r.languages='';}",
    "try{r.hardware_concurrency=typeof navigator.hardwareConcurrency==='number'?navigator.hardwareConcurrency:null;}catch(e){r.hardware_concurrency=null;}",
    "try{r.device_memory=typeof navigator.deviceMemory==='number'?navigator.deviceMemory:null;}catch(e){r.device_memory=null;}",
    "try{r.timezone=String(Intl.DateTimeFormat().resolvedOptions().timeZone||'');}catch(e){r.timezone='';}",
    "try{r.screen_width=typeof screen==='object'&&screen&&typeof screen.width==='number'?screen.width:null;}catch(e){r.screen_width=null;}",
    "try{r.screen_height=typeof screen==='object'&&screen&&typeof screen.height==='number'?screen.height:null;}catch(e){r.screen_height=null;}",
    "try{r.color_depth=typeof screen==='object'&&screen&&typeof screen.colorDepth==='number'?screen.colorDepth:null;}catch(e){r.color_depth=null;}",
    "try{r.webdriver=typeof navigator.webdriver==='boolean'?navigator.webdriver:null;}catch(e){r.webdriver=null;}",
    "return JSON.stringify(r);",
    "})()"
);

const IFRAME_PROBE: &str = concat!(
    "(function(){",
    "try{",
    "var doc=document;",
    "var body=doc.body||doc.documentElement;",
    "if(!body){return JSON.stringify({skipped:'no document body'});}",
    "var f=doc.createElement('iframe');",
    "f.setAttribute('aria-hidden','true');",
    "f.style.cssText='position:absolute;width:0;height:0;border:0;visibility:hidden;';",
    "f.srcdoc='<!doctype html><html><head></head><body></body></html>';",
    "body.appendChild(f);",
    "var w=f.contentWindow;",
    "if(!w){try{body.removeChild(f);}catch(_e){} return JSON.stringify({skipped:'iframe contentWindow unavailable'});}",
    "var n=w.navigator;",
    "if(!n){try{body.removeChild(f);}catch(_e){} return JSON.stringify({skipped:'iframe navigator unavailable'});}",
    "var r={};",
    "try{r.user_agent=String(n.userAgent||'');}catch(e){r.user_agent='';}",
    "try{r.platform=String(n.platform||'');}catch(e){r.platform='';}",
    "try{r.languages=Array.isArray(n.languages)?n.languages.join(','):'';}catch(e){r.languages='';}",
    "try{r.hardware_concurrency=typeof n.hardwareConcurrency==='number'?n.hardwareConcurrency:null;}catch(e){r.hardware_concurrency=null;}",
    "try{r.device_memory=typeof n.deviceMemory==='number'?n.deviceMemory:null;}catch(e){r.device_memory=null;}",
    "try{r.timezone=String(w.Intl.DateTimeFormat().resolvedOptions().timeZone||'');}catch(e){r.timezone='';}",
    "try{r.screen_width=w.screen&&typeof w.screen.width==='number'?w.screen.width:null;}catch(e){r.screen_width=null;}",
    "try{r.screen_height=w.screen&&typeof w.screen.height==='number'?w.screen.height:null;}catch(e){r.screen_height=null;}",
    "try{r.color_depth=w.screen&&typeof w.screen.colorDepth==='number'?w.screen.colorDepth:null;}catch(e){r.color_depth=null;}",
    "try{r.webdriver=typeof n.webdriver==='boolean'?n.webdriver:null;}catch(e){r.webdriver=null;}",
    "try{body.removeChild(f);}catch(_e){}",
    "return JSON.stringify(r);",
    "}catch(e){",
    "return JSON.stringify({skipped:'iframe probe failed: '+(e&&e.message?e.message:String(e))});",
    "}",
    "})()"
);

// The worker probe embeds a multi-line JS program as a string
// passed to `new Blob([...])`. The script is a JS string literal in
// the host probe; the Rust raw string below contains that JS string
// verbatim (with `\n` → newline in JS, because the entire raw
// string is the JS source).
const WORKER_PROBE: &str = r#"(function(){
try{
if(typeof Worker==='undefined'){return JSON.stringify({skipped:'Worker unsupported'});}
if(typeof Blob==='undefined'||typeof URL==='undefined'||typeof URL.createObjectURL!=='function'){return JSON.stringify({skipped:'Blob/URL.createObjectURL unsupported'});}
var src="self.onmessage=function(_e){var r={};try{r.user_agent=String(navigator.userAgent||'');}catch(_err){r.user_agent='';}try{r.platform=String(navigator.platform||'');}catch(_err){r.platform='';}try{r.languages=Array.isArray(navigator.languages)?navigator.languages.join(','):'';}catch(_err){r.languages='';}try{r.hardware_concurrency=typeof navigator.hardwareConcurrency==='number'?navigator.hardwareConcurrency:null;}catch(_err){r.hardware_concurrency=null;}try{r.device_memory=typeof navigator.deviceMemory==='number'?navigator.deviceMemory:null;}catch(_err){r.device_memory=null;}try{r.timezone=String(Intl.DateTimeFormat().resolvedOptions().timeZone||'');}catch(_err){r.timezone='';}try{r.webdriver=typeof navigator.webdriver==='boolean'?navigator.webdriver:null;}catch(_err){r.webdriver=null;}try{self.postMessage(JSON.stringify(r));self.close();}catch(_err){self.postMessage('SKIP:'+(_err&&_err.message?_err.message:String(_err)));self.close();}};";
var blob;
try{blob=new Blob([src],{type:'application/javascript'});}catch(e){return JSON.stringify({skipped:'Blob construction failed: '+(e&&e.message?e.message:String(e))});}
var url;
try{url=URL.createObjectURL(blob);}catch(e){return JSON.stringify({skipped:'createObjectURL failed: '+(e&&e.message?e.message:String(e))});}
var worker;
try{worker=new Worker(url);}catch(e){try{URL.revokeObjectURL(url);}catch(_e){} return JSON.stringify({skipped:'new Worker failed: '+(e&&e.message?e.message:String(e))});}
var result=null;
var errorReason='';
worker.onmessage=function(ev){result=ev&&typeof ev.data==='string'?ev.data:null;};
worker.onerror=function(ev){errorReason=(ev&&ev.message)?ev.message:'unknown worker error';};
try{worker.postMessage('probe');}catch(e){try{worker.terminate();}catch(_e){} try{URL.revokeObjectURL(url);}catch(_e){} return JSON.stringify({skipped:'postMessage failed: '+(e&&e.message?e.message:String(e))});}
var deadline=Date.now()+2000;
while(result===null&&errorReason===''&&Date.now()<deadline){}
try{worker.terminate();}catch(_e){}
try{URL.revokeObjectURL(url);}catch(_e){}
if(errorReason!==''){return JSON.stringify({skipped:'worker error: '+errorReason});}
if(result===null){return JSON.stringify({skipped:'worker probe timeout'});}
if(typeof result==='string'&&result.indexOf('SKIP:')===0){return JSON.stringify({skipped:result.substring(5)});}
return result;
}catch(e){
return JSON.stringify({skipped:'worker probe failed: '+(e&&e.message?e.message:String(e))});
}
})()"#;

// ─── CoherenceProbe runner ────────────────────────────────────────────────────

/// Runner that executes the cross-context identity probes via CDP and
/// aggregates the results into a [`CoherenceDriftReport`][super::CoherenceDriftReport].
///
/// `CoherenceProbe` is the **default-on** entry point for cross-context
/// stealth coherence. Worker probes are best-effort: when the runtime
/// does not expose `Worker`/`Blob`/`URL.createObjectURL`, the worker
/// slot in the report is populated with
/// [`ContextObservation::Skipped`] rather than panicking.
///
/// # Idempotence
///
/// All probe methods are safely re-runnable on the same page; they
/// do not mutate the DOM beyond a transient iframe that is removed
/// at the end of the probe, and the worker is `terminate()`-ed and
/// its `Blob` URL is `revokeObjectURL`-ed before the probe returns.
///
/// # Feature flag
///
/// The coherence module is **default-on**; no feature gate is
/// required. The probe runner requires a live browser page (i.e.
/// the existing `browser-cdp` capability, which is the
/// `stygian-browser` default).
///
/// # Example
///
/// ```no_run
/// # async fn run() -> stygian_browser::error::Result<()> {
/// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
/// use stygian_browser::coherence::CoherenceProbe;
/// use std::time::Duration;
///
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let handle = pool.acquire().await?;
/// let mut page = handle
///     .browser()
///     .expect("valid browser")
///     .new_page()
///     .await?;
/// page.navigate(
///     "https://example.com",
///     WaitUntil::DomContentLoaded,
///     Duration::from_secs(30),
/// )
/// .await?;
///
/// let probe = CoherenceProbe::default();
/// let report = probe.run(&page).await?;
/// println!(
///     "coherent={} hard_drift={} contexts={}/3",
///     report.is_coherent(),
///     report.has_hard_drift(),
///     report.observed_context_count(),
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct CoherenceProbe {
    _private: (),
}

impl CoherenceProbe {
    /// Build a new runner with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Run all three probes (top, iframe, worker) and build a
    /// [`CoherenceDriftReport`][super::CoherenceDriftReport].
    ///
    /// Per-context probe failures never abort the run; the failing
    /// context is recorded as a [`Skipped`][ContextObservation::Skipped]
    /// marker.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError`] only when CDP itself is unreachable
    /// (no live browser connection). Per-probe script errors are
    /// captured as skipped observations.
    pub async fn run(&self, page: &PageHandle) -> Result<super::CoherenceDriftReport> {
        let top = self.probe_top(page).await;
        let iframe = self.probe_iframe(page).await;
        let worker = self.probe_worker(page).await;
        Ok(super::report::build_report(top, iframe, worker, None))
    }

    /// Run all three probes **and** evaluate a
    /// [`crate::freshness::FreshnessContract`] against the
    /// top-level identity signature.
    ///
    /// The contract is checked against the top-level
    /// [`IdentitySurface`] signature; the resulting
    /// [`crate::freshness::FreshnessReport`] is attached to the
    /// returned drift report under [`super::CoherenceDriftReport::freshness`].
    ///
    /// # Errors
    ///
    /// See [`Self::run`].
    pub async fn run_with_freshness(
        &self,
        page: &PageHandle,
        contract: &crate::freshness::FreshnessContract,
    ) -> Result<super::CoherenceDriftReport> {
        let top = self.probe_top(page).await;
        let iframe = self.probe_iframe(page).await;
        let worker = self.probe_worker(page).await;

        // Build the freshness check input from the top-level surface
        // signature, when available.
        let freshness_report = match &top {
            ContextObservation::Observed { surface } => {
                let observed_signature = super::report::surface_signature(surface);
                let input = crate::freshness::FreshnessCheckInput::new(
                    &contract.domain,
                    Some(observed_signature.as_str()),
                    crate::freshness::unix_epoch_ms(),
                );
                let report = crate::freshness::FreshnessReport::evaluate(contract, &input);
                report.log();
                Some(report)
            }
            ContextObservation::Skipped { .. } => None,
        };

        Ok(super::report::build_report(
            top,
            iframe,
            worker,
            freshness_report,
        ))
    }

    /// Run only the top-level document probe.
    ///
    /// Useful for unit tests or callers that want a single-context
    /// snapshot without paying for the iframe + worker probes.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError`] on CDP failure. Script errors
    /// surface as a [`Skipped`][ContextObservation::Skipped]
    /// observation.
    pub async fn probe_top(&self, page: &PageHandle) -> ContextObservation {
        run_probe(page, TOP_LEVEL_PROBE, "top-level").await
    }

    /// Run only the same-origin iframe probe.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError`] on CDP failure. Script errors
    /// surface as a [`Skipped`][ContextObservation::Skipped]
    /// observation.
    pub async fn probe_iframe(&self, page: &PageHandle) -> ContextObservation {
        run_probe(page, IFRAME_PROBE, "iframe").await
    }

    /// Run only the dedicated/shared worker probe.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError`] on CDP failure. Script errors
    /// surface as a [`Skipped`][ContextObservation::Skipped]
    /// observation.
    pub async fn probe_worker(&self, page: &PageHandle) -> ContextObservation {
        run_probe(page, WORKER_PROBE, "worker").await
    }
}

// ─── Internal: run a single probe and decode the JSON envelope ────────────────

async fn run_probe(page: &PageHandle, script: &str, label: &'static str) -> ContextObservation {
    let json: String = match page.eval(script).await {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                context = label,
                error = %err,
                "coherence probe CDP evaluation failed",
            );
            return ContextObservation::skipped(format!("{label} CDP failure: {err}"));
        }
    };
    match serde_json::from_str::<ProbeOutput>(&json) {
        Ok(out) => out.into_observation(),
        Err(err) => {
            tracing::warn!(
                context = label,
                error = %err,
                raw = %json,
                "coherence probe returned invalid JSON",
            );
            ContextObservation::skipped(format!(
                "{label} JSON decode failed: {err}"
            ))
        }
    }
}

// Compile-time guarantee that the public error type accepts our
// `Result` alias; silences dead-code lints on `BrowserError` in
// feature combinations where the import would otherwise be unused.
#[allow(dead_code)]
const _: fn() -> Option<BrowserError> = || None;

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coherence::report::{
        ContextKind, ContextPair, DriftSeverity, build_report,
    };

    #[test]
    fn top_level_probe_uses_old_style_function_and_json_stringify() {
        // Old-style IIFE + var for broadest browser-engine compatibility
        assert!(TOP_LEVEL_PROBE.starts_with("(function(){"));
        assert!(TOP_LEVEL_PROBE.contains("var r={};"));
        assert!(TOP_LEVEL_PROBE.contains("JSON.stringify(r)"));
        assert!(!TOP_LEVEL_PROBE.contains("=>"), "no arrow functions");
        assert!(
            !TOP_LEVEL_PROBE.contains("let "),
            "no `let` declarations"
        );
        assert!(
            !TOP_LEVEL_PROBE.contains("const "),
            "no `const` declarations"
        );
    }

    #[test]
    fn iframe_probe_creates_srcdoc_iframe() {
        assert!(IFRAME_PROBE.contains("srcdoc="));
        assert!(IFRAME_PROBE.contains("createElement('iframe')"));
        assert!(IFRAME_PROBE.contains("appendChild"));
        // Cleanup is required for idempotence
        assert!(IFRAME_PROBE.contains("removeChild"));
    }

    #[test]
    fn worker_probe_uses_blob_url_and_terminates() {
        assert!(WORKER_PROBE.contains("new Worker("));
        assert!(WORKER_PROBE.contains("createObjectURL"));
        assert!(WORKER_PROBE.contains("terminate"));
        assert!(WORKER_PROBE.contains("revokeObjectURL"));
        // Bounded busy-wait so the runner can never block indefinitely
        assert!(WORKER_PROBE.contains("Date.now()+2000"));
    }

    #[test]
    fn all_probes_wrap_in_iife() {
        for (name, script) in [
            ("top", TOP_LEVEL_PROBE),
            ("iframe", IFRAME_PROBE),
            ("worker", WORKER_PROBE),
        ] {
            let trimmed = script.trim_start();
            assert!(
                trimmed.starts_with("(function(){"),
                "{name} probe must be a self-invoking function expression"
            );
            assert!(
                script.trim_end().ends_with(")()"),
                "{name} probe must end with the IIFE invocation"
            );
        }
    }

    #[test]
    fn probe_output_skipped_decodes_to_observation() {
        let raw = r#"{"skipped":"Worker unsupported"}"#;
        let parsed: ProbeOutput = serde_json::from_str(raw).expect("decode");
        let obs = parsed.into_observation();
        assert!(obs.is_skipped());
        assert!(!obs.is_observed());
    }

    #[test]
    fn probe_output_observed_decodes_to_observation() {
        let raw = r#"{"user_agent":"Mozilla/5.0","platform":"MacIntel","languages":"en-US","hardware_concurrency":8,"device_memory":8,"timezone":"UTC","screen_width":1920,"screen_height":1080,"color_depth":24,"webdriver":false}"#;
        let parsed: ProbeOutput = serde_json::from_str(raw).expect("decode");
        let obs = parsed.into_observation();
        let surface = obs.surface().expect("surface present");
        assert_eq!(surface.user_agent.as_deref(), Some("Mozilla/5.0"));
        assert_eq!(surface.hardware_concurrency, Some(8));
        assert_eq!(surface.webdriver, Some(false));
    }

    #[test]
    fn probe_output_partial_surface_decodes_cleanly() {
        // Missing fields are allowed; the deserialiser fills them with None.
        let raw = r#"{"user_agent":"Mozilla/5.0"}"#;
        let parsed: ProbeOutput = serde_json::from_str(raw).expect("decode");
        let surface = parsed.into_observation().surface().expect("surface").clone();
        assert_eq!(surface.user_agent.as_deref(), Some("Mozilla/5.0"));
        assert!(surface.platform.is_none());
        assert!(surface.webdriver.is_none());
    }

    #[test]
    fn probe_output_invalid_falls_back_to_observed_with_empty_surface() {
        // A bare `null` is not a valid struct; the runner treats
        // JSON decode failures as a `Skipped` marker (see
        // `run_probe`), not a panicking decode error.
        let raw = "null";
        let parsed: serde_json::Result<ProbeOutput> = serde_json::from_str(raw);
        assert!(parsed.is_err());

        // An empty `{}` IS a valid struct — it decodes to a default
        // (empty) `IdentitySurface` with no `skipped` marker, which
        // is the contract for "context probed but produced nothing".
        let parsed: ProbeOutput = serde_json::from_str("{}").expect("decode empty object");
        let obs = parsed.into_observation();
        assert!(obs.is_observed());
        assert!(obs.surface().expect("surface").is_empty());
    }

    #[test]
    fn build_report_with_three_skipped_contexts_emits_no_drift() {
        let report = build_report(
            ContextObservation::skipped("a"),
            ContextObservation::skipped("b"),
            ContextObservation::skipped("c"),
            None,
        );
        assert!(report.is_coherent());
        assert_eq!(report.observed_context_count(), 0);
        assert_eq!(report.skipped_context_count(), 3);
    }

    #[test]
    fn coherence_probe_default_is_constructible() {
        let _ = CoherenceProbe::default();
        let _ = CoherenceProbe::new();
    }

    #[test]
    fn drift_severity_classifies_hard_fields() {
        assert_eq!(
            super::super::report::field_severity("user_agent"),
            DriftSeverity::Hard
        );
        assert_eq!(
            super::super::report::field_severity("platform"),
            DriftSeverity::Hard
        );
        assert_eq!(
            super::super::report::field_severity("languages"),
            DriftSeverity::Hard
        );
        assert_eq!(
            super::super::report::field_severity("webdriver"),
            DriftSeverity::Hard
        );
        assert_eq!(
            super::super::report::field_severity("hardware_concurrency"),
            DriftSeverity::KnownLimitation
        );
        assert_eq!(
            super::super::report::field_severity("device_memory"),
            DriftSeverity::KnownLimitation
        );
    }

    #[test]
    fn context_kind_constants_resolve_internally() {
        // Sanity-check the sides() resolver used by the comparison
        // helpers; this catches typos in the ContextPair → ContextKind
        // mapping before they reach a live browser.
        assert_eq!(
            ContextPair::TopIframe.sides(),
            (ContextKind::Top, ContextKind::Iframe)
        );
        assert_eq!(
            ContextPair::TopWorker.sides(),
            (ContextKind::Top, ContextKind::Worker)
        );
        assert_eq!(
            ContextPair::IframeWorker.sides(),
            (ContextKind::Iframe, ContextKind::Worker)
        );
    }
}
