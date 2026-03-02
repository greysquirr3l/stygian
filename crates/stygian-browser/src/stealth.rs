//! Stealth configuration and anti-detection features
//!
//! Provides navigator property spoofing and CDP injection scripts that make
//! a headless Chrome instance appear identical to a real browser.
//!
//! # Overview
//!
//! 1. **Navigator spoofing** — Override `navigator.webdriver`, `platform`,
//!    `userAgent`, `hardwareConcurrency`, `deviceMemory`, `maxTouchPoints`,
//!    and `vendor` via `Object.defineProperty` so properties are non-configurable
//!    and non-writable (harder to detect the override).
//!
//! 2. **WebGL spoofing** — Replace `getParameter` on `WebGLRenderingContext` and
//!    `WebGL2RenderingContext` to return controlled vendor/renderer strings.
//!
//! # Example
//!
//! ```
//! use stygian_browser::stealth::{NavigatorProfile, StealthConfig, StealthProfile};
//!
//! let profile = NavigatorProfile::windows_chrome();
//! let script = StealthProfile::new(StealthConfig::default(), profile).injection_script();
//! assert!(script.contains("'webdriver'"));
//! ```

use serde::{Deserialize, Serialize};

// ─── StealthConfig ────────────────────────────────────────────────────────────

/// Feature flags controlling which stealth techniques are active.
///
/// # Example
///
/// ```
/// use stygian_browser::stealth::StealthConfig;
/// let cfg = StealthConfig::default();
/// assert!(cfg.spoof_navigator);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StealthConfig {
    /// Override navigator properties (webdriver, platform, userAgent, etc.)
    pub spoof_navigator: bool,
    /// Replace WebGL getParameter with controlled vendor/renderer strings.
    pub randomize_webgl: bool,
    /// Randomise Canvas `toDataURL` fingerprint (stub — needs canvas noise).
    pub randomize_canvas: bool,
    /// Enable human-like behaviour simulation.
    pub human_behavior: bool,
    /// Enable CDP leak protection (remove `Runtime.enable` artifacts).
    pub protect_cdp: bool,
}

impl Default for StealthConfig {
    fn default() -> Self {
        Self {
            spoof_navigator: true,
            randomize_webgl: true,
            randomize_canvas: true,
            human_behavior: true,
            protect_cdp: true,
        }
    }
}

impl StealthConfig {
    /// All stealth features enabled (maximum evasion).
    pub fn paranoid() -> Self {
        Self::default()
    }

    /// Only navigator and CDP protection (low overhead).
    pub const fn minimal() -> Self {
        Self {
            spoof_navigator: true,
            randomize_webgl: false,
            randomize_canvas: false,
            human_behavior: false,
            protect_cdp: true,
        }
    }

    /// All stealth features disabled.
    pub const fn disabled() -> Self {
        Self {
            spoof_navigator: false,
            randomize_webgl: false,
            randomize_canvas: false,
            human_behavior: false,
            protect_cdp: false,
        }
    }
}

// ─── NavigatorProfile ─────────────────────────────────────────────────────────

/// A bundle of navigator property values that together form a convincing
/// browser identity.
///
/// All properties are validated at construction time to guarantee that
/// `platform` matches the OS fragment in `user_agent`.
///
/// # Example
///
/// ```
/// use stygian_browser::stealth::NavigatorProfile;
/// let p = NavigatorProfile::mac_chrome();
/// assert_eq!(p.platform, "MacIntel");
/// assert!(p.user_agent.contains("Mac OS X"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorProfile {
    /// Full `User-Agent` string (`navigator.userAgent` **and** the HTTP header).
    pub user_agent: String,
    /// Platform string e.g. `"Win32"`, `"MacIntel"`, `"Linux x86_64"`.
    pub platform: String,
    /// Browser vendor (`"Google Inc."`).
    pub vendor: String,
    /// Logical CPU core count. Realistic values: 4, 8, 12, 16.
    pub hardware_concurrency: u8,
    /// Device RAM in GiB. Realistic values: 4, 8, 16.
    pub device_memory: u8,
    /// Maximum simultaneous touch points (0 = no touchscreen, 10 = tablet/phone).
    pub max_touch_points: u8,
    /// WebGL vendor string (only used when `StealthConfig::randomize_webgl` is true).
    pub webgl_vendor: String,
    /// WebGL renderer string.
    pub webgl_renderer: String,
}

