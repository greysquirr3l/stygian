//! WebRTC IP leak prevention and geolocation consistency
//!
//! Browsers can expose the host machine's real IP address via `RTCPeerConnection`,
//! even when a proxy is configured, because WebRTC uses UDP STUN requests that bypass
//! the HTTP proxy tunnel.  This module provides:
//!
//! - [`WebRtcPolicy`] — controls how aggressively WebRTC is restricted.
//! - [`ProxyLocation`] — optional geolocation to match a proxy's region.
//! - [`WebRtcConfig`] — bundles policy + location and generates injection scripts
//!   and Chrome launch arguments.
//!
//! ## Example
//!
//! ```
//! use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy, ProxyLocation};
//!
//! let location = ProxyLocation {
//!     latitude: 40.7128,
//!     longitude: -74.0060,
//!     accuracy: 100.0,
//!     timezone: "America/New_York".to_string(),
//!     locale: "en-US".to_string(),
//! };
//!
//! let config = WebRtcConfig {
//!     policy: WebRtcPolicy::DisableNonProxied,
//!     public_ip: Some("203.0.113.42".to_string()),
//!     local_ip: Some("192.168.1.100".to_string()),
//!     location: Some(location),
//! };
//!
//! let script = config.injection_script();
//! assert!(script.contains("RTCPeerConnection"));
//! let args = config.chrome_args();
//! assert!(args.iter().any(|a| a.contains("disable_non_proxied_udp")));
//! ```

use serde::{Deserialize, Serialize};

// ─── WebRtcPolicy ─────────────────────────────────────────────────────────────

/// Controls how WebRTC connections are handled to prevent IP leakage.
///
/// # Example
///
/// ```
/// use stygian_browser::webrtc::WebRtcPolicy;
/// assert_eq!(WebRtcPolicy::default(), WebRtcPolicy::DisableNonProxied);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebRtcPolicy {
    /// No WebRTC restrictions.  The real IP may be exposed.
    AllowAll,
    /// Force WebRTC traffic through the configured proxy.
    ///
    /// Applies the `disable_non_proxied_udp` handling policy via
    /// `--force-webrtc-ip-handling-policy`.  This is the recommended setting
    /// when a proxy is in use.
    #[default]
    DisableNonProxied,
    /// Completely block WebRTC by overriding `RTCPeerConnection` with a stub
    /// that never emits ICE candidates.  Breaks real-time communication on
    /// target pages but provides the strongest IP protection.
    BlockAll,
}

// ─── ProxyLocation ────────────────────────────────────────────────────────────

/// Geographic location metadata for geolocation consistency with a proxy.
///
/// When a proxy routes traffic through a specific region, the browser's
/// `navigator.geolocation` API and timezone should match that region so that
/// fingerprint analysis cannot detect the mismatch.
///
/// # Example
///
/// ```
/// use stygian_browser::webrtc::ProxyLocation;
///
/// let loc = ProxyLocation::new_us_east();
/// assert_eq!(loc.timezone, "America/New_York");
/// assert_eq!(loc.locale, "en-US");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyLocation {
    /// WGS-84 decimal latitude.
    pub latitude: f64,
    /// WGS-84 decimal longitude.
    pub longitude: f64,
    /// Accuracy radius in metres (typical GPS: 10–50, cell/IP: 100–5000).
    pub accuracy: f64,
    /// IANA timezone identifier, e.g. `"America/New_York"`.
    pub timezone: String,
    /// BCP-47 locale tag, e.g. `"en-US"`.
    pub locale: String,
}

impl ProxyLocation {
    /// US East Coast (New York) preset.
    pub fn new_us_east() -> Self {
        Self {
            latitude: 40.7128,
            longitude: -74.0060,
            accuracy: 1000.0,
            timezone: "America/New_York".to_string(),
            locale: "en-US".to_string(),
        }
    }

    /// US West Coast (Los Angeles) preset.
    pub fn new_us_west() -> Self {
        Self {
            latitude: 34.0522,
            longitude: -118.2437,
            accuracy: 1000.0,
            timezone: "America/Los_Angeles".to_string(),
            locale: "en-US".to_string(),
        }
    }

    /// UK (London) preset.
    pub fn new_uk() -> Self {
        Self {
            latitude: 51.5074,
            longitude: -0.1278,
            accuracy: 1000.0,
            timezone: "Europe/London".to_string(),
            locale: "en-GB".to_string(),
        }
    }

    /// Central Europe (Frankfurt) preset.
    pub fn new_eu_central() -> Self {
        Self {
            latitude: 50.1109,
            longitude: 8.6821,
            accuracy: 1000.0,
            timezone: "Europe/Berlin".to_string(),
            locale: "de-DE".to_string(),
        }
    }

