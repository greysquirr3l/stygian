//! Advanced CDP leak hardening.
//!
//! Injects a script that runs **before** all other stealth scripts to:
//!
//! 1. Delete Playwright / Puppeteer binding remnants from `window`.
//! 2. Sanitize `Error.prototype.stack` to remove CDP-specific frame URLs.
//! 3. Harden `console.debug` so a getter-based stack-inspection trap cannot
//!    detect CDP from within it.
//! 4. Ensure the `Navigator.prototype.webdriver` property descriptor matches
//!    Chrome's native format (accessor descriptor with a getter, no setter).
//! 5. Mark as non-enumerable any artifact properties that CDP injection may
//!    have left enumerable on `window`.
//!
//! # Example
//!
//! ```
//! use stygian_browser::cdp_hardening::{cdp_hardening_script, CdpHardeningConfig};
//!
//! let cfg = CdpHardeningConfig::default();
//! let js = cdp_hardening_script(&cfg);
//! assert!(js.contains("__playwright"));
//! assert!(js.contains("Error.prototype"));
//! ```

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for CDP leak hardening.
///
/// # Example
///
/// ```
/// use stygian_browser::cdp_hardening::CdpHardeningConfig;
///
/// let cfg = CdpHardeningConfig::default();
/// assert!(cfg.enabled);
/// assert!(cfg.sanitize_stacks);
/// assert!(cfg.protect_console);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpHardeningConfig {
    /// Master switch. When `false`, [`cdp_hardening_script`] returns an empty string.
    pub enabled: bool,
    /// Whether to sanitize CDP-specific frames from `Error.prototype.stack`.
    pub sanitize_stacks: bool,
    /// Whether to override `console.debug` to defeat getter-trap-based stack inspection.
    pub protect_console: bool,
}

impl Default for CdpHardeningConfig {
    /// All protections enabled.
    fn default() -> Self {
        Self {
            enabled: true,
            sanitize_stacks: true,
            protect_console: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Script generator
// ---------------------------------------------------------------------------

/// Generate the CDP hardening injection script.
///
/// Returns an empty string when `config.enabled` is `false`.
///
/// # Example
///
/// ```
/// use stygian_browser::cdp_hardening::{cdp_hardening_script, CdpHardeningConfig};
///
/// let js = cdp_hardening_script(&CdpHardeningConfig { enabled: false, ..Default::default() });
/// assert!(js.is_empty());
///
/// let js2 = cdp_hardening_script(&CdpHardeningConfig::default());
/// assert!(js2.contains("__playwright__binding__"));
/// ```
#[must_use]
pub fn cdp_hardening_script(config: &CdpHardeningConfig) -> String {
    if !config.enabled {
        return String::new();
    }

    let stack_section = if config.sanitize_stacks {
        ERROR_STACK_SECTION
    } else {
        ""
    };

    let console_section = if config.protect_console {
        CONSOLE_DEBUG_SECTION
    } else {
        ""
    };

    format!(
        r#"(function() {{
  'use strict';

  // ── 1. Delete Playwright / Puppeteer binding remnants ─────────────────
  var _cdpArtifacts = [
    '__playwright__binding__',
    '__pwInitScripts',
    '__playwright_evaluation_script__',
    '__puppeteer_evaluation_script__',
    '__puppeteer__binding__',
    '__playwright_clock__',
    '__pw_manual_fulfill__',
    '__pw_dispatch_event__',
    '__pwpEventListeners',
  ];
  _cdpArtifacts.forEach(function(key) {{
    try {{ delete window[key]; }} catch(e) {{}}
    try {{
      if (key in window) {{
        Object.defineProperty(window, key, {{
          value: undefined, writable: false, configurable: false, enumerable: false
        }});
      }}
    }} catch(e) {{}}
  }});

{stack_section}
{console_section}

  // ── 4. Navigator.prototype.webdriver — native-looking accessor descriptor ──
  try {{
    // Chrome's native descriptor: {{ get: f, set: undefined, enumerable: true, configurable: true }}
    var _wdDesc = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver');
    if (_wdDesc) {{
      // If currently a data descriptor or has a set, redefine as accessor-only
      if (!_wdDesc.get || _wdDesc.set !== undefined) {{
        var _wdGetter = function webdriver() {{ return false; }};
        _wdGetter.toString = function toString() {{
          return 'function webdriver() {{ [native code] }}';
        }};
        Object.defineProperty(Navigator.prototype, 'webdriver', {{
          get: _wdGetter,
          set: undefined,
          enumerable: true,
          configurable: true,
        }});
      }} else {{
        // Patch the existing getter to return false
        var _existingGetter = _wdDesc.get;
        // Only override if current getter would reveal webdriver=true
        void _existingGetter; // referenced intentionally
        var _falseGetter = function webdriver() {{ return false; }};
        _falseGetter.toString = function toString() {{
          return 'function webdriver() {{ [native code] }}';
        }};
        Object.defineProperty(Navigator.prototype, 'webdriver', {{
          get: _falseGetter,
          set: undefined,
          enumerable: true,
          configurable: true,
        }});
      }}
    }}
  }} catch(e) {{}}

  // ── 5. Enumeration protection — mark CDP artifacts as non-enumerable ──
  var _nonEnumProps = [
    'cdc_adoQpoasnfa76pfcZLmcfl_Array',
    'cdc_adoQpoasnfa76pfcZLmcfl_Promise',
    'cdc_adoQpoasnfa76pfcZLmcfl_Symbol',
    '__cdc_asdjflasutopfhvcZLmcfl_',
    '__selenium_evaluate',
    '__selenium_unwrapped',
    '__webdriverFunc',
    '__webdriver_evaluate',
    '__driver_evaluate',
    '__driver_unwrapped',
    '__lastWatirAlert',
    '__lastWatirConfirm',
    '__lastWatirPrompt',
  ];
  _nonEnumProps.forEach(function(key) {{
    try {{
      if (key in window) {{
        var _desc = Object.getOwnPropertyDescriptor(window, key);
        if (_desc && _desc.enumerable) {{
          Object.defineProperty(window, key, {{
            value: _desc.value,
            writable: _desc.writable || false,
            configurable: _desc.configurable || false,
            enumerable: false,
          }});
        }}
      }}
    }} catch(e) {{}}
  }});

}})();
"#,
        stack_section = stack_section,
        console_section = console_section,
    )
}