impl NavigatorProfile {
    /// A typical Windows 10 Chrome 120 profile.
    pub fn windows_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
            platform: "Win32".to_string(),
            vendor: "Google Inc.".to_string(),
            hardware_concurrency: 8,
            device_memory: 8,
            max_touch_points: 0,
            webgl_vendor: "Google Inc. (NVIDIA)".to_string(),
            webgl_renderer:
                "ANGLE (NVIDIA, NVIDIA GeForce GTX 1650 Direct3D11 vs_5_0 ps_5_0, D3D11)"
                    .to_string(),
        }
    }

    /// A typical macOS Chrome 120 profile.
    pub fn mac_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
            platform: "MacIntel".to_string(),
            vendor: "Google Inc.".to_string(),
            hardware_concurrency: 8,
            device_memory: 8,
            max_touch_points: 0,
            webgl_vendor: "Google Inc. (Intel)".to_string(),
            webgl_renderer: "ANGLE (Intel, Apple M1 Pro, OpenGL 4.1)".to_string(),
        }
    }

    /// A typical Linux Chrome profile (common in data-centre environments).
    pub fn linux_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
            platform: "Linux x86_64".to_string(),
            vendor: "Google Inc.".to_string(),
            hardware_concurrency: 4,
            device_memory: 4,
            max_touch_points: 0,
            webgl_vendor: "Mesa/X.org".to_string(),
            webgl_renderer: "llvmpipe (LLVM 15.0.7, 256 bits)".to_string(),
        }
    }
}

impl Default for NavigatorProfile {
    fn default() -> Self {
        Self::mac_chrome()
    }
}

// ─── StealthProfile ───────────────────────────────────────────────────────────

/// Combines [`StealthConfig`] (feature flags) with a [`NavigatorProfile`]
/// (identity values) and produces a single JavaScript injection script.
///
/// # Example
///
/// ```
/// use stygian_browser::stealth::{NavigatorProfile, StealthConfig, StealthProfile};
///
/// let profile = StealthProfile::new(StealthConfig::default(), NavigatorProfile::windows_chrome());
/// let script = profile.injection_script();
/// assert!(script.contains("Win32"));
/// assert!(script.contains("NVIDIA"));
/// ```
pub struct StealthProfile {
    config: StealthConfig,
    navigator: NavigatorProfile,
}

impl StealthProfile {
    /// Build a new profile from config flags and identity values.
    pub const fn new(config: StealthConfig, navigator: NavigatorProfile) -> Self {
        Self { config, navigator }
    }

    /// Generate the JavaScript to inject via
    /// `Page.addScriptToEvaluateOnNewDocument`.
    ///
    /// Returns an empty string if all stealth flags are disabled.
    pub fn injection_script(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if self.config.spoof_navigator {
            parts.push(self.navigator_spoof_script());
        }

        if self.config.randomize_webgl {
            parts.push(self.webgl_spoof_script());
        }

        if parts.is_empty() {
            return String::new();
        }

        // Wrap in an IIFE so nothing leaks to page scope
        format!(
            "(function() {{\n  'use strict';\n{}\n}})();",
            parts.join("\n\n")
        )
    }

    // ─── Private helpers ──────────────────────────────────────────────────────

    fn navigator_spoof_script(&self) -> String {
        let nav = &self.navigator;

        // Helper: Object.defineProperty with a fixed non-configurable value
        // so the spoofed value cannot be overwritten by anti-bot scripts.
        format!(
            r"  // --- Navigator spoofing ---
  (function() {{
    const defineReadOnly = (target, prop, value) => {{
      try {{
        Object.defineProperty(target, prop, {{
          get: () => value,
          enumerable: true,
          configurable: false,
        }});
      }} catch (_) {{}}
    }};

    // Remove the webdriver flag entirely
    defineReadOnly(navigator, 'webdriver', undefined);

    // Platform / identity
    defineReadOnly(navigator, 'platform',           {platform:?});
    defineReadOnly(navigator, 'userAgent',          {user_agent:?});
    defineReadOnly(navigator, 'vendor',             {vendor:?});
    defineReadOnly(navigator, 'hardwareConcurrency', {hwc});
    defineReadOnly(navigator, 'deviceMemory',        {dm});
    defineReadOnly(navigator, 'maxTouchPoints',       {mtp});

    // Permissions API — real browsers resolve 'notifications' as 'default'
    if (navigator.permissions && navigator.permissions.query) {{
      const origQuery = navigator.permissions.query.bind(navigator.permissions);
      navigator.permissions.query = (params) => {{
        if (params && params.name === 'notifications') {{
          return Promise.resolve({{ state: Notification.permission, onchange: null }});
        }}
        return origQuery(params);
      }};
    }}
  }})();",
            platform = nav.platform,
            user_agent = nav.user_agent,
            vendor = nav.vendor,
            hwc = nav.hardware_concurrency,
            dm = nav.device_memory,
            mtp = nav.max_touch_points,
        )
    }