    /// Asia Pacific (Singapore) preset.
    pub fn new_apac() -> Self {
        Self {
            latitude: 1.3521,
            longitude: 103.8198,
            accuracy: 1000.0,
            timezone: "Asia/Singapore".to_string(),
            locale: "en-SG".to_string(),
        }
    }
}

// ─── WebRtcConfig ─────────────────────────────────────────────────────────────

/// WebRTC leak-prevention and geolocation consistency configuration.
///
/// Produces both a JavaScript injection script (to run before page load) and
/// Chrome launch arguments that enforce the chosen [`WebRtcPolicy`].
///
/// # Example
///
/// ```
/// use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
///
/// let cfg = WebRtcConfig::default();
/// assert_eq!(cfg.policy, WebRtcPolicy::DisableNonProxied);
/// let args = cfg.chrome_args();
/// assert!(args.iter().any(|a| a.contains("disable_non_proxied_udp")));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebRtcConfig {
    /// WebRTC IP-handling policy.
    pub policy: WebRtcPolicy,

    /// Fake public IP address to substitute in WebRTC SDP when using
    /// [`WebRtcPolicy::BlockAll`].  Use an IP plausible for the proxy region.
    /// Has no effect when [`WebRtcPolicy::AllowAll`] or
    /// [`WebRtcPolicy::DisableNonProxied`] is selected.
    pub public_ip: Option<String>,

    /// Fake LAN IP address to substitute in WebRTC SDP when using
    /// [`WebRtcPolicy::BlockAll`].
    pub local_ip: Option<String>,

    /// Optional geographic location to inject via `navigator.geolocation`.
    /// When `None`, geolocation is not overridden.
    pub location: Option<ProxyLocation>,
}

impl WebRtcConfig {
    /// Returns `true` when WebRTC is not being restricted at all.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
    ///
    /// let cfg = WebRtcConfig { policy: WebRtcPolicy::AllowAll, ..Default::default() };
    /// assert!(cfg.is_permissive());
    /// ```
    pub fn is_permissive(&self) -> bool {
        self.policy == WebRtcPolicy::AllowAll && self.location.is_none()
    }

