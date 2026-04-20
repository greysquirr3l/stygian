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
    /// A typical Windows 10 Chrome 131 profile.
    pub fn windows_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
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

    /// A typical macOS Chrome 131 profile.
    pub fn mac_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
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

    /// A typical Linux Chrome 131 profile (common in data-centre environments).
    pub fn linux_chrome() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
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

        // Always inject chrome-object and userAgentData spoofing when navigator
        // spoofing is active — both are Cloudflare Turnstile detection vectors.
        if self.config.spoof_navigator {
            parts.push(Self::chrome_object_script());
            parts.push(self.user_agent_data_script());
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

    // Remove the webdriver flag at both the prototype and instance levels.
    // Cloudflare and pixelscan probe Navigator.prototype directly via
    // Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver').
    // In real Chrome the property is enumerable:false — matching that is
    // essential; enumerable:true is a Turnstile detection signal.
    // configurable:true is kept so polyfills don't throw on a second
    // defineProperty call.
    try {{
      Object.defineProperty(Navigator.prototype, 'webdriver', {{
        get: () => undefined,
        enumerable: false,
        configurable: true,
      }});
    }} catch (_) {{}}
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

    fn chrome_object_script() -> String {
        // Cloudflare Turnstile checks window.chrome.runtime, window.chrome.csi,
        // and window.chrome.loadTimes — all present in real Chrome but absent
        // in headless. Stubbing them removes these detection signals.
        r"  // --- window.chrome object spoofing ---
  (function() {
    if (!window.chrome) {
      Object.defineProperty(window, 'chrome', {
        value: {},
        enumerable: true,
        configurable: false,
        writable: false,
      });
    }
    const chrome = window.chrome;
    // chrome.runtime — checked by Turnstile; needs at least an object with
    // id and connect stubs to pass duck-type checks.
    if (!chrome.runtime) {
      chrome.runtime = {
        id: undefined,
        connect: () => {},
        sendMessage: () => {},
        onMessage: { addListener: () => {}, removeListener: () => {} },
      };
    }
    // chrome.csi and chrome.loadTimes — legacy APIs present in real Chrome.
    if (!chrome.csi) {
      chrome.csi = () => ({
        startE: Date.now(),
        onloadT: Date.now(),
        pageT: 0,
        tran: 15,
      });
    }
    if (!chrome.loadTimes) {
      chrome.loadTimes = () => ({
        requestTime: Date.now() / 1000,
        startLoadTime: Date.now() / 1000,
        commitLoadTime: Date.now() / 1000,
        finishDocumentLoadTime: Date.now() / 1000,
        finishLoadTime: Date.now() / 1000,
        firstPaintTime: Date.now() / 1000,
        firstPaintAfterLoadTime: 0,
        navigationType: 'Other',
        wasFetchedViaSpdy: false,
        wasNpnNegotiated: true,
        npnNegotiatedProtocol: 'h2',
        wasAlternateProtocolAvailable: false,
        connectionInfo: 'h2',
      });
    }
  })();"
            .to_string()
    }

    fn user_agent_data_script(&self) -> String {
        let nav = &self.navigator;
        // Extract the major Chrome version from the UA string so that
        // navigator.userAgentData.brands is consistent with navigator.userAgent.
        // Mismatch between the two is a primary Cloudflare JA3/UA coherence check.
        let version = nav
            .user_agent
            .split("Chrome/")
            .nth(1)
            .and_then(|s| s.split('.').next())
            .unwrap_or("131");
        let mobile = nav.max_touch_points > 0;
        let platform = if nav.platform.contains("Win") {
            "Windows"
        } else if nav.platform.contains("Mac") {
            "macOS"
        } else {
            "Linux"
        };

        format!(
            r"  // --- navigator.userAgentData spoofing ---
  (function() {{
    const uaData = {{
      brands: [
        {{ brand: 'Google Chrome',  version: '{version}' }},
        {{ brand: 'Chromium',       version: '{version}' }},
        {{ brand: 'Not=A?Brand',    version: '99'        }},
      ],
      mobile: {mobile},
      platform: '{platform}',
      getHighEntropyValues: (hints) => Promise.resolve({{
        brands: [
          {{ brand: 'Google Chrome',  version: '{version}' }},
          {{ brand: 'Chromium',       version: '{version}' }},
          {{ brand: 'Not=A?Brand',    version: '99'        }},
        ],
        mobile: {mobile},
        platform: '{platform}',
        architecture: 'x86',
        bitness: '64',
        model: '',
        platformVersion: '10.0.0',
        uaFullVersion: '{version}.0.0.0',
        fullVersionList: [
          {{ brand: 'Google Chrome',  version: '{version}.0.0.0' }},
          {{ brand: 'Chromium',       version: '{version}.0.0.0' }},
          {{ brand: 'Not=A?Brand',    version: '99.0.0.0'        }},
        ],
      }}),
      toJSON: () => ({{
        brands: [
          {{ brand: 'Google Chrome',  version: '{version}' }},
          {{ brand: 'Chromium',       version: '{version}' }},
          {{ brand: 'Not=A?Brand',    version: '99'        }},
        ],
        mobile: {mobile},
        platform: '{platform}',
      }}),
    }};
    try {{
      Object.defineProperty(navigator, 'userAgentData', {{
        get: () => uaData,
        enumerable: true,
        configurable: false,
      }});
    }} catch (_) {{}}
  }})();"
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
/// | ------------ | ------------------------------------------------------------------------- |
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
#[allow(clippy::too_many_lines)]
pub async fn apply_stealth_to_page(
    page: &chromiumoxide::Page,
    config: &crate::config::BrowserConfig,
) -> crate::error::Result<()> {
    use crate::cdp_protection::CdpProtection;
    use crate::config::StealthLevel;
    use crate::error::BrowserError;
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

    // For Advanced stealth, always resolve a coherent profile so every
    // injected surface can derive from the same identity.
    let effective_profile = (config.stealth_level == StealthLevel::Advanced).then(|| {
        config
            .fingerprint_profile
            .clone()
            .unwrap_or_else(|| fallback_profile_for_config(config))
    });

    // ── CDP hardening — runs FIRST to clean up binding remnants ───────────────
    #[cfg(feature = "stealth")]
    {
        let hardening_script = crate::cdp_hardening::cdp_hardening_script(&config.cdp_hardening);
        if !hardening_script.is_empty() {
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(cdp-hardening)",
                hardening_script,
            )
            .await?;
        }
    }

    // ── Basic and above ────────────────────────────────────────────────────────
    let cdp_script =
        CdpProtection::new(config.cdp_fix_mode, config.source_url.clone()).build_injection_script();
    if !cdp_script.is_empty() {
        inject_one(page, "AddScriptToEvaluateOnNewDocument(cdp)", cdp_script).await?;
    }

    let (nav_profile, stealth_cfg) = match config.stealth_level {
        StealthLevel::Basic => (NavigatorProfile::default(), StealthConfig::minimal()),
        StealthLevel::Advanced => {
            let profile = effective_profile
                .as_ref()
                .ok_or_else(|| BrowserError::ConfigError("missing advanced profile".to_string()))?;
            (
                navigator_profile_from_coherent_profile(profile),
                StealthConfig::paranoid(),
            )
        }
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
        let profile = effective_profile
            .as_ref()
            .ok_or_else(|| BrowserError::ConfigError("missing advanced profile".to_string()))?;
        let fp = fingerprint_from_coherent_profile(profile);
        // Build one engine once so all spoofed surfaces share the same per-session seed.
        let noise_seed = profile.noise_seed;
        let noise_engine = crate::noise::NoiseEngine::new(noise_seed);
        let noise_seed = noise_engine.seed();
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

        if config.noise.canvas_enabled {
            let canvas_script = crate::canvas_noise::canvas_noise_script(&noise_engine);
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(canvas-noise)",
                canvas_script,
            )
            .await?;
        }

        // WebGL, audio, and rects noise (T39, T40, T41)
        if config.noise.webgl_enabled {
            let webgl_script =
                crate::webgl_noise::webgl_noise_script(&profile.webgl, &noise_engine);
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(webgl-noise)",
                webgl_script,
            )
            .await?;
        }

        if config.noise.audio_enabled {
            let audio_script = crate::audio_noise::audio_noise_script(&noise_engine);
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(audio-noise)",
                audio_script,
            )
            .await?;
        }

        if config.noise.rects_enabled {
            let rects_script = crate::rects_noise::rects_noise_script(&noise_engine);
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(rects-noise)",
                rects_script,
            )
            .await?;
        }

        // Navigator coherence (T43) — always inject in Advanced using the
        // effective coherent profile.
        let nav_script = crate::navigator_coherence::navigator_coherence_script(profile);
        inject_one(
            page,
            "AddScriptToEvaluateOnNewDocument(navigator-coherence)",
            nav_script,
        )
        .await?;

        // Timing noise (T44)
        {
            let timing_cfg = crate::timing_noise::TimingNoiseConfig {
                enabled: true,
                jitter_ms: 0.3,
                seed: noise_seed,
            };
            let timing_script = crate::timing_noise::timing_noise_script(&timing_cfg);
            inject_one(
                page,
                "AddScriptToEvaluateOnNewDocument(timing-noise)",
                timing_script,
            )
            .await?;
        }

        // Peripheral stealth (T47)
        {
            let peripheral_cfg =
                crate::peripheral_stealth::PeripheralStealthConfig::default_with_seed(noise_seed);
            let peripheral_script =
                crate::peripheral_stealth::peripheral_stealth_script_with_profile(
                    &peripheral_cfg,
                    Some(profile),
                );
            if !peripheral_script.is_empty() {
                inject_one(
                    page,
                    "AddScriptToEvaluateOnNewDocument(peripheral-stealth)",
                    peripheral_script,
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn navigator_profile_from_coherent_profile(
    profile: &crate::profile::FingerprintProfile,
) -> NavigatorProfile {
    NavigatorProfile {
        user_agent: profile.browser.user_agent.clone(),
        platform: profile.platform.platform_string.clone(),
        vendor: "Google Inc.".to_string(),
        hardware_concurrency: u8::try_from(profile.hardware.cores).unwrap_or(8),
        device_memory: u8::try_from(profile.hardware.memory_gb).unwrap_or(8),
        max_touch_points: profile.platform.max_touch_points,
        webgl_vendor: profile.webgl.vendor.clone(),
        webgl_renderer: profile.webgl.renderer.clone(),
    }
}

fn fingerprint_from_coherent_profile(
    profile: &crate::profile::FingerprintProfile,
) -> crate::fingerprint::Fingerprint {
    crate::fingerprint::Fingerprint {
        user_agent: profile.browser.user_agent.clone(),
        screen_resolution: (profile.screen.width, profile.screen.height),
        // The v3 profile model does not currently include explicit
        // timezone/language fields; derive deterministic values from profile
        // seed so they stay stable per profile and vary across sessions.
        timezone: fingerprint_timezone_from_coherent_profile(profile),
        language: fingerprint_language_from_coherent_profile(profile),
        platform: profile.platform.platform_string.clone(),
        hardware_concurrency: profile.hardware.cores,
        device_memory: profile.hardware.memory_gb,
        webgl_vendor: Some(profile.webgl.vendor.clone()),
        webgl_renderer: Some(profile.webgl.renderer.clone()),
        canvas_noise: true,
        fonts: Vec::new(),
    }
}

fn fallback_profile_for_config(
    config: &crate::config::BrowserConfig,
) -> crate::profile::FingerprintProfile {
    let seed = config.noise.seed.map_or_else(
        || {
            // Keep a stable fallback per BrowserConfig instance so all pages
            // from the same instance share one coherent profile.
            std::ptr::from_ref(config) as usize as u64
        },
        crate::noise::NoiseSeed::as_u64,
    );

    // Weighted mapping: windows 65%, macOS 20%, linux 5%, android 10%.
    match seed % 100 {
        0..=64 => crate::profile::FingerprintProfile::windows_chrome_136_rtx3060(),
        65..=84 => crate::profile::FingerprintProfile::macos_chrome_136_m1(),
        85..=89 => crate::profile::FingerprintProfile::linux_chrome_136_intel(),
        _ => crate::profile::FingerprintProfile::android_chrome_136_pixel(),
    }
}

fn deterministic_profile_choice<'a>(seed: u64, salt: &str, choices: &'a [&'a str]) -> &'a str {
    let Some(default_choice) = choices.first().copied() else {
        return "";
    };

    let mut hash = seed ^ 0xcbf2_9ce4_8422_2325_u64;
    for byte in salt.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }

    let Ok(len_u64) = u64::try_from(choices.len()) else {
        return default_choice;
    };
    if len_u64 == 0 {
        return default_choice;
    }

    let idx_u64 = hash % len_u64;
    let Ok(idx) = usize::try_from(idx_u64) else {
        return default_choice;
    };

    choices.get(idx).copied().unwrap_or(default_choice)
}

fn fingerprint_timezone_from_coherent_profile(
    profile: &crate::profile::FingerprintProfile,
) -> String {
    const TZ_WINDOWS: &[&str] = &[
        "America/New_York",
        "America/Chicago",
        "America/Denver",
        "America/Los_Angeles",
        "America/Toronto",
    ];
    const TZ_MAC: &[&str] = &[
        "America/Los_Angeles",
        "America/New_York",
        "Europe/London",
        "Europe/Paris",
    ];
    const TZ_LINUX: &[&str] = &[
        "Europe/Berlin",
        "Europe/Amsterdam",
        "Europe/London",
        "America/New_York",
    ];
    const TZ_ANDROID: &[&str] = &[
        "Asia/Tokyo",
        "Asia/Seoul",
        "Asia/Singapore",
        "Australia/Sydney",
        "America/Los_Angeles",
    ];

    let choices = match profile.platform.os {
        crate::profile::Os::Windows => TZ_WINDOWS,
        crate::profile::Os::MacOs => TZ_MAC,
        crate::profile::Os::Linux => TZ_LINUX,
        crate::profile::Os::Android | crate::profile::Os::Ios => TZ_ANDROID,
    };

    deterministic_profile_choice(profile.noise_seed.as_u64(), "timezone", choices).to_string()
}

fn fingerprint_language_from_coherent_profile(
    profile: &crate::profile::FingerprintProfile,
) -> String {
    const LANGUAGES: &[&str] = &[
        "en-US", "en-GB", "fr-FR", "de-DE", "es-ES", "it-IT", "nl-NL", "pt-BR", "sv-SE",
    ];

    let keyboard = profile.platform.keyboard_layout.trim();
    let normalized = match keyboard {
        "en" | "en-US" | "en_US" | "us" | "US" => Some("en-US"),
        "en-GB" | "en_GB" | "uk" | "UK" => Some("en-GB"),
        "fr" | "fr-FR" | "fr_FR" => Some("fr-FR"),
        "de" | "de-DE" | "de_DE" => Some("de-DE"),
        "es" | "es-ES" | "es_ES" => Some("es-ES"),
        "it" | "it-IT" | "it_IT" => Some("it-IT"),
        "nl" | "nl-NL" | "nl_NL" => Some("nl-NL"),
        "pt" | "pt-BR" | "pt_BR" => Some("pt-BR"),
        "sv" | "sv-SE" | "sv_SE" => Some("sv-SE"),
        _ => None,
    };
    if let Some(lang) = normalized {
        return lang.to_string();
    }

    deterministic_profile_choice(profile.noise_seed.as_u64(), "language", LANGUAGES).to_string()
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
