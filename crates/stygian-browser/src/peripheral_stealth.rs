//! Peripheral detection surface hardening.
//!
//! Covers secondary browser detection vectors that build bot-confidence scores:
//!
//! 1. **iframe `innerWidth` mismatch** — ensures iframe inner dimensions differ
//!    slightly from the parent window (Kasada check).
//! 2. **Document visibility** — forces `document.hidden = false` and
//!    `visibilityState = "visible"`.
//! 3. **Camera / microphone device names** — returns platform-appropriate fake
//!    device names with seeded random `deviceId`/`groupId`.
//! 4. **Port scan protection** — blocks `fetch` and `XMLHttpRequest` to
//!    localhost on commonly-probed ports.
//! 5. **History length** — overrides `history.length` to a plausible 3-8.
//! 6. **`requestAnimationFrame` timing** — adds light per-frame jitter to
//!    prevent headless rAF timing detection.
//! 7. **PDF viewer** — ensures `navigator.pdfViewerEnabled` reads `true`.
//!
//! # Example
//!
//! ```
//! use stygian_browser::peripheral_stealth::{peripheral_stealth_script, PeripheralStealthConfig};
//! use stygian_browser::noise::NoiseSeed;
//!
//! let cfg = PeripheralStealthConfig::default_with_seed(NoiseSeed::from(1_u64));
//! let js = peripheral_stealth_script(&cfg);
//! assert!(js.contains("document.hidden"));
//! assert!(js.contains("history.length"));
//! ```

use serde::{Deserialize, Serialize};

use crate::noise::{NoiseEngine, NoiseSeed};
use crate::profile::{FingerprintProfile, Os};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Per-subsystem toggles for peripheral stealth injection.
///
/// # Example
///
/// ```
/// use stygian_browser::peripheral_stealth::PeripheralStealthConfig;
/// use stygian_browser::noise::NoiseSeed;
///
/// let cfg = PeripheralStealthConfig::default_with_seed(NoiseSeed::from(1_u64));
/// assert!(cfg.iframe_inner_width);
/// assert!(cfg.always_visible);
/// assert!(cfg.fake_media_devices);
/// assert!(cfg.block_port_scan);
/// assert!(cfg.history_length);
/// assert!(cfg.raf_timing);
/// assert!(cfg.pdf_viewer);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralStealthConfig {
    /// Adjust iframe `contentWindow.innerWidth` to differ from parent.
    pub iframe_inner_width: bool,
    /// Force `document.hidden = false` / `visibilityState = "visible"`.
    pub always_visible: bool,
    /// Return platform-appropriate fake camera/microphone devices.
    pub fake_media_devices: bool,
    /// Block fetch/XHR to localhost probe ports silently.
    pub block_port_scan: bool,
    /// Override `history.length` to a plausible value (3–8).
    pub history_length: bool,
    /// Add per-frame jitter to `requestAnimationFrame`.
    pub raf_timing: bool,
    /// Ensure `navigator.pdfViewerEnabled` returns `true`.
    pub pdf_viewer: bool,
    /// Noise seed for deterministic device IDs and history length.
    pub seed: NoiseSeed,
}

impl PeripheralStealthConfig {
    /// All subsystems enabled, with a given seed.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::peripheral_stealth::PeripheralStealthConfig;
    /// use stygian_browser::noise::NoiseSeed;
    ///
    /// let cfg = PeripheralStealthConfig::default_with_seed(NoiseSeed::from(42_u64));
    /// assert!(cfg.iframe_inner_width);
    /// ```
    #[must_use]
    pub const fn default_with_seed(seed: NoiseSeed) -> Self {
        Self {
            iframe_inner_width: true,
            always_visible: true,
            fake_media_devices: true,
            block_port_scan: true,
            history_length: true,
            raf_timing: true,
            pdf_viewer: true,
            seed,
        }
    }
}

impl Default for PeripheralStealthConfig {
    fn default() -> Self {
        Self::default_with_seed(NoiseSeed::random())
    }
}

// ---------------------------------------------------------------------------
// Script generator
// ---------------------------------------------------------------------------

