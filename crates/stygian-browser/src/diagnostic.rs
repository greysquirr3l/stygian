//! Stealth self-diagnostic — JavaScript detection checks.
//!
//! Defines a catalogue of JavaScript snippets that detect common browser-
//! automation telltales when evaluated inside a live browser context via
//! CDP `Runtime.evaluate`.
//!
//! Each check script evaluates to a JSON string:
//!
//! ```json
//! { "passed": true, "details": "..." }
//! ```
//!
//! # Usage
//!
//! 1. Iterate [`all_checks`] to get the built-in check catalogue.
//! 2. For each [`DetectionCheck`], send `check.script` to the browser via
//!    CDP and collect the returned JSON string.
//! 3. Call [`DetectionCheck::parse_output`] to get a [`CheckResult`].
//! 4. Aggregate with [`DiagnosticReport::new`].
//!
//! # Example
//!
//! ```
//! use stygian_browser::diagnostic::{all_checks, DiagnosticReport};
//!
//! // Simulate every check returning a passing result
//! let results = all_checks()
//!     .iter()
//!     .map(|check| check.parse_output(r#"{"passed":true,"details":"ok"}"#))
//!     .collect::<Vec<_>>();
//!
//! let report = DiagnosticReport::new(results);
//! assert!(report.is_clean());
//! ```

use serde::{Deserialize, Serialize};

// ── CheckId ───────────────────────────────────────────────────────────────────

/// Stable identifier for a built-in stealth detection check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckId {
    /// `navigator.webdriver` must be `undefined` or `false`.
    WebDriverFlag,
    /// `window.chrome.runtime` must be present (absent in headless by default).
    ChromeObject,
    /// `navigator.plugins` must have at least one entry.
    PluginCount,
    /// `navigator.languages` must be non-empty.
    LanguagesPresent,
    /// Canvas `toDataURL()` must return non-trivial image data.
    CanvasConsistency,
    /// WebGL vendor/renderer must not contain the `SwiftShader` software-renderer marker.
    WebGlVendor,
    /// No automation-specific globals (`__puppeteer__`, `__playwright`, etc.) must be present.
    AutomationGlobals,
    /// `window.outerWidth` and `window.outerHeight` must be non-zero.
    OuterWindowSize,
    /// `navigator.userAgent` must not contain the `"HeadlessChrome"` substring.
    HeadlessUserAgent,
    /// `Notification.permission` must not be pre-granted (automation artefact).
    NotificationPermission,
    /// `window.matchMedia` must be a function (PX env-bitmask bit 0).
    MatchMediaPresent,
    /// `document.elementFromPoint` must be a function (PX env-bitmask bit 1).
    ElementFromPointPresent,
    /// `window.requestAnimationFrame` must be a function (PX env-bitmask bit 2).
    RequestAnimationFramePresent,
    /// `window.getComputedStyle` must be a function (PX env-bitmask bit 3).
    GetComputedStylePresent,
    /// `CSS.supports` must exist and be callable (PX env-bitmask bit 4).
    CssSupportsPresent,
    /// `navigator.sendBeacon` must be a function (PX env-bitmask bit 5).
    SendBeaconPresent,
    /// `document.execCommand` must be a function (PX env-bitmask bit 6).
    ExecCommandPresent,
    /// `process.versions.node` must be absent — not a Node.js environment (PX env-bitmask bit 7).
    NodeJsAbsent,
    /// `Navigator.prototype.webdriver` should look like a native accessor descriptor.
    WebDriverDescriptorShape,
    /// `navigator.userAgentData` should exist and expose coherent client hints.
    UserAgentDataPresent,
    /// `navigator.connection` should expose plausible network information.
    ConnectionPresent,
    /// Hidden font-probe elements should yield non-zero layout measurements.
    HiddenFontProbeRect,
    /// `screen` metrics and `devicePixelRatio` should be plausible and coherent.
    ScreenMetricsCoherent,
    /// The Web Audio surface should exist and expose a non-zero sample rate.
    AudioContextPresent,
}

/// Stable identifier for a browser surface we do not yet spoof or validate fully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitationId {
    /// WebGPU / `navigator.gpu` is exposed but not yet spoofed or validated.
    WebGpuSurface,
    /// `performance.memory` is exposed but not yet spoofed or validated.
    PerformanceMemorySurface,
    /// `navigator.storage` may be unavailable on opaque origins (e.g. `about:blank`).
    OpaqueOriginStorage,
}

// ── CheckResult ───────────────────────────────────────────────────────────────

/// The outcome of running a single detection check in the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Which check produced this result.
    pub id: CheckId,
    /// Human-readable description of what was tested.
    pub description: String,
    /// `true` if the browser appears legitimate for this check.
    pub passed: bool,
    /// Diagnostic detail returned by the JavaScript evaluation.
    pub details: String,
}

/// A known browser surface that is visible but not yet covered by stealth diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownLimitation {
    /// Which limitation was observed.
    pub id: LimitationId,
    /// Human-readable description of the uncovered surface.
    pub description: String,
    /// Runtime detail from the page context.
    pub details: String,
}

