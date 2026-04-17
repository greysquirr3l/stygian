//! Comprehensive navigator property coherence injection.
//!
//! Overrides all navigator properties that anti-bot systems cross-reference to
//! ensure they are consistent with a single [`FingerprintProfile`]. Covers:
//! `hardwareConcurrency`, `deviceMemory`, `connection` (`NetworkInformation API`),
//! `maxTouchPoints`, `languages`, `pdfViewerEnabled`, `plugins`, `mimeTypes`,
//! and `userAgentData` Client Hints.
//!
//! # Example
//!
//! ```
//! use stygian_browser::navigator_coherence::navigator_coherence_script;
//! use stygian_browser::profile::FingerprintProfile;
//!
//! let p = FingerprintProfile::windows_chrome_136_rtx3060();
//! let js = navigator_coherence_script(&p);
//! assert!(js.contains("hardwareConcurrency"));
//! assert!(js.contains("deviceMemory"));
//! assert!(js.contains("userAgentData"));
//! ```

use crate::profile::{BrowserKind, FingerprintProfile};

/// Generate a CDP injection script for comprehensive navigator coherence.
///
/// All overrides are derived from `profile` to guarantee internal consistency.
/// Must be injected via `Page.addScriptToEvaluateOnNewDocument`.
///
/// When a profile is not set, call `stealth.rs`'s existing navigator spoof
/// instead — this function requires a fully populated profile.
///
/// # Example
///
/// ```
/// use stygian_browser::navigator_coherence::navigator_coherence_script;
/// use stygian_browser::profile::FingerprintProfile;
///
/// let p = FingerprintProfile::linux_chrome_136_intel();
/// let js = navigator_coherence_script(&p);
/// assert!(js.contains("4")); // 4 cores
/// assert!(js.contains("userAgentData"));
/// ```
#[must_use]
pub fn navigator_coherence_script(profile: &FingerprintProfile) -> String {
    let cores = profile.hardware.cores;
    let memory = profile.hardware.memory_gb;
    let rtt = profile.network.rtt;
    let downlink = profile.network.downlink;
    let effective_type = &profile.network.effective_type;
    let save_data = profile.network.save_data;
    let max_touch = profile.platform.max_touch_points;
    let pdf_viewer_enabled = matches!(
        profile.browser.kind,
        BrowserKind::Chrome | BrowserKind::Edge
    );

    // Build languages array from the UA / platform
    let languages_js = build_languages_js(profile);
    let plugins_js = build_plugins_js(profile);
    let ua_data_js = build_ua_data_js(profile);
    let user_agent = &profile.browser.user_agent;
    let platform_string = &profile.platform.platform_string;

    format!(
        r"(function() {{
  'use strict';

  // ── toString spoof utility ───────────────────────────────────────────────
  function _nts(name) {{ return function toString() {{ return 'function ' + name + '() {{ [native code] }}'; }}; }}
  function _def(obj, prop, val) {{
    Object.defineProperty(obj, prop, {{ value: val, writable: false, configurable: false, enumerable: false }});
  }}
  function _defGetter(obj, prop, getter) {{
    getter.toString = _nts('get ' + prop);
    Object.defineProperty(obj, prop, {{ get: getter, configurable: false, enumerable: true }});
  }}

  // ── 1. hardwareConcurrency ───────────────────────────────────────────────
  _defGetter(Navigator.prototype, 'hardwareConcurrency', function() {{ return {cores}; }});

  // ── 2. deviceMemory ─────────────────────────────────────────────────────
  if ('deviceMemory' in navigator) {{
    _defGetter(Navigator.prototype, 'deviceMemory', function() {{ return {memory}; }});
  }}

  // ── 3. connection (NetworkInformation) ──────────────────────────────────
  const _conn = Object.create(EventTarget.prototype);
  _def(_conn, 'rtt', {rtt});
  _def(_conn, 'downlink', {downlink});
  _def(_conn, 'effectiveType', '{effective_type}');
  _def(_conn, 'saveData', {save_data_js});
  _def(_conn, 'onchange', null);
  _defGetter(Navigator.prototype, 'connection', function() {{ return _conn; }});

  // ── 4. maxTouchPoints ───────────────────────────────────────────────────
  _defGetter(Navigator.prototype, 'maxTouchPoints', function() {{ return {max_touch}; }});

  // ── 5. languages ─────────────────────────────────────────────────────────
  const _langs = {languages_js};
  _defGetter(Navigator.prototype, 'languages', function() {{ return _langs; }});
  _defGetter(Navigator.prototype, 'language', function() {{ return _langs[0] || 'en-US'; }});

  // ── 6. pdfViewerEnabled ──────────────────────────────────────────────────
  _defGetter(Navigator.prototype, 'pdfViewerEnabled', function() {{ return {pdf_viewer_enabled_js}; }});

  // ── 7. plugins + mimeTypes ───────────────────────────────────────────────
  {plugins_js}

  // ── 8. userAgent + platform ─────────────────────────────────────────────
  _defGetter(Navigator.prototype, 'userAgent', function() {{ return '{user_agent}'; }});
  _defGetter(Navigator.prototype, 'platform', function() {{ return '{platform_string}'; }});
  _defGetter(Navigator.prototype, 'appVersion', function() {{
    return '{user_agent}'.replace('Mozilla/', '');
  }});

  // ── 9. userAgentData (Client Hints) ─────────────────────────────────────
  {ua_data_js}

}})();
",
        cores = cores,
        memory = memory,
        rtt = rtt,
        downlink = downlink,
        effective_type = effective_type,
        save_data_js = if save_data { "true" } else { "false" },
        max_touch = max_touch,
        languages_js = languages_js,
        pdf_viewer_enabled_js = if pdf_viewer_enabled { "true" } else { "false" },
        plugins_js = plugins_js,
        user_agent = user_agent.replace('\'', "\\'"),
        platform_string = platform_string.replace('\'', "\\'"),
        ua_data_js = ua_data_js,
    )
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn build_languages_js(profile: &FingerprintProfile) -> String {
    // Derive language from the sec_ch_ua_platform — always en-US as primary for now.
    // Future: add locale field to BrowserProfile.
    let _ = profile.browser.sec_ch_ua_mobile == "?1";
    "['en-US', 'en']".into()
}