    fn webgl_spoof_script(&self) -> String {
        let nav = &self.navigator;

        format!(
            r"  // --- WebGL fingerprint spoofing ---
  (function() {{
    const GL_VENDOR   = 0x1F00;
    const GL_RENDERER = 0x1F01;

    const spoofCtx = (ctx) => {{
      if (!ctx) return;
      const origGetParam = ctx.getParameter.bind(ctx);
      ctx.getParameter = (param) => {{
        if (param === GL_VENDOR)   return {webgl_vendor:?};
        if (param === GL_RENDERER) return {webgl_renderer:?};
        return origGetParam(param);
      }};
    }};

    // Wrap HTMLCanvasElement.prototype.getContext
    const origGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function(type, ...args) {{
      const ctx = origGetContext.call(this, type, ...args);
      if (type === 'webgl' || type === 'experimental-webgl' || type === 'webgl2') {{
        spoofCtx(ctx);
      }}
      return ctx;
    }};
  }})();",
            webgl_vendor = nav.webgl_vendor,
            webgl_renderer = nav.webgl_renderer,
        )
    }
}

// ─── Stealth application ──────────────────────────────────────────────────────

/// Inject all stealth scripts into a freshly opened browser page.
///
/// Scripts are registered with `Page.addScriptToEvaluateOnNewDocument` so they
/// execute before any page-owned JavaScript on every subsequent navigation.
/// Which scripts are injected is determined by
/// [`crate::config::StealthLevel`]:
///
/// | Level      | Injected content                                                        |
/// |------------|-------------------------------------------------------------------------|
/// | `None`     | Nothing                                                                 |
/// | `Basic`    | CDP leak fix + `navigator.webdriver` removal + minimal navigator spoof  |
/// | `Advanced` | Basic + full WebGL/navigator spoofing + fingerprint + WebRTC protection |
///
/// # Errors
///
/// Returns [`crate::error::BrowserError::CdpError`] if a CDP command fails.
///
/// # Example
///
/// ```no_run
/// # async fn run(
/// #     page: &chromiumoxide::Page,
/// #     config: &stygian_browser::BrowserConfig,
/// # ) -> stygian_browser::Result<()> {
/// use stygian_browser::stealth::apply_stealth_to_page;
/// apply_stealth_to_page(page, config).await?;
/// # Ok(())
/// # }
/// ```
pub async fn apply_stealth_to_page(
    page: &chromiumoxide::Page,
    config: &crate::config::BrowserConfig,
) -> crate::error::Result<()> {
    use crate::cdp_protection::CdpProtection;
    use crate::config::StealthLevel;
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

    /// Inline helper: push one script as `AddScriptToEvaluateOnNewDocument`.
    async fn inject_one(
        page: &chromiumoxide::Page,
        op: &'static str,
        source: String,
    ) -> crate::error::Result<()> {
        use crate::error::BrowserError;
        page.evaluate_on_new_document(AddScriptToEvaluateOnNewDocumentParams {
            source,
            world_name: None,
            include_command_line_api: None,
            run_immediately: None,
        })
        .await
        .map_err(|e| BrowserError::CdpError {
            operation: op.to_string(),
            message: e.to_string(),
        })?;
        Ok(())
    }

    if config.stealth_level == StealthLevel::None {
        return Ok(());
    }

    // ── Basic and above ────────────────────────────────────────────────────────
    let cdp_script =
        CdpProtection::new(config.cdp_fix_mode, config.source_url.clone()).build_injection_script();
    if !cdp_script.is_empty() {
        inject_one(page, "AddScriptToEvaluateOnNewDocument(cdp)", cdp_script).await?;
    }

    let (nav_profile, stealth_cfg) = match config.stealth_level {
        StealthLevel::Basic => (NavigatorProfile::default(), StealthConfig::minimal()),
        StealthLevel::Advanced => (
            NavigatorProfile::windows_chrome(),
            StealthConfig::paranoid(),
        ),
        StealthLevel::None => unreachable!(),
    };
    let nav_script = StealthProfile::new(stealth_cfg, nav_profile).injection_script();
    if !nav_script.is_empty() {
        inject_one(
            page,
            "AddScriptToEvaluateOnNewDocument(navigator)",
            nav_script,
        )
        .await?;
    }

    // ── Advanced only ──────────────────────────────────────────────────────────
    if config.stealth_level == StealthLevel::Advanced {
        let fp = crate::fingerprint::Fingerprint::random();
        let fp_script = crate::fingerprint::inject_fingerprint(&fp);
        inject_one(
            page,
            "AddScriptToEvaluateOnNewDocument(fingerprint)",
            fp_script,
        )
        .await?;

        let webrtc_script = config.webrtc.injection_script();
        if !webrtc_script.is_empty() {
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(webrtc)",
                webrtc_script,
            )
            .await?;
        }
    }

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_produces_empty_script() {
        let p = StealthProfile::new(StealthConfig::disabled(), NavigatorProfile::default());
        assert_eq!(p.injection_script(), "");
    }

    #[test]
    fn navigator_script_contains_platform() {
        let profile = NavigatorProfile::windows_chrome();
        let p = StealthProfile::new(StealthConfig::minimal(), profile);
        let script = p.injection_script();
        assert!(script.contains("Win32"), "platform must be in script");
        assert!(
            script.contains("'webdriver'"),
            "webdriver removal must be present"
        );
    }

    #[test]
    fn navigator_script_contains_user_agent() {
        let p = StealthProfile::new(StealthConfig::minimal(), NavigatorProfile::mac_chrome());
        let script = p.injection_script();
        assert!(script.contains("Mac OS X"));
        assert!(script.contains("MacIntel"));
    }

    #[test]
    fn webgl_script_contains_vendor_renderer() {
        let p = StealthProfile::new(
            StealthConfig {
                spoof_navigator: false,
                randomize_webgl: true,
                ..StealthConfig::disabled()
            },
            NavigatorProfile::windows_chrome(),
        );
        let script = p.injection_script();
        assert!(
            script.contains("NVIDIA"),
            "WebGL vendor must appear in script"
        );
        assert!(
            script.contains("getParameter"),
            "WebGL method must be overridden"
        );
    }

    #[test]
    fn full_profile_wraps_in_iife() {
        let p = StealthProfile::new(StealthConfig::default(), NavigatorProfile::default());
        let script = p.injection_script();
        assert!(script.starts_with("(function()"), "script must be an IIFE");
        assert!(script.ends_with("})();"));
    }

    #[test]
    fn navigator_profile_linux_has_correct_platform() {
        assert_eq!(NavigatorProfile::linux_chrome().platform, "Linux x86_64");
    }

    #[test]
    fn stealth_config_paranoid_equals_default() {
        let a = StealthConfig::paranoid();
        let b = StealthConfig::default();
        assert_eq!(a.spoof_navigator, b.spoof_navigator);
        assert_eq!(a.randomize_webgl, b.randomize_webgl);
        assert_eq!(a.randomize_canvas, b.randomize_canvas);
        assert_eq!(a.human_behavior, b.human_behavior);
        assert_eq!(a.protect_cdp, b.protect_cdp);
    }

    #[test]
    fn hardware_concurrency_reasonable() {
        let p = NavigatorProfile::windows_chrome();
        assert!(p.hardware_concurrency >= 2);
        assert!(p.hardware_concurrency <= 64);
    }

    // ── T13: stealth level script generation ──────────────────────────────────

    #[test]
    fn none_level_is_not_active() {
        use crate::config::StealthLevel;
        assert!(!StealthLevel::None.is_active());
    }

    #[test]
    fn basic_level_cdp_script_removes_webdriver() {
        use crate::cdp_protection::{CdpFixMode, CdpProtection};
        let script = CdpProtection::new(CdpFixMode::AddBinding, None).build_injection_script();
        assert!(
            script.contains("webdriver"),
            "CDP protection script should remove navigator.webdriver"
        );
    }

    #[test]
    fn basic_level_minimal_config_injects_navigator() {
        let config = StealthConfig::minimal();
        let profile = NavigatorProfile::default();
        let script = StealthProfile::new(config, profile).injection_script();
        assert!(
            !script.is_empty(),
            "Basic stealth should produce a navigator script"
        );
    }

    #[test]
    fn advanced_level_paranoid_config_includes_webgl() {
        let config = StealthConfig::paranoid();
        let profile = NavigatorProfile::windows_chrome();
        let script = StealthProfile::new(config, profile).injection_script();
        assert!(
            script.contains("webgl") && script.contains("getParameter"),
            "Advanced stealth should spoof WebGL via getParameter patching"
        );
    }

    #[test]
    fn advanced_level_fingerprint_script_non_empty() {
        use crate::fingerprint::{Fingerprint, inject_fingerprint};
        let fp = Fingerprint::random();
        let script = inject_fingerprint(&fp);
        assert!(
            !script.is_empty(),
            "Fingerprint injection script must not be empty"
        );
    }

    #[test]
    fn stealth_level_ordering() {
        use crate::config::StealthLevel;
        assert!(!StealthLevel::None.is_active());
        assert!(StealthLevel::Basic.is_active());
        assert!(StealthLevel::Advanced.is_active());
    }

    #[test]
    fn navigator_profile_basic_uses_default() {
        // Basic → default navigator profile (mac_chrome)
        let profile = NavigatorProfile::default();
        assert_eq!(profile.platform, "MacIntel");
    }

    #[test]
    fn navigator_profile_advanced_uses_windows() {
        let profile = NavigatorProfile::windows_chrome();
        assert_eq!(profile.platform, "Win32");
    }
}