/// Optional observed transport fingerprints to compare against expected values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportObservations {
    /// Observed JA3 hash (lower/upper hex accepted).
    pub ja3_hash: Option<String>,
    /// Observed JA4 fingerprint string.
    pub ja4: Option<String>,
    /// Observed HTTP/3 perk text (`SETTINGS|PSEUDO_HEADERS`).
    pub http3_perk_text: Option<String>,
    /// Observed HTTP/3 perk hash.
    pub http3_perk_hash: Option<String>,
}

/// Transport-level diagnostics emitted alongside JavaScript stealth checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportDiagnostic {
    /// User-Agent sampled from the live page.
    pub user_agent: String,
    /// Built-in profile name inferred from User-Agent, if any.
    pub expected_profile: Option<String>,
    /// Expected JA3 raw string from the inferred profile.
    pub expected_ja3_raw: Option<String>,
    /// Expected JA3 hash from the inferred profile.
    pub expected_ja3_hash: Option<String>,
    /// Expected JA4 fingerprint from the inferred profile.
    pub expected_ja4: Option<String>,
    /// Expected HTTP/3 perk text derived from User-Agent.
    pub expected_http3_perk_text: Option<String>,
    /// Expected HTTP/3 perk hash derived from User-Agent.
    pub expected_http3_perk_hash: Option<String>,
    /// Caller-supplied observed transport values.
    pub observed: TransportObservations,
    /// `true` when all supplied observations match expected fingerprints.
    /// `None` when no observations were supplied.
    pub transport_match: Option<bool>,
    /// Human-readable mismatch reasons.
    pub mismatches: Vec<String>,
}

impl TransportDiagnostic {
    /// Build transport diagnostics from `user_agent` and optional observations.
    #[must_use]
    pub fn from_user_agent_and_observations(
        user_agent: &str,
        observed: Option<&TransportObservations>,
    ) -> Self {
        let observed = observed.cloned().unwrap_or_default();

        // Resolve profile once; derive all fingerprints from it to avoid repeated UA parsing.
        let expected_profile = crate::tls::expected_tls_profile_from_user_agent(user_agent);
        let expected_ja3 = expected_profile.map(crate::tls::TlsProfile::ja3);
        let expected_ja4 = expected_profile.map(crate::tls::TlsProfile::ja4);
        let expected_http3 = expected_profile.and_then(crate::tls::TlsProfile::http3_perk);

        let mut mismatches = Vec::new();

        if let (Some(expected), Some(observed_hash)) = (
            expected_ja3.as_ref().map(|j| j.hash.as_str()),
            observed.ja3_hash.as_deref(),
        ) && !observed_hash.eq_ignore_ascii_case(expected)
        {
            mismatches.push(format!(
                "ja3_hash mismatch: expected '{expected}', observed '{observed_hash}'"
            ));
        }

        if let (Some(expected), Some(observed_ja4)) = (
            expected_ja4.as_ref().map(|j| j.fingerprint.as_str()),
            observed.ja4.as_deref(),
        ) && observed_ja4 != expected
        {
            mismatches.push(format!(
                "ja4 mismatch: expected '{expected}', observed '{observed_ja4}'"
            ));
        }

        if let Some(expected) = expected_http3.as_ref() {
            let cmp = expected.compare(
                observed.http3_perk_text.as_deref(),
                observed.http3_perk_hash.as_deref(),
            );
            mismatches.extend(cmp.mismatches);
        }

        // If callers supplied observed transport fields that cannot be compared
        // due to missing expectations, surface that explicitly instead of
        // reporting a false positive match.
        if observed.ja3_hash.is_some() && expected_ja3.is_none() {
            mismatches.push(
                "ja3_hash was provided but no expected JA3 could be derived from user-agent"
                    .to_string(),
            );
        }
        if observed.ja4.is_some() && expected_ja4.is_none() {
            mismatches.push(
                "ja4 was provided but no expected JA4 could be derived from user-agent".to_string(),
            );
        }
        if (observed.http3_perk_text.is_some() || observed.http3_perk_hash.is_some())
            && expected_http3.is_none()
        {
            mismatches.push(
                "http3 perk observation was provided but no expected HTTP/3 fingerprint could be derived from user-agent"
                    .to_string(),
            );
        }

        let has_observed = observed.ja3_hash.is_some()
            || observed.ja4.is_some()
            || observed.http3_perk_text.is_some()
            || observed.http3_perk_hash.is_some();

        Self {
            user_agent: user_agent.to_string(),
            expected_profile: expected_profile.map(|p| p.name.clone()),
            expected_ja3_raw: expected_ja3.as_ref().map(|j| j.raw.clone()),
            expected_ja3_hash: expected_ja3.as_ref().map(|j| j.hash.clone()),
            expected_ja4: expected_ja4.as_ref().map(|j| j.fingerprint.clone()),
            expected_http3_perk_text: expected_http3
                .as_ref()
                .map(crate::tls::Http3Perk::perk_text),
            expected_http3_perk_hash: expected_http3
                .as_ref()
                .map(crate::tls::Http3Perk::perk_hash),
            observed,
            transport_match: has_observed.then_some(mismatches.is_empty()),
            mismatches,
        }
    }
}