// ── Static script sections ────────────────────────────────────────────────

/// Error.prototype.stack sanitization section.
const ERROR_STACK_SECTION: &str = r#"  // ── 2. Sanitize Error.prototype.stack ───────────────────────────────
  try {
    var _origStackDesc = Object.getOwnPropertyDescriptor(Error.prototype, 'stack');
    if (_origStackDesc && _origStackDesc.get) {
      var _origStackGetter = _origStackDesc.get;
      var _cdpFrameRe = /(https?:\/\/[^\s]*(?:__puppeteer|__playwright|pptr:|puppeteer-eval|playwright-eval)[^\s]*|chrome-extension:\/\/[^\s]*)/g;
      var _sanitizedGetter = function stack() {
        var s = _origStackGetter.call(this);
        if (typeof s !== 'string') { return s; }
        return s.replace(_cdpFrameRe, 'https://example.com/app.js');
      };
      _sanitizedGetter.toString = function toString() {
        return 'function get stack() { [native code] }';
      };
      Object.defineProperty(Error.prototype, 'stack', {
        get: _sanitizedGetter,
        set: _origStackDesc.set,
        enumerable: _origStackDesc.enumerable,
        configurable: _origStackDesc.configurable,
      });
    } else if (_origStackDesc && 'value' in _origStackDesc) {
      // Data-descriptor path: wrap with a getter going forward
      // Nothing to patch here at definition time; stack is per-instance
    }
  } catch(e) {}"#;

/// console.debug protection section.
const CONSOLE_DEBUG_SECTION: &str = r#"  // ── 3. console.debug getter-trap hardening ────────────────────────────
  try {
    var _origDebug = console.debug.bind(console);
    var _safeDebug = function debug() {
      return _origDebug.apply(console, arguments);
    };
    _safeDebug.toString = function toString() {
      return 'function debug() { [native code] }';
    };
    try {
      console.debug = _safeDebug;
    } catch(e) {
      Object.defineProperty(console, 'debug', {
        value: _safeDebug, writable: true, configurable: true, enumerable: false
      });
    }
  } catch(e) {}"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_script() -> String {
        cdp_hardening_script(&CdpHardeningConfig::default())
    }

    #[test]
    fn disabled_returns_empty() {
        let js = cdp_hardening_script(&CdpHardeningConfig {
            enabled: false,
            ..Default::default()
        });
        assert!(js.is_empty());
    }

    #[test]
    fn script_deletes_playwright_artifacts() {
        let js = default_script();
        assert!(
            js.contains("__playwright__binding__"),
            "missing playwright binding"
        );
        assert!(js.contains("__pwInitScripts"), "missing pwInitScripts");
        assert!(
            js.contains("__playwright_evaluation_script__"),
            "missing playwright eval script"
        );
        assert!(
            js.contains("__puppeteer_evaluation_script__"),
            "missing puppeteer eval script"
        );
    }

    #[test]
    fn error_stack_sanitizer_regex_present() {
        let js = default_script();
        assert!(js.contains("__puppeteer"), "missing puppeteer pattern");
        assert!(js.contains("__playwright"), "missing playwright pattern");
        assert!(js.contains("pptr:"), "missing pptr: pattern");
        assert!(
            js.contains("chrome-extension://"),
            "missing chrome-extension pattern"
        );
    }

    #[test]
    fn console_debug_has_native_tostring_spoof() {
        let js = default_script();
        assert!(
            js.contains("function debug() { [native code] }"),
            "missing native toString spoof for console.debug"
        );
    }

    #[test]
    fn webdriver_descriptor_matches_chrome_native() {
        let js = default_script();
        // Should define as accessor-only (no set)
        assert!(
            js.contains("set: undefined"),
            "missing set: undefined for webdriver"
        );
        assert!(
            js.contains("enumerable: true"),
            "webdriver must be enumerable"
        );
        assert!(
            js.contains("configurable: true"),
            "webdriver must be configurable"
        );
    }

    #[test]
    fn no_new_enumerable_window_properties() {
        let js = default_script();
        // All artifact keys in section 5 should be marked non-enumerable
        assert!(
            js.contains("enumerable: false"),
            "artifacts must be set non-enumerable"
        );
    }

    #[test]
    fn sanitize_stacks_false_omits_error_section() {
        let js = cdp_hardening_script(&CdpHardeningConfig {
            enabled: true,
            sanitize_stacks: false,
            protect_console: true,
        });
        assert!(
            !js.contains("Error.prototype"),
            "error section should be absent"
        );
        // console section should still be present
        assert!(js.contains("console.debug"));
    }

    #[test]
    fn protect_console_false_omits_console_section() {
        let js = cdp_hardening_script(&CdpHardeningConfig {
            enabled: true,
            sanitize_stacks: true,
            protect_console: false,
        });
        // No console.debug override
        assert!(
            !js.contains("_safeDebug"),
            "console section should be absent"
        );
        // Error stack section still present
        assert!(js.contains("Error.prototype"));
    }
}