/// Generate the peripheral stealth injection script.
///
/// Pass `fingerprint_profile` when you want platform-appropriate device names;
/// if `None`, defaults to Windows-like device names.
///
/// # Example
///
/// ```
/// use stygian_browser::peripheral_stealth::{peripheral_stealth_script, PeripheralStealthConfig};
/// use stygian_browser::noise::NoiseSeed;
///
/// let cfg = PeripheralStealthConfig::default_with_seed(NoiseSeed::from(1_u64));
/// let js = peripheral_stealth_script(&cfg);
/// assert!(js.contains("visibilityState"));
/// ```
#[must_use]
pub fn peripheral_stealth_script(config: &PeripheralStealthConfig) -> String {
    peripheral_stealth_script_with_profile(config, None)
}

/// Generate peripheral stealth script with optional [`FingerprintProfile`] for
/// platform-aware device names.
///
/// # Example
///
/// ```
/// use stygian_browser::peripheral_stealth::{
///     peripheral_stealth_script_with_profile, PeripheralStealthConfig,
/// };
/// use stygian_browser::profile::FingerprintProfile;
/// use stygian_browser::noise::NoiseSeed;
///
/// let cfg = PeripheralStealthConfig::default_with_seed(NoiseSeed::from(1_u64));
/// let profile = FingerprintProfile::macos_chrome_136_m1();
/// let js = peripheral_stealth_script_with_profile(&cfg, Some(&profile));
/// assert!(js.contains("FaceTime"));
/// ```
#[must_use]
pub fn peripheral_stealth_script_with_profile(
    config: &PeripheralStealthConfig,
    fingerprint_profile: Option<&FingerprintProfile>,
) -> String {
    let engine = NoiseEngine::new(config.seed);

    let mut sections: Vec<String> = Vec::new();

    // ── 1. iframe innerWidth ─────────────────────────────────────────────────
    if config.iframe_inner_width {
        sections.push(IFRAME_INNER_WIDTH_SECTION.to_string());
    }

    // ── 2. Document visibility ────────────────────────────────────────────────
    if config.always_visible {
        sections.push(VISIBILITY_SECTION.to_string());
    }

    // ── 3. Camera / microphone devices ────────────────────────────────────────
    if config.fake_media_devices {
        let os = fingerprint_profile.map(|p| &p.platform.os);
        let video_device = platform_video_device(os);
        let audio_device = platform_audio_device(os);

        // Derive deterministic hex hashes for device/group IDs
        let video_device_id = engine.hex_id("media.video.device_id");
        let video_group_id = engine.hex_id("media.video.group_id");
        let audio_device_id = engine.hex_id("media.audio.device_id");
        let audio_group_id = engine.hex_id("media.audio.group_id");

        sections.push(format!(
            r"  // ── 3. Fake media devices (camera / microphone) ───────────────────────
  if (navigator.mediaDevices && navigator.mediaDevices.enumerateDevices) {{
    const _fakeDevices = [
      {{
        deviceId: '{video_device_id}',
        groupId: '{video_group_id}',
        kind: 'videoinput',
        label: '{video_device}',
        toJSON: function() {{
          return {{ deviceId: '{video_device_id}', groupId: '{video_group_id}', kind: 'videoinput', label: '{video_device}' }};
        }},
      }},
      {{
        deviceId: '{audio_device_id}',
        groupId: '{audio_group_id}',
        kind: 'audioinput',
        label: '{audio_device}',
        toJSON: function() {{
          return {{ deviceId: '{audio_device_id}', groupId: '{audio_group_id}', kind: 'audioinput', label: '{audio_device}' }};
        }},
      }},
    ];
    const _origEnum = navigator.mediaDevices.enumerateDevices.bind(navigator.mediaDevices);
    Object.defineProperty(navigator.mediaDevices, 'enumerateDevices', {{
      value: function enumerateDevices() {{
        return _origEnum().then(function(real) {{
          // If real devices are present (permissions granted), return them;
          // otherwise return our fake list to avoid empty-list detection.
          return real.length > 0 && real.some(function(d) {{ return d.label !== ''; }})
            ? real
            : _fakeDevices;
        }});
      }},
      writable: false, configurable: false, enumerable: true,
    }});
  }}",
        ));
    }

    // ── 4. Port scan protection ────────────────────────────────────────────────
    if config.block_port_scan {
        sections.push(PORT_SCAN_SECTION.to_string());
    }

    // ── 5. History length ─────────────────────────────────────────────────────
    if config.history_length {
        // Derive a stable value in [3, 8] from the seed
        let history_len = 3u64 + (engine.u64_noise("history.length") % 6);
        sections.push(format!(
            r"  // ── 5. History length ──────────────────────────────────────────────────
  try {{
    Object.defineProperty(History.prototype, 'length', {{
      get: function() {{ return {history_len}; }},
      configurable: true, enumerable: true,
    }});
  }} catch(e) {{}}",
        ));
    }

    // ── 6. requestAnimationFrame timing ──────────────────────────────────────
    if config.raf_timing {
        sections.push(RAF_TIMING_SECTION.to_string());
    }

    // ── 7. pdfViewerEnabled ───────────────────────────────────────────────────
    if config.pdf_viewer {
        sections.push(PDF_VIEWER_SECTION.to_string());
    }

    if sections.is_empty() {
        return String::new();
    }

    format!(
        "(function() {{\n  'use strict';\n\n{body}\n\n}})();\n",
        body = sections.join("\n\n"),
    )
}