fn build_plugins_js(profile: &FingerprintProfile) -> String {
    let is_chrome_like = matches!(
        profile.browser.kind,
        BrowserKind::Chrome | BrowserKind::Edge
    );
    if !is_chrome_like {
        return String::new();
    }
    // Chrome 136 standard plugin list (5 entries)
    r"
  (function() {
    const _mimeTypes = [
      { type: 'application/pdf', suffixes: 'pdf', description: '' },
      { type: 'text/pdf', suffixes: 'pdf', description: '' },
    ];
    const _pluginData = [
      'PDF Viewer',
      'Chrome PDF Viewer',
      'Chromium PDF Viewer',
      'Microsoft Edge PDF Viewer',
      'WebKit built-in PDF',
    ];
    function _makePlugin(name) {
      const p = Object.create(Plugin.prototype);
      Object.defineProperty(p, 'name',        { value: name, enumerable: true });
      Object.defineProperty(p, 'description', { value: '', enumerable: true });
      Object.defineProperty(p, 'filename',    { value: 'internal-pdf-viewer', enumerable: true });
      Object.defineProperty(p, 'length',      { value: _mimeTypes.length, enumerable: true });
      _mimeTypes.forEach(function(mt, i) {
        Object.defineProperty(p, i, { value: mt, enumerable: true });
      });
      p.item = function(i) { return _mimeTypes[i] || null; };
      p.namedItem = function(n) { return _mimeTypes.find(function(m) { return m.type === n; }) || null; };
      return p;
    }
    const _plugins = _pluginData.map(_makePlugin);
    const _pluginArray = Object.create(PluginArray.prototype);
    Object.defineProperty(_pluginArray, 'length', { value: _plugins.length, enumerable: true });
    _plugins.forEach(function(p, i) { Object.defineProperty(_pluginArray, i, { value: p, enumerable: true }); });
    _pluginArray.item = function(i) { return _plugins[i] || null; };
    _pluginArray.namedItem = function(n) { return _plugins.find(function(p) { return p.name === n; }) || null; };
    _pluginArray.refresh = function() {};
    Object.defineProperty(Navigator.prototype, 'plugins', {
      get: function() { return _pluginArray; }, configurable: false, enumerable: true
    });
  })();".into()
}

fn build_ua_data_js(profile: &FingerprintProfile) -> String {
    let brands = parse_sec_ch_ua_brands(&profile.browser.sec_ch_ua);
    let mobile = profile.browser.sec_ch_ua_mobile == "?1";
    let platform = strip_quotes(&profile.browser.sec_ch_ua_platform);
    let os_version = &profile.platform.os_version;

    format!(
        r"
  if (typeof NavigatorUAData !== 'undefined' || 'userAgentData' in navigator) {{
    const _brands = {brands};
    const _uaData = {{
      brands: _brands,
      mobile: {mobile_js},
      platform: '{platform}',
      getHighEntropyValues: function(hints) {{
        return Promise.resolve({{
          architecture: 'x86',
          model: '',
          platform: '{platform}',
          platformVersion: '{os_version}',
          fullVersionList: _brands,
          mobile: {mobile_js},
          bitness: '64',
          wow64: false,
        }});
      }},
      toJSON: function() {{
        return {{ brands: _brands, mobile: {mobile_js}, platform: '{platform}' }};
      }},
    }};
    Object.defineProperty(Navigator.prototype, 'userAgentData', {{
      get: function() {{ return _uaData; }}, configurable: false, enumerable: true
    }});
  }}",
        brands = brands,
        mobile_js = if mobile { "true" } else { "false" },
        platform = platform,
        os_version = os_version,
    )
}