// ── DiagnosticReport ──────────────────────────────────────────────────────────

/// Aggregate result from running all detection checks.
///
/// # Example
///
/// ```
/// use stygian_browser::diagnostic::{all_checks, DiagnosticReport};
///
/// let results = all_checks()
///     .iter()
///     .map(|c| c.parse_output(r#"{"passed":true,"details":"ok"}"#))
///     .collect::<Vec<_>>();
/// let report = DiagnosticReport::new(results);
/// assert!(report.is_clean());
/// assert!((report.coverage_pct() - 100.0).abs() < 0.001);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    /// Individual check results in catalogue order.
    pub checks: Vec<CheckResult>,
    /// Number of checks where `passed == true`.
    pub passed_count: usize,
    /// Number of checks where `passed == false`.
    pub failed_count: usize,
    /// Browser surfaces observed at runtime that are not yet covered fully.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_limitations: Vec<KnownLimitation>,
    /// Optional transport-layer diagnostics (JA3/JA4/HTTP3 perk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportDiagnostic>,
}

impl DiagnosticReport {
    /// Build a report from an ordered list of check results.
    pub fn new(checks: Vec<CheckResult>) -> Self {
        let passed_count = checks.iter().filter(|r| r.passed).count();
        let failed_count = checks.len() - passed_count;
        Self {
            checks,
            passed_count,
            failed_count,
            known_limitations: Vec::new(),
            transport: None,
        }
    }

    /// Attach known browser-surface limitations to this report.
    #[must_use]
    pub fn with_known_limitations(mut self, known_limitations: Vec<KnownLimitation>) -> Self {
        self.known_limitations = known_limitations;
        self
    }

    /// Attach transport diagnostics to this report.
    #[must_use]
    pub fn with_transport(mut self, transport: TransportDiagnostic) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Returns `true` when every check passed.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.failed_count == 0
    }

    /// Percentage of checks that passed (0.0–100.0).
    #[allow(clippy::cast_precision_loss)]
    pub fn coverage_pct(&self) -> f64 {
        if self.checks.is_empty() {
            return 0.0;
        }
        self.passed_count as f64 / self.checks.len() as f64 * 100.0
    }

    /// Iterate over all checks that returned `passed: false`.
    pub fn failures(&self) -> impl Iterator<Item = &CheckResult> {
        self.checks.iter().filter(|r| !r.passed)
    }
}

// ── DetectionCheck ────────────────────────────────────────────────────────────

/// A single stealth detection check: identifier, description, and JavaScript
/// to evaluate via CDP `Runtime.evaluate`.
pub struct DetectionCheck {
    /// Stable identifier.
    pub id: CheckId,
    /// Human-readable description of what this check tests.
    pub description: &'static str,
    /// Self-contained JavaScript expression that **must** evaluate to a JSON
    /// string with shape `'{"passed":true|false,"details":"..."}'`.
    ///
    /// The expression is sent verbatim to CDP `Runtime.evaluate`.  Use IIFEs
    /// (`(function(){ ... })()`) for multi-statement scripts.
    pub script: &'static str,
}

/// A runtime probe for a visible browser surface we do not yet fully cover.
pub struct LimitationProbe {
    /// Stable identifier.
    pub id: LimitationId,
    /// Human-readable description.
    pub description: &'static str,
    /// JavaScript expression returning a JSON string with shape
    /// `'{"limited":true|false,"details":"..."}'`.
    pub script: &'static str,
}

impl DetectionCheck {
    /// Parse the JSON string returned by the CDP evaluation of [`script`](Self::script).
    ///
    /// If the JSON is invalid (e.g. the script threw an exception), returns a
    /// failing [`CheckResult`] with the raw output in `details` (conservative
    /// fallback — avoids silently hiding problems).
    pub fn parse_output(&self, json: &str) -> CheckResult {
        #[derive(Deserialize)]
        struct Output {
            passed: bool,
            #[serde(default)]
            details: String,
        }

        match serde_json::from_str::<Output>(json) {
            Ok(o) => CheckResult {
                id: self.id,
                description: self.description.to_string(),
                passed: o.passed,
                details: o.details,
            },
            Err(e) => CheckResult {
                id: self.id,
                description: self.description.to_string(),
                passed: false,
                details: format!("parse error: {e} | raw: {json}"),
            },
        }
    }
}

impl LimitationProbe {
    fn limitation(&self, details: String) -> KnownLimitation {
        KnownLimitation {
            id: self.id,
            description: self.description.to_string(),
            details,
        }
    }