// ---------------------------------------------------------------------------
// Platform-aware device names
// ---------------------------------------------------------------------------

const fn platform_video_device(os: Option<&Os>) -> &'static str {
    match os {
        Some(Os::MacOs | Os::Ios) => "FaceTime HD Camera",
        Some(Os::Linux) => "USB2.0 PC Camera",
        Some(Os::Android) => "Camera 0",
        _ => "Integrated Webcam",
    }
}

const fn platform_audio_device(os: Option<&Os>) -> &'static str {
    match os {
        Some(Os::MacOs | Os::Ios) => "MacBook Pro Microphone",
        Some(Os::Linux) => "Built-in Audio Analog Stereo",
        Some(Os::Android) => "Default",
        _ => "Microphone (Realtek Audio)",
    }
}

// ---------------------------------------------------------------------------
// Static script sections
// ---------------------------------------------------------------------------

const IFRAME_INNER_WIDTH_SECTION: &str = r"  // ── 1. iframe innerWidth mismatch ─────────────────────────────────────
  try {
    const _origContentWindow = Object.getOwnPropertyDescriptor(
      HTMLIFrameElement.prototype, 'contentWindow'
    );
    if (_origContentWindow && _origContentWindow.get) {
      const _origGetter = _origContentWindow.get;
      Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
        get: function() {
          const cw = _origGetter.call(this);
          if (!cw) { return cw; }
          // Expose a border-adjusted innerWidth to prevent Kasada's
          // iframe-vs-window width equality check
          const _iframeWidth = cw.innerWidth;
          if (typeof _iframeWidth === 'number' && _iframeWidth === window.innerWidth) {
            try {
              Object.defineProperty(cw, 'innerWidth', {
                get: function() { return _iframeWidth - 17; }, // scrollbar offset
                configurable: true,
              });
            } catch(e) {}
          }
          return cw;
        },
        configurable: true,
        enumerable: true,
      });
    }
  } catch(e) {}";

const VISIBILITY_SECTION: &str = r"  // ── 2. Document visibility ─────────────────────────────────────────────
  try {
    Object.defineProperty(Document.prototype, 'hidden', {
      get: function() { return false; }, configurable: false, enumerable: true,
    });
    Object.defineProperty(Document.prototype, 'visibilityState', {
      get: function() { return 'visible'; }, configurable: false, enumerable: true,
    });
    // Filter visibilitychange events from propagating
    const _origDocAEL = Document.prototype.addEventListener;
    Document.prototype.addEventListener = function addEventListener(type, listener, opts) {
      if (type === 'visibilitychange') { return; }
      return _origDocAEL.call(this, type, listener, opts);
    };
    Document.prototype.addEventListener.toString = function toString() {
      return 'function addEventListener() { [native code] }';
    };
  } catch(e) {}";