/// Parse `Sec-CH-UA` header string into a JS array literal of brand objects.
///
/// Input: `"Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99"`
/// Output: `[{brand:"Chromium",version:"136"}, ...]`
fn parse_sec_ch_ua_brands(sec_ch_ua: &str) -> String {
    use std::fmt::Write;

    let mut result = String::from('[');
    for part in sec_ch_ua.split(',') {
        let part = part.trim();
        // Each part: `"Brand";v="version"`
        let mut iter = part.splitn(2, ";v=");
        let brand_raw = iter.next().unwrap_or("").trim().trim_matches('"');
        let version_raw = iter.next().unwrap_or("\"\"").trim().trim_matches('"');
        if !brand_raw.is_empty() {
            if result.len() > 1 {
                result.push(',');
            }
            let _ = write!(
                result,
                "{{brand:\"{brand_raw}\",version:\"{version_raw}\"}}",
            );
        }
    }
    result.push(']');
    result
}

/// Strip surrounding quotes from a `sec_ch_ua_platform` value like `"Windows"`.
fn strip_quotes(s: &str) -> String {
    s.trim_matches('"').to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FingerprintProfile;

    fn script_for(p: &FingerprintProfile) -> String {
        navigator_coherence_script(p)
    }

    #[test]
    fn script_overrides_all_nine_groups() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        let js = script_for(&p);
        assert!(
            js.contains("hardwareConcurrency"),
            "missing hardwareConcurrency"
        );
        assert!(js.contains("deviceMemory"), "missing deviceMemory");
        assert!(js.contains("connection"), "missing connection");
        assert!(js.contains("maxTouchPoints"), "missing maxTouchPoints");
        assert!(js.contains("languages"), "missing languages");
        assert!(js.contains("pdfViewerEnabled"), "missing pdfViewerEnabled");
        assert!(js.contains("plugins"), "missing plugins");
        assert!(js.contains("userAgentData"), "missing userAgentData");
        assert!(js.contains("userAgent"), "missing userAgent");
    }

    #[test]
    fn hardware_concurrency_matches_profile() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        let js = script_for(&p);
        assert!(
            js.contains(&format!("return {};", p.hardware.cores)),
            "hardwareConcurrency value not found"
        );
    }

    #[test]
    fn device_memory_matches_profile() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        let js = script_for(&p);
        assert!(
            js.contains(&format!("return {};", p.hardware.memory_gb)),
            "deviceMemory value not found"
        );
    }

    #[test]
    fn connection_has_all_four_properties() {
        let js = script_for(&FingerprintProfile::windows_chrome_136_rtx3060());
        assert!(js.contains("rtt"), "missing rtt");
        assert!(js.contains("downlink"), "missing downlink");
        assert!(js.contains("effectiveType"), "missing effectiveType");
        assert!(js.contains("saveData"), "missing saveData");
    }

    #[test]
    fn plugins_count_five_for_chrome() {
        let js = script_for(&FingerprintProfile::windows_chrome_136_rtx3060());
        assert!(js.contains("PDF Viewer"), "missing PDF Viewer plugin");
        assert!(
            js.contains("Chrome PDF Viewer"),
            "missing Chrome PDF Viewer"
        );
        assert!(
            js.contains("Chromium PDF Viewer"),
            "missing Chromium PDF Viewer"
        );
        assert!(
            js.contains("Microsoft Edge PDF Viewer"),
            "missing Edge plugin"
        );
        assert!(js.contains("WebKit built-in PDF"), "missing WebKit plugin");
    }

    #[test]
    fn ua_data_brands_match_sec_ch_ua() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        let js = script_for(&p);
        assert!(js.contains("Google Chrome"), "missing Google Chrome brand");
        assert!(js.contains("Chromium"), "missing Chromium brand");
    }

    #[test]
    fn max_touch_points_desktop_is_zero() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        assert_eq!(p.platform.max_touch_points, 0);
        let js = script_for(&p);
        assert!(js.contains("return 0;"), "maxTouchPoints not 0 on desktop");
    }

    #[test]
    fn max_touch_points_mobile_is_nonzero() {
        let p = FingerprintProfile::android_chrome_136_pixel();
        assert!(p.platform.max_touch_points > 0);
        let js = script_for(&p);
        assert!(
            js.contains(&format!("return {};", p.platform.max_touch_points)),
            "maxTouchPoints not > 0 on mobile"
        );
    }

    #[test]
    fn parse_sec_ch_ua_brands_roundtrip() {
        let input = r#""Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99""#;
        let out = parse_sec_ch_ua_brands(input);
        assert!(out.contains("Chromium"), "Chromium not parsed");
        assert!(out.contains("136"), "version 136 not parsed");
    }
}