    /// Parse the JSON string returned by the CDP evaluation of [`script`](Self::script).
    pub fn parse_output(&self, json: &str) -> Option<KnownLimitation> {
        #[derive(Deserialize)]
        struct Output {
            limited: bool,
            #[serde(default)]
            details: String,
        }

        match serde_json::from_str::<Output>(json) {
            Ok(output) => output.limited.then(|| self.limitation(output.details)),
            Err(error) => Some(self.limitation(format!("parse error: {error} | raw: {json}"))),
        }
    }
}

// ── Built-in JavaScript scripts ───────────────────────────────────────────────

const SCRIPT_WEBDRIVER: &str = concat!(
    "JSON.stringify({",
    "passed:navigator.webdriver===false||navigator.webdriver===undefined,",
    "details:String(navigator.webdriver)",
    "})"
);

const SCRIPT_CHROME_OBJECT: &str = concat!(
    "JSON.stringify({",
    "passed:typeof window.chrome!=='undefined'&&window.chrome!==null",
    "&&typeof window.chrome.runtime!=='undefined',",
    "details:typeof window.chrome",
    "})"
);

const SCRIPT_PLUGIN_COUNT: &str = concat!(
    "JSON.stringify({",
    "passed:navigator.plugins.length>0,",
    "details:navigator.plugins.length+' plugins'",
    "})"
);

const SCRIPT_LANGUAGES: &str = concat!(
    "JSON.stringify({",
    "passed:Array.isArray(navigator.languages)&&navigator.languages.length>0,",
    "details:JSON.stringify(navigator.languages)",
    "})"
);

const SCRIPT_CANVAS: &str = concat!(
    "(function(){",
    "var c=document.createElement('canvas');",
    "c.width=200;c.height=50;",
    "var ctx=c.getContext('2d');",
    "ctx.fillStyle='#1a2b3c';ctx.fillRect(0,0,200,50);",
    "ctx.font='16px Arial';ctx.fillStyle='#fafafa';",
    "ctx.fillText('stygian-diag',10,30);",
    "var d=c.toDataURL();",
    "return JSON.stringify({passed:d.length>200,details:'len='+d.length});",
    "})()"
);

const SCRIPT_WEBGL_VENDOR: &str = concat!(
    "(function(){",
    "var gl=document.createElement('canvas').getContext('webgl');",
    "if(!gl)return JSON.stringify({passed:false,details:'webgl unavailable'});",
    "var ext=gl.getExtension('WEBGL_debug_renderer_info');",
    "if(!ext)return JSON.stringify({passed:true,details:'debug ext absent (normal)'});",
    "var v=gl.getParameter(ext.UNMASKED_VENDOR_WEBGL)||'';",
    "var r=gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)||'';",
    "var sw=v.includes('SwiftShader')||r.includes('SwiftShader');",
    "return JSON.stringify({passed:!sw,details:v+'/'+r});",
    "})()"
);

const SCRIPT_AUTOMATION_GLOBALS: &str = concat!(
    "JSON.stringify({",
    "passed:typeof window.__puppeteer__==='undefined'",
    "&&typeof window.__playwright==='undefined'",
    "&&typeof window.__webdriverFunc==='undefined'",
    "&&typeof window._phantom==='undefined',",
    "details:'automation globals checked'",
    "})"
);

const SCRIPT_OUTER_WINDOW: &str = concat!(
    "JSON.stringify({",
    "passed:window.outerWidth>0&&window.outerHeight>0,",
    "details:window.outerWidth+'x'+window.outerHeight",
    "})"
);

const SCRIPT_HEADLESS_UA: &str = concat!(
    "JSON.stringify({",
    "passed:!navigator.userAgent.includes('HeadlessChrome'),",
    "details:navigator.userAgent.substring(0,100)",
    "})"
);

const SCRIPT_NOTIFICATION: &str = concat!(
    "JSON.stringify({",
    "passed:typeof Notification==='undefined'||Notification.permission!=='granted',",
    "details:typeof Notification!=='undefined'?Notification.permission:'unavailable'",
    "})"
);

const SCRIPT_MATCH_MEDIA: &str = concat!(
    "JSON.stringify({",
    "passed:typeof window.matchMedia==='function',",
    "details:typeof window.matchMedia",
    "})"
);

const SCRIPT_ELEMENT_FROM_POINT: &str = concat!(
    "JSON.stringify({",
    "passed:typeof document.elementFromPoint==='function',",
    "details:typeof document.elementFromPoint",
    "})"
);

const SCRIPT_RAF: &str = concat!(
    "JSON.stringify({",
    "passed:typeof window.requestAnimationFrame==='function',",
    "details:typeof window.requestAnimationFrame",
    "})"
);

const SCRIPT_GET_COMPUTED_STYLE: &str = concat!(
    "JSON.stringify({",
    "passed:typeof window.getComputedStyle==='function',",
    "details:typeof window.getComputedStyle",
    "})"
);

const SCRIPT_CSS_SUPPORTS: &str = concat!(
    "JSON.stringify({",
    "passed:typeof CSS!=='undefined'&&typeof CSS.supports==='function',",
    "details:typeof CSS!=='undefined'?typeof CSS.supports:'undefined'",
    "})"
);