const PORT_SCAN_SECTION: &str = r"  // ── 4. Port scan protection ──────────────────────────────────────────────
  (function() {
    // Ports commonly probed by anti-bot scripts during port scanning
    const _probePorts = new Set([
      22, 23, 25, 80, 443, 3000, 3001, 3002, 3389, 3999,
      5000, 5432, 5500, 5900, 6379, 8080, 8081, 8082, 8083,
      8084, 8085, 8086, 8087, 8088, 8089, 8090, 8091, 8092,
      8093, 8094, 8095, 8096, 8097, 8098, 8099, 9229,
    ]);
    const _localHosts = ['127.0.0.1', 'localhost', '[::1]', '0.0.0.0', '::1'];
    function _isLocalProbe(url) {
      try {
        const u = new URL(url);
        const port = parseInt(u.port || (u.protocol === 'https:' ? '443' : '80'), 10);
        return _localHosts.some(function(h) { return u.hostname === h; }) &&
               _probePorts.has(port);
      } catch(e) { return false; }
    }
    // Wrap fetch
    const _origFetch = window.fetch;
    window.fetch = function fetch(resource, init) {
      const url = typeof resource === 'string' ? resource
        : resource instanceof Request ? resource.url : String(resource);
      if (_isLocalProbe(url)) {
        return Promise.reject(new TypeError('Failed to fetch'));
      }
      return _origFetch.apply(window, arguments);
    };
    window.fetch.toString = function toString() {
      return 'function fetch() { [native code] }';
    };
    // Wrap XMLHttpRequest.open
    const _origXhrOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function open(method, url) {
      if (_isLocalProbe(String(url))) {
        // Silently make this request go nowhere
        return _origXhrOpen.call(this, method, 'about:blank');
      }
      return _origXhrOpen.apply(this, arguments);
    };
    XMLHttpRequest.prototype.open.toString = function toString() {
      return 'function open() { [native code] }';
    };
  })();";

const RAF_TIMING_SECTION: &str = r"  // ── 6. requestAnimationFrame timing jitter ─────────────────────────────
  try {
    const _origRAF = window.requestAnimationFrame;
    let __raf_counter = 0;
    window.requestAnimationFrame = function requestAnimationFrame(callback) {
      const _frame = __raf_counter++;
      return _origRAF.call(window, function(timestamp) {
        // Add sub-millisecond jitter to the rAF timestamp to simulate real
        // vsync timing variation (±0.1 ms max)
        const jitter = ((_frame * 2654435761) % 1000) / 10000000.0;
        return callback(timestamp + jitter);
      });
    };
    window.requestAnimationFrame.toString = function toString() {
      return 'function requestAnimationFrame() { [native code] }';
    };
  } catch(e) {}";

const PDF_VIEWER_SECTION: &str = r"  // ── 7. pdfViewerEnabled ─────────────────────────────────────────────────
  try {
    const _pdfDesc = Object.getOwnPropertyDescriptor(Navigator.prototype, 'pdfViewerEnabled');
    if (!_pdfDesc || (_pdfDesc.get && navigator.pdfViewerEnabled !== true)) {
      Object.defineProperty(Navigator.prototype, 'pdfViewerEnabled', {
        get: function() { return true; },
        configurable: false, enumerable: true,
      });
    }
  } catch(e) {}";

// ---------------------------------------------------------------------------
// NoiseEngine hex_id helper (private extension)
// ---------------------------------------------------------------------------

fn f64_bits_to_hex(value: f64) -> String {
  format!("{:016x}", value.to_bits())
}

trait NoiseEngineExt {
    fn hex_id(&self, key: &str) -> String;
    fn u64_noise(&self, key: &str) -> u64;
}

impl NoiseEngineExt for NoiseEngine {
    /// Derive a 64-char hex string (256-bit-equivalent ID) from `key`.
    fn hex_id(&self, key: &str) -> String {
        // Build 4 × u64 and format as hex to approximate a UUID-length device ID
        let a = self.float_noise(key, 0);
        let b = self.float_noise(key, 1);
        let c = self.float_noise(key, 2);
        let d = self.float_noise(key, 3);
      format!(
        "{}{}{}{}",
        f64_bits_to_hex(a),
        f64_bits_to_hex(b),
        f64_bits_to_hex(c),
        f64_bits_to_hex(d)
      )
    }