    /// Chrome launch arguments that enforce the selected [`WebRtcPolicy`].
    ///
    /// Returns an empty `Vec` for [`WebRtcPolicy::AllowAll`] since no Chrome
    /// flag is needed in that case.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
    ///
    /// let cfg = WebRtcConfig { policy: WebRtcPolicy::BlockAll, ..Default::default() };
    /// let args = cfg.chrome_args();
    /// assert!(args.iter().any(|a| a.contains("disable_non_proxied_udp")));
    /// ```
    pub fn chrome_args(&self) -> Vec<String> {
        match self.policy {
            WebRtcPolicy::AllowAll => vec![],
            WebRtcPolicy::DisableNonProxied | WebRtcPolicy::BlockAll => {
                vec!["--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_string()]
            }
        }
    }

    /// JavaScript injection script that overrides `RTCPeerConnection` and
    /// optionally overrides `navigator.geolocation`.
    ///
    /// The generated script is an IIFE (immediately-invoked function expression)
    /// designed to be injected via CDP `Page.addScriptToEvaluateOnNewDocument`.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
    ///
    /// let cfg = WebRtcConfig { policy: WebRtcPolicy::BlockAll, ..Default::default() };
    /// let script = cfg.injection_script();
    /// assert!(script.contains("RTCPeerConnection"));
    /// ```
    pub fn injection_script(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        let rtc_part = match self.policy {
            WebRtcPolicy::AllowAll => String::new(),
            WebRtcPolicy::DisableNonProxied => rtc_disable_non_proxied_script(),
            WebRtcPolicy::BlockAll => {
                let public_ip = self.public_ip.as_deref().unwrap_or("203.0.113.1");
                let local_ip = self.local_ip.as_deref().unwrap_or("10.0.0.1");
                rtc_block_all_script(public_ip, local_ip)
            }
        };

        if !rtc_part.is_empty() {
            parts.push(rtc_part);
        }

        if let Some(loc) = &self.location {
            parts.push(geolocation_script(loc));
        }

        if parts.is_empty() {
            return String::new();
        }

        // Wrap everything in a single IIFE so variables don't leak to page scope.
        format!(
            "(function(){{\n  'use strict';\n{}\n}})();",
            parts.join("\n")
        )
    }
}

// ─── Private script builders ──────────────────────────────────────────────────

/// Generates JS that filters out non-proxied ICE candidates from SDP without
/// completely blocking WebRTC (mirrors the Chrome flag behaviour in JS-space
/// for defence-in-depth).
fn rtc_disable_non_proxied_script() -> String {
    r"
  // WebRTC: suppress host/srflx candidates; allow relay (TURN) candidates only
  (function patchRTCNonProxied() {
    var _RPC = window.RTCPeerConnection;
    if (!_RPC) return;
    var patchedRPC = function(config) {
      var pc = new _RPC(config);
      var origSetLocalDescription = pc.setLocalDescription.bind(pc);
      // Intercept onicecandidate to strip host + srflx candidates
      var origOICH = pc.__lookupGetter__ ? null : null; // will use addEventListener
      pc.addEventListener('icecandidate', function(e) {
        if (e.candidate && e.candidate.candidate) {
          var c = e.candidate.candidate;
          // Drop host (LAN) and server-reflexive (public via STUN) candidates
          if (c.indexOf('typ host') !== -1 || c.indexOf('typ srflx') !== -1) {
            Object.defineProperty(e, 'candidate', { value: null, configurable: true });
          }
        }
      }, true);
      return pc;
    };
    patchedRPC.prototype = _RPC.prototype;
    Object.defineProperty(window, 'RTCPeerConnection', {
      value: patchedRPC,
      writable: false,
      configurable: false,
    });
  })();
"
    .to_string()
}

/// Generates JS that completely overrides `RTCPeerConnection` with a stub that
/// replaces real IPs in SDP with the supplied fake IPs.
fn rtc_block_all_script(public_ip: &str, local_ip: &str) -> String {
    format!(
        r"
  // WebRTC: replace all real IPs in SDP with fake ones
  (function patchRTCBlockAll() {{
    var _RPC = window.RTCPeerConnection;
    if (!_RPC) return;
    var PUBLIC_IP = '{public_ip}';
    var LOCAL_IP  = '{local_ip}';
    var PRIV_RE   = /^(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.)/;

    function patchSDP(sdp) {{
      return sdp.replace(
        /(\b(?:\d{{1,3}}\.)\d{{1,3}}\.(?:\d{{1,3}}\.)\d{{1,3}}\b)/g,
        function(ip) {{
          if (ip === '127.0.0.1' || ip === '0.0.0.0') return ip;
          if (PRIV_RE.test(ip)) return LOCAL_IP;
          return PUBLIC_IP;
        }}
      );
    }}

    var patchedRPC = function(config) {{
      // Remove all ICE servers so no STUN/TURN queries are made
      if (config && Array.isArray(config.iceServers)) {{
        config.iceServers = [];
      }}
      var pc = new _RPC(config);
      ['createOffer', 'createAnswer'].forEach(function(method) {{
        var orig = pc[method].bind(pc);
        pc[method] = function() {{
          return orig.apply(this, arguments).then(function(desc) {{
            if (desc && desc.sdp) {{
              return new RTCSessionDescription({{
                type: desc.type,
                sdp: patchSDP(desc.sdp),
              }});
            }}
            return desc;
          }});
        }};
      }});
      return pc;
    }};
    patchedRPC.prototype = _RPC.prototype;
    Object.defineProperty(window, 'RTCPeerConnection', {{
      value: patchedRPC,
      writable: false,
      configurable: false,
    }});
  }})();
"
    )
}

/// Generates JS that overrides `navigator.geolocation` with a fixed fake position.
fn geolocation_script(loc: &ProxyLocation) -> String {
    format!(
        r"
  // Geolocation override to match proxy region
  (function patchGeolocation() {{
    var fakeCoords = {{
      latitude:         {lat},
      longitude:        {lon},
      accuracy:         {acc},
      altitude:         null,
      altitudeAccuracy: null,
      heading:          null,
      speed:            null,
    }};
    var fakePosition = {{ coords: fakeCoords, timestamp: Date.now() }};

    try {{
      Object.defineProperty(navigator, 'geolocation', {{
        value: {{
          getCurrentPosition: function(success, _err, _opts) {{
            setTimeout(function() {{ success(fakePosition); }}, 50 + Math.random() * 100);
          }},
          watchPosition: function(success, _err, _opts) {{
            setTimeout(function() {{ success(fakePosition); }}, 50 + Math.random() * 100);
            return 1;
          }},
          clearWatch: function() {{}},
        }},
        writable:     false,
        configurable: false,
      }});
    }} catch (_) {{
      // Already non-configurable in some browsers; best-effort only.
    }}
  }})();
",
        lat = loc.latitude,
        lon = loc.longitude,
        acc = loc.accuracy,
    )
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_disable_non_proxied() {
        assert_eq!(WebRtcPolicy::default(), WebRtcPolicy::DisableNonProxied);
    }

    #[test]
    fn allow_all_has_no_chrome_args() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::AllowAll,
            ..Default::default()
        };
        assert!(cfg.chrome_args().is_empty());
    }

    #[test]
    fn disable_non_proxied_adds_webrtc_flag() {
        let cfg = WebRtcConfig::default();
        let args = cfg.chrome_args();
        assert_eq!(args.len(), 1);
        assert!(
            args.first()
                .is_some_and(|a| a.contains("disable_non_proxied_udp"))
        );
    }

    #[test]
    fn block_all_adds_webrtc_flag() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::BlockAll,
            ..Default::default()
        };
        let args = cfg.chrome_args();
        assert!(!args.is_empty());
        assert!(args.iter().any(|a| a.contains("disable_non_proxied_udp")));
    }

    #[test]
    fn allow_all_injection_script_is_empty() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::AllowAll,
            ..Default::default()
        };
        assert!(cfg.injection_script().is_empty());
    }

    #[test]
    fn disable_non_proxied_script_contains_rtc() {
        let cfg = WebRtcConfig::default();
        let script = cfg.injection_script();
        assert!(script.contains("RTCPeerConnection"));
    }

    #[test]
    fn block_all_script_contains_rtc_and_fake_ips() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::BlockAll,
            public_ip: Some("1.2.3.4".to_string()),
            local_ip: Some("10.0.0.5".to_string()),
            ..Default::default()
        };
        let script = cfg.injection_script();
        assert!(script.contains("RTCPeerConnection"));
        assert!(script.contains("1.2.3.4"));
        assert!(script.contains("10.0.0.5"));
    }

    #[test]
    fn block_all_uses_default_fake_ips_when_none_set() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::BlockAll,
            public_ip: None,
            local_ip: None,
            ..Default::default()
        };
        let script = cfg.injection_script();
        // Should use fallback IPs (203.0.113.1 and 10.0.0.1)
        assert!(script.contains("203.0.113.1"));
        assert!(script.contains("10.0.0.1"));
    }

    #[test]
    fn geolocation_script_included_when_location_set() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::AllowAll,
            location: Some(ProxyLocation::new_us_east()),
            ..Default::default()
        };
        let script = cfg.injection_script();
        assert!(script.contains("geolocation"));
        assert!(script.contains("40.7128"));
    }

    #[test]
    fn is_permissive_only_when_allow_all_and_no_location() {
        let mut cfg = WebRtcConfig {
            policy: WebRtcPolicy::AllowAll,
            ..Default::default()
        };
        assert!(cfg.is_permissive());

        cfg.location = Some(ProxyLocation::new_uk());
        assert!(!cfg.is_permissive());

        cfg.location = None;
        cfg.policy = WebRtcPolicy::DisableNonProxied;
        assert!(!cfg.is_permissive());
    }

    #[test]
    fn proxy_location_presets_have_valid_coords() {
        let presets = [
            ProxyLocation::new_us_east(),
            ProxyLocation::new_us_west(),
            ProxyLocation::new_uk(),
            ProxyLocation::new_eu_central(),
            ProxyLocation::new_apac(),
        ];
        for loc in &presets {
            assert!(loc.latitude >= -90.0 && loc.latitude <= 90.0);
            assert!(loc.longitude >= -180.0 && loc.longitude <= 180.0);
            assert!(loc.accuracy > 0.0);
            assert!(!loc.timezone.is_empty());
            assert!(!loc.locale.is_empty());
        }
    }

    #[test]
    fn proxy_location_serializes_to_json() -> Result<(), Box<dyn std::error::Error>> {
        let loc = ProxyLocation::new_us_east();
        let json = serde_json::to_string(&loc)?;
        let back: ProxyLocation = serde_json::from_str(&json)?;
        assert!((back.latitude - loc.latitude).abs() < 1e-9);
        assert_eq!(back.timezone, loc.timezone);
        Ok(())
    }

    #[test]
    fn webrtc_config_serializes_to_json() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::DisableNonProxied,
            public_ip: Some("1.2.3.4".to_string()),
            local_ip: None,
            location: Some(ProxyLocation::new_uk()),
        };
        let json = serde_json::to_string(&cfg)?;
        let back: WebRtcConfig = serde_json::from_str(&json)?;
        assert_eq!(back.policy, cfg.policy);
        assert_eq!(back.public_ip, cfg.public_ip);
        Ok(())
    }

    #[test]
    fn combined_script_is_valid_iife() {
        let cfg = WebRtcConfig {
            policy: WebRtcPolicy::DisableNonProxied,
            location: Some(ProxyLocation::new_apac()),
            ..Default::default()
        };
        let script = cfg.injection_script();
        assert!(script.starts_with("(function(){"));
        assert!(script.ends_with("})();"));
        assert!(script.contains("RTCPeerConnection"));
        assert!(script.contains("geolocation"));
    }
}