const SCRIPT_SEND_BEACON: &str = concat!(
    "JSON.stringify({",
    "passed:typeof navigator.sendBeacon==='function',",
    "details:typeof navigator.sendBeacon",
    "})"
);

const SCRIPT_EXEC_COMMAND: &str = concat!(
    "JSON.stringify({",
    "passed:typeof document.execCommand==='function',",
    "details:typeof document.execCommand",
    "})"
);

const SCRIPT_NODEJS_ABSENT: &str = concat!(
    "JSON.stringify({",
    "passed:typeof process==='undefined'",
    "||process.versions==null",
    "||typeof process.versions.node==='undefined',",
    "details:typeof process",
    "})"
);

const SCRIPT_WEBDRIVER_DESCRIPTOR: &str = concat!(
    "(function(){",
    "var d=Object.getOwnPropertyDescriptor(Navigator.prototype,'webdriver');",
    "var ok=typeof d==='undefined'||(typeof d.get==='function'&&d.set===undefined&&d.configurable===true);",
    "var detail=d?('getter='+typeof d.get+',set='+typeof d.set+',configurable='+String(d.configurable)+',enumerable='+String(d.enumerable)):'missing';",
    "return JSON.stringify({passed:ok,details:detail});",
    "})()"
);

const SCRIPT_USER_AGENT_DATA: &str = concat!(
    "(function(){",
    "var d=navigator.userAgentData;",
    "var ok=typeof d==='undefined'||(Array.isArray(d.brands)&&d.brands.length>0&&typeof d.mobile==='boolean'&&typeof d.getHighEntropyValues==='function');",
    "var detail=typeof d==='undefined'?'undefined':('brands='+(Array.isArray(d.brands)?d.brands.length:0)+',mobile='+String(d.mobile)+',platform='+(d.platform||''));",
    "return JSON.stringify({passed:ok,details:detail});",
    "})()"
);

const SCRIPT_CONNECTION: &str = concat!(
    "(function(){",
    "var c=navigator.connection;",
    "var ok=typeof c!=='undefined'&&typeof c.rtt==='number'&&c.rtt>=0&&typeof c.downlink==='number'&&c.downlink>=0&&typeof c.effectiveType==='string'&&c.effectiveType.length>0;",
    "var detail=typeof c==='undefined'?'undefined':('rtt='+String(c.rtt)+',downlink='+String(c.downlink)+',effectiveType='+(c.effectiveType||''));",
    "return JSON.stringify({passed:ok,details:detail});",
    "})()"
);

const SCRIPT_STORAGE_ESTIMATE: &str = concat!(
    "(function(){",
    "var s=navigator.storage;",
    "var limited=!s||typeof s.estimate!=='function';",
    "var detail=!s?'storage unavailable':typeof s.estimate;",
    "return JSON.stringify({limited:limited,details:detail});",
    "})()"
);

const SCRIPT_HIDDEN_FONT_PROBE: &str = concat!(
    "(function(){",
    "var root=document.body||document.documentElement;",
    "if(!root){return JSON.stringify({passed:false,details:'no root element available'});}",
    "var probe=document.createElement('div');",
    "probe.textContent='mmmmmmmmmlli';",
    "probe.setAttribute('aria-hidden','true');",
    "probe.style.position='absolute';",
    "probe.style.visibility='hidden';",
    "probe.style.font='16px Arial';",
    "root.appendChild(probe);",
    "var rect=probe.getBoundingClientRect();",
    "probe.remove();",
    "var ok=rect.width>0&&rect.height>0;",
    "return JSON.stringify({passed:ok,details:'width='+rect.width+',height='+rect.height});",
    "})()"
);

const SCRIPT_SCREEN_METRICS: &str = concat!(
    "JSON.stringify({",
    "passed:screen.width>0&&screen.height>0&&screen.availWidth>0&&screen.availHeight>0&&screen.availWidth<=screen.width&&screen.availHeight<=screen.height&&window.devicePixelRatio>0,",
    "details:'screen='+screen.width+'x'+screen.height+',avail='+screen.availWidth+'x'+screen.availHeight+',dpr='+window.devicePixelRatio",
    "})"
);

const SCRIPT_AUDIO_CONTEXT: &str = concat!(
    "(function(){",
    "var C=window.AudioContext||window.webkitAudioContext;",
    "if(!C)return JSON.stringify({passed:false,details:'AudioContext unavailable'});",
    "var ctx=new C();",
    "var sampleRate=ctx.sampleRate||0;",
    "var baseLatency=typeof ctx.baseLatency==='number'?ctx.baseLatency:-1;",
    "if(typeof ctx.close==='function'){ctx.close();}",
    "return JSON.stringify({passed:sampleRate>0,details:'sampleRate='+sampleRate+',baseLatency='+baseLatency});",
    "})()"
);

const SCRIPT_WEBGPU_LIMITATION: &str = concat!(
    "JSON.stringify({",
    "limited:'gpu' in navigator,",
    "details:typeof navigator.gpu",
    "})"
);