    /// Derive a deterministic `u64` from `key`.
    fn u64_noise(&self, key: &str) -> u64 {
        self.float_noise(key, 0).to_bits()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::NoiseSeed;

    fn cfg(seed: u64) -> PeripheralStealthConfig {
        PeripheralStealthConfig::default_with_seed(NoiseSeed::from(seed))
    }

    fn script(seed: u64) -> String {
        peripheral_stealth_script(&cfg(seed))
    }

    // ── iframe innerWidth ─────────────────────────────────────────────────────

    #[test]
    fn iframe_script_adjusts_inner_width() {
        let js = script(1);
        assert!(js.contains("innerWidth"), "missing innerWidth override");
        assert!(
            js.contains("scrollbar offset") || js.contains("17"),
            "missing offset adjustment"
        );
    }

    // ── Document visibility ────────────────────────────────────────────────────

    #[test]
    fn visibility_forces_hidden_false_and_visible() {
        let js = script(1);
        assert!(
            js.contains("document.hidden") || js.contains("'hidden'"),
            "missing hidden"
        );
        assert!(js.contains("visibilityState"), "missing visibilityState");
        assert!(js.contains("'visible'"), "must set visible");
    }

    // ── Camera device names ────────────────────────────────────────────────────

    #[test]
    fn camera_names_windows_default() {
        let cfg = cfg(1);
        let js = peripheral_stealth_script_with_profile(&cfg, None);
        assert!(
            js.contains("Integrated Webcam"),
            "missing Windows video device"
        );
        assert!(js.contains("Realtek"), "missing Windows audio device");
    }

    #[test]
    fn camera_names_macos() {
        use crate::profile::FingerprintProfile;
        let cfg = cfg(1);
        let profile = FingerprintProfile::macos_chrome_136_m1();
        let js = peripheral_stealth_script_with_profile(&cfg, Some(&profile));
        assert!(js.contains("FaceTime"), "missing macOS video device");
        assert!(
            js.contains("MacBook Pro Microphone"),
            "missing macOS audio device"
        );
    }

    #[test]
    fn camera_names_linux() {
        use crate::profile::FingerprintProfile;
        let cfg = cfg(1);
        let profile = FingerprintProfile::linux_chrome_136_intel();
        let js = peripheral_stealth_script_with_profile(&cfg, Some(&profile));
        assert!(
            js.contains("USB2.0 PC Camera"),
            "missing Linux video device"
        );
        assert!(js.contains("Built-in Audio"), "missing Linux audio device");
    }

    // ── Port scanning protection ───────────────────────────────────────────────

    #[test]
    fn port_scan_blocks_localhost_fetch() {
        let js = script(1);
        assert!(js.contains("127.0.0.1"), "missing localhost check");
        assert!(js.contains("Failed to fetch"), "missing fetch rejection");
        assert!(js.contains("_probePorts"), "missing probe ports set");
    }

    // ── History length ─────────────────────────────────────────────────────────

    #[test]
    fn history_length_in_range_3_to_8() {
        // Run many seeds and ensure the derived length is always in [3, 8]
        for seed in 0u64..50 {
            let cfg = cfg(seed);
            let engine = NoiseEngine::new(cfg.seed);
            let len = 3u64 + (engine.u64_noise("history.length") % 6);
            assert!(
                (3..=8).contains(&len),
                "history length {len} out of range for seed {seed}"
            );
        }
    }

    #[test]
    fn history_length_script_in_output() {
        let js = script(1);
        assert!(
            js.contains("history.length") || js.contains("History.prototype"),
            "missing history override"
        );
    }

    // ── rAF timing ────────────────────────────────────────────────────────────

    #[test]
    fn raf_timing_script_references_noise() {
        let js = script(1);
        assert!(js.contains("requestAnimationFrame"), "missing rAF override");
        assert!(js.contains("jitter"), "missing jitter variable in rAF");
    }

    // ── Individual toggles ────────────────────────────────────────────────────

    #[test]
    fn each_subsystem_can_be_disabled() {
        let disabled = PeripheralStealthConfig {
            iframe_inner_width: false,
            always_visible: false,
            fake_media_devices: false,
            block_port_scan: false,
            history_length: false,
            raf_timing: false,
            pdf_viewer: false,
            seed: NoiseSeed::from(1_u64),
        };
        assert!(peripheral_stealth_script(&disabled).is_empty());
    }

    #[test]
    fn only_visibility_enabled() {
        let cfg = PeripheralStealthConfig {
            iframe_inner_width: false,
            always_visible: true,
            fake_media_devices: false,
            block_port_scan: false,
            history_length: false,
            raf_timing: false,
            pdf_viewer: false,
            seed: NoiseSeed::from(1_u64),
        };
        let js = peripheral_stealth_script(&cfg);
        assert!(
            js.contains("visibilityState"),
            "visibility should be present"
        );
        assert!(!js.contains("innerWidth"), "iframe should be absent");
        assert!(!js.contains("history.length"), "history should be absent");
    }

    // ── Integration (always ignored) ──────────────────────────────────────────

    #[test]
    #[ignore = "requires launched browser"]
    fn live_document_hidden_returns_false() {}

    #[test]
    #[ignore = "requires launched browser"]
    fn live_history_length_greater_than_one() {}

    #[test]
    #[ignore = "requires launched browser with media permissions"]
    fn live_enumerate_devices_returns_platform_appropriate_names() {}
}
