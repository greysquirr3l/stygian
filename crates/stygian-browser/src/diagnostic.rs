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
        }
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
    "details:typeof CSS",
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
    "||typeof process.versions==='undefined'",
    "||typeof process.versions.node==='undefined',",
    "details:typeof process",
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
];

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_checks_returns_ten_entries() {
        assert_eq!(all_checks().len(), 18);
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
        assert_eq!(report.passed_count, 18);
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
        assert_eq!(report.passed_count, 16);
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
        let report = DiagnosticReport::new(results);
        let json = serde_json::to_string(&report).unwrap();
        let restored: DiagnosticReport = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.passed_count, report.passed_count);
        assert!(restored.is_clean());
    }
}