const SCRIPT_PERFORMANCE_MEMORY_LIMITATION: &str = concat!(
    "JSON.stringify({",
    "limited:typeof performance.memory!=='undefined',",
    "details:typeof performance.memory",
    "})"
);

// ── Static check catalogue ────────────────────────────────────────────────────

/// Return all built-in stealth detection checks.
///
/// Iterate the slice, send each `check.script` to the browser via CDP, then
/// call [`DetectionCheck::parse_output`] with the returned JSON string.
pub fn all_checks() -> &'static [DetectionCheck] {
    CHECKS
}

/// Return all known browser-surface limitation probes.
pub fn all_limitation_probes() -> &'static [LimitationProbe] {
    LIMITATION_PROBES
}

static CHECKS: &[DetectionCheck] = &[
    DetectionCheck {
        id: CheckId::WebDriverFlag,
        description: "navigator.webdriver must be false/undefined",
        script: SCRIPT_WEBDRIVER,
    },
    DetectionCheck {
        id: CheckId::ChromeObject,
        description: "window.chrome.runtime must exist",
        script: SCRIPT_CHROME_OBJECT,
    },
    DetectionCheck {
        id: CheckId::PluginCount,
        description: "navigator.plugins must be non-empty",
        script: SCRIPT_PLUGIN_COUNT,
    },
    DetectionCheck {
        id: CheckId::LanguagesPresent,
        description: "navigator.languages must be non-empty",
        script: SCRIPT_LANGUAGES,
    },
    DetectionCheck {
        id: CheckId::CanvasConsistency,
        description: "canvas toDataURL must return non-trivial image data",
        script: SCRIPT_CANVAS,
    },
    DetectionCheck {
        id: CheckId::WebGlVendor,
        description: "WebGL vendor must not be SwiftShader (software renderer)",
        script: SCRIPT_WEBGL_VENDOR,
    },
    DetectionCheck {
        id: CheckId::AutomationGlobals,
        description: "automation globals (Puppeteer/Playwright) must be absent",
        script: SCRIPT_AUTOMATION_GLOBALS,
    },
    DetectionCheck {
        id: CheckId::OuterWindowSize,
        description: "window.outerWidth/outerHeight must be non-zero",
        script: SCRIPT_OUTER_WINDOW,
    },
    DetectionCheck {
        id: CheckId::HeadlessUserAgent,
        description: "User-Agent must not contain 'HeadlessChrome'",
        script: SCRIPT_HEADLESS_UA,
    },
    DetectionCheck {
        id: CheckId::NotificationPermission,
        description: "Notification.permission must not be pre-granted",
        script: SCRIPT_NOTIFICATION,
    },
    DetectionCheck {
        id: CheckId::MatchMediaPresent,
        description: "window.matchMedia must be a function (PX env-bitmask bit 0)",
        script: SCRIPT_MATCH_MEDIA,
    },
    DetectionCheck {
        id: CheckId::ElementFromPointPresent,
        description: "document.elementFromPoint must be a function (PX env-bitmask bit 1)",
        script: SCRIPT_ELEMENT_FROM_POINT,
    },
    DetectionCheck {
        id: CheckId::RequestAnimationFramePresent,
        description: "window.requestAnimationFrame must be a function (PX env-bitmask bit 2)",
        script: SCRIPT_RAF,
    },
    DetectionCheck {
        id: CheckId::GetComputedStylePresent,
        description: "window.getComputedStyle must be a function (PX env-bitmask bit 3)",
        script: SCRIPT_GET_COMPUTED_STYLE,
    },
    DetectionCheck {
        id: CheckId::CssSupportsPresent,
        description: "CSS.supports must exist and be callable (PX env-bitmask bit 4)",
        script: SCRIPT_CSS_SUPPORTS,
    },
    DetectionCheck {
        id: CheckId::SendBeaconPresent,
        description: "navigator.sendBeacon must be a function (PX env-bitmask bit 5)",
        script: SCRIPT_SEND_BEACON,
    },
    DetectionCheck {
        id: CheckId::ExecCommandPresent,
        description: "document.execCommand must be a function (PX env-bitmask bit 6)",
        script: SCRIPT_EXEC_COMMAND,
    },
    DetectionCheck {
        id: CheckId::NodeJsAbsent,
        description: "process.versions.node must be absent — not a Node.js environment (PX env-bitmask bit 7)",
        script: SCRIPT_NODEJS_ABSENT,
    },
    DetectionCheck {
        id: CheckId::WebDriverDescriptorShape,
        description: "Navigator.prototype.webdriver must look like an accessor descriptor",
        script: SCRIPT_WEBDRIVER_DESCRIPTOR,
    },
    DetectionCheck {
        id: CheckId::UserAgentDataPresent,
        description: "navigator.userAgentData must expose coherent client hints",
        script: SCRIPT_USER_AGENT_DATA,
    },
    DetectionCheck {
        id: CheckId::ConnectionPresent,
        description: "navigator.connection must expose plausible network information",
        script: SCRIPT_CONNECTION,
    },
    DetectionCheck {
        id: CheckId::HiddenFontProbeRect,
        description: "hidden font probes must yield non-zero layout measurements",
        script: SCRIPT_HIDDEN_FONT_PROBE,
    },
    DetectionCheck {
        id: CheckId::ScreenMetricsCoherent,
        description: "screen metrics and devicePixelRatio must be coherent",
        script: SCRIPT_SCREEN_METRICS,
    },
    DetectionCheck {
        id: CheckId::AudioContextPresent,
        description: "AudioContext must expose a non-zero sample rate",
        script: SCRIPT_AUDIO_CONTEXT,
    },
];

static LIMITATION_PROBES: &[LimitationProbe] = &[
    LimitationProbe {
        id: LimitationId::WebGpuSurface,
        description: "navigator.gpu / WebGPU is exposed but not yet spoofed or validated",
        script: SCRIPT_WEBGPU_LIMITATION,
    },
    LimitationProbe {
        id: LimitationId::PerformanceMemorySurface,
        description: "performance.memory is exposed but not yet spoofed or validated",
        script: SCRIPT_PERFORMANCE_MEMORY_LIMITATION,
    },
    LimitationProbe {
        id: LimitationId::OpaqueOriginStorage,
        description: "navigator.storage is unavailable or incomplete on this origin",
        script: SCRIPT_STORAGE_ESTIMATE,
    },
];

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_checks_returns_eighteen_entries() {
        assert_eq!(all_checks().len(), 24);
    }

    #[test]
    fn all_limitation_probes_returns_two_entries() {
        assert_eq!(all_limitation_probes().len(), 3);
    }

    #[test]
    fn all_checks_have_unique_ids() {
        let ids: HashSet<_> = all_checks().iter().map(|c| c.id).collect();
        assert_eq!(
            ids.len(),
            all_checks().len(),
            "duplicate check ids detected"
        );
    }

    #[test]
    fn all_checks_have_non_empty_scripts_with_json_stringify() {
        for check in all_checks() {
            assert!(
                !check.script.is_empty(),
                "check {:?} has empty script",
                check.id
            );
            assert!(
                check.script.contains("JSON.stringify"),
                "check {:?} script must produce a JSON string",
                check.id
            );
        }
    }

    #[test]
    fn parse_output_valid_passing_json() {
        let check = &all_checks()[0]; // WebDriverFlag
        let result = check.parse_output(r#"{"passed":true,"details":"undefined"}"#);
        assert!(result.passed);
        assert_eq!(result.id, CheckId::WebDriverFlag);
        assert_eq!(result.details, "undefined");
    }

    #[test]
    fn parse_output_valid_failing_json() {
        let check = &all_checks()[0];
        let result = check.parse_output(r#"{"passed":false,"details":"true"}"#);
        assert!(!result.passed);
    }

    #[test]
    fn parse_output_invalid_json_returns_fail_with_details() {
        let check = &all_checks()[0];
        let result = check.parse_output("not json at all");
        assert!(!result.passed);
        assert!(result.details.contains("parse error"));
    }

    #[test]
    fn parse_output_preserves_check_id() {
        let check = all_checks()
            .iter()
            .find(|c| c.id == CheckId::ChromeObject)
            .unwrap();
        let result = check.parse_output(r#"{"passed":true,"details":"object"}"#);
        assert_eq!(result.id, CheckId::ChromeObject);
        assert_eq!(result.description, check.description);
    }

    #[test]
    fn parse_output_missing_details_defaults_to_empty() {
        let check = &all_checks()[0];
        let result = check.parse_output(r#"{"passed":true}"#);
        assert!(result.passed);
        assert!(result.details.is_empty());
    }

    #[test]
    fn diagnostic_report_all_passing() {
        let results: Vec<CheckResult> = all_checks()
            .iter()
            .map(|c| c.parse_output(r#"{"passed":true,"details":"ok"}"#))
            .collect();
        let report = DiagnosticReport::new(results);
        assert!(report.is_clean());
        assert_eq!(report.passed_count, 24);
        assert!(report.known_limitations.is_empty());
        assert_eq!(report.failed_count, 0);
        assert!((report.coverage_pct() - 100.0).abs() < 0.001);
        assert_eq!(report.failures().count(), 0);
    }

    #[test]
    fn diagnostic_report_some_failing() {
        let mut results: Vec<CheckResult> = all_checks()
            .iter()
            .map(|c| c.parse_output(r#"{"passed":true,"details":"ok"}"#))
            .collect();
        results[0].passed = false;
        results[2].passed = false;
        let report = DiagnosticReport::new(results);
        assert!(!report.is_clean());
        assert_eq!(report.failed_count, 2);
        assert_eq!(report.passed_count, 22);
        assert_eq!(report.failures().count(), 2);
    }

    #[test]
    fn diagnostic_report_empty_checks() {
        let report = DiagnosticReport::new(Vec::new());
        assert!(report.is_clean()); // vacuously true
        assert!((report.coverage_pct()).abs() < 0.001);
    }

    #[test]
    fn check_result_serializes_with_snake_case_id() {
        let result = CheckResult {
            id: CheckId::WebDriverFlag,
            description: "test".to_string(),
            passed: true,
            details: "ok".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"web_driver_flag\""), "got: {json}");
        assert!(json.contains("\"passed\":true"));
    }

    #[test]
    fn diagnostic_report_serializes_and_deserializes() {
        let results: Vec<CheckResult> = all_checks()
            .iter()
            .map(|c| c.parse_output(r#"{"passed":true,"details":"ok"}"#))
            .collect();
        let report = DiagnosticReport::new(results).with_known_limitations(vec![KnownLimitation {
            id: LimitationId::WebGpuSurface,
            description: "navigator.gpu / WebGPU is exposed but not yet spoofed or validated"
                .to_string(),
            details: "object".to_string(),
        }]);
        let json = serde_json::to_string(&report).unwrap();
        let restored: DiagnosticReport = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.passed_count, report.passed_count);
        assert_eq!(restored.known_limitations.len(), 1);
        assert!(restored.is_clean());
    }

    #[test]
    fn limitation_probe_reports_surface_when_limited() {
        let probe = &all_limitation_probes()[0];
        let limitation = probe
            .parse_output(r#"{"limited":true,"details":"object"}"#)
            .unwrap();
        assert_eq!(limitation.id, LimitationId::WebGpuSurface);
        assert_eq!(limitation.details, "object");
    }

    #[test]
    fn limitation_probe_returns_none_when_surface_not_limited() {
        let probe = &all_limitation_probes()[0];
        assert!(
            probe
                .parse_output(r#"{"limited":false,"details":"undefined"}"#)
                .is_none()
        );
    }

    #[test]
    fn transport_diagnostic_reports_match_for_matching_observations() {
        let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
        let expected = TransportDiagnostic::from_user_agent_and_observations(user_agent, None);

        // Ensure test UA resolves at least one expected fingerprint.
        assert!(
            expected.expected_profile.is_some()
                || expected.expected_ja3_hash.is_some()
                || expected.expected_ja4.is_some()
                || expected.expected_http3_perk_text.is_some()
        );

        let observed = TransportObservations {
            ja3_hash: expected.expected_ja3_hash.clone(),
            ja4: expected.expected_ja4.clone(),
            http3_perk_text: expected.expected_http3_perk_text.clone(),
            http3_perk_hash: expected.expected_http3_perk_hash,
        };
        let diagnostic =
            TransportDiagnostic::from_user_agent_and_observations(user_agent, Some(&observed));

        assert_eq!(diagnostic.transport_match, Some(true));
        assert!(diagnostic.mismatches.is_empty());
    }

    #[test]
    fn transport_diagnostic_reports_mismatch_for_mismatching_observations() {
        let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
        let expected = TransportDiagnostic::from_user_agent_and_observations(user_agent, None);

        assert!(expected.expected_ja3_hash.is_some());

        let observed = TransportObservations {
            ja3_hash: Some("definitely-not-the-expected-ja3".to_string()),
            ja4: expected.expected_ja4.clone(),
            http3_perk_text: expected.expected_http3_perk_text.clone(),
            http3_perk_hash: expected.expected_http3_perk_hash,
        };
        let diagnostic =
            TransportDiagnostic::from_user_agent_and_observations(user_agent, Some(&observed));

        assert_eq!(diagnostic.transport_match, Some(false));
        assert!(!diagnostic.mismatches.is_empty());
        assert!(
            diagnostic
                .mismatches
                .iter()
                .any(|m| m.contains("ja3_hash mismatch"))
        );
    }

    #[test]
    fn transport_diagnostic_flags_observations_when_no_expectations_derivable() {
        let user_agent = "UnknownBrowser/0.0";
        let diagnostic_without_observed =
            TransportDiagnostic::from_user_agent_and_observations(user_agent, None);

        // Unknown UA should not resolve any expectations.
        assert_eq!(diagnostic_without_observed.expected_profile, None);
        assert_eq!(diagnostic_without_observed.expected_ja3_hash, None);
        assert_eq!(diagnostic_without_observed.expected_ja4, None);
        assert_eq!(diagnostic_without_observed.expected_http3_perk_text, None);

        let observed = TransportObservations {
            ja3_hash: Some("some-observed-ja3".to_string()),
            ja4: Some("some-observed-ja4".to_string()),
            http3_perk_text: Some("some-observed-http3-perk-text".to_string()),
            http3_perk_hash: Some("some-observed-http3-perk-hash".to_string()),
        };

        let diagnostic =
            TransportDiagnostic::from_user_agent_and_observations(user_agent, Some(&observed));

        // With observations but no expectations, mismatches should be flagged.
        assert_eq!(diagnostic.transport_match, Some(false));
        assert!(!diagnostic.mismatches.is_empty());
        assert!(
            diagnostic
                .mismatches
                .iter()
                .any(|m| m.contains("no expected JA3 could be derived"))
        );
    }
}
