//! Browser fingerprint generation and JavaScript injection.
//!
//! Generates realistic, randomised browser fingerprints and produces JavaScript
//! strings suitable for `Page.addScriptToEvaluateOnNewDocument` so every new
//! page context starts with a consistent, spoofed identity.
//!
//! # Example
//!
//! ```
//! use stygian_browser::fingerprint::{Fingerprint, inject_fingerprint};
//!
//! let fp = Fingerprint::random();
//! let script = inject_fingerprint(&fp);
//! assert!(!script.is_empty());
//! assert!(script.contains("screen"));
//! ```

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

// ── curated value pools ──────────────────────────────────────────────────────

const SCREEN_RESOLUTIONS: &[(u32, u32)] = &[
    (1920, 1080),
    (2560, 1440),
    (1440, 900),
    (1366, 768),
    (1536, 864),
    (1280, 800),
    (2560, 1600),
    (1680, 1050),
];

const TIMEZONES: &[&str] = &[
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "Europe/London",
    "Europe/Paris",
    "Europe/Berlin",
    "Asia/Tokyo",
    "Asia/Shanghai",
    "Australia/Sydney",
];

const LANGUAGES: &[&str] = &[
    "en-US", "en-GB", "en-AU", "en-CA", "fr-FR", "de-DE", "es-ES", "it-IT", "pt-BR", "ja-JP",
    "zh-CN",
];

const HARDWARE_CONCURRENCY: &[u32] = &[4, 8, 12, 16];
const DEVICE_MEMORY: &[u32] = &[4, 8, 16];

/// (vendor, renderer) pairs that correspond to real GPU configurations.
const WEBGL_PROFILES: &[(&str, &str, &str)] = &[
    ("Intel Inc.", "Intel Iris OpenGL Engine", "MacIntel"),
    ("Intel Inc.", "Intel UHD Graphics 630", "MacIntel"),
    (
        "Google Inc. (Apple)",
        "ANGLE (Apple, Apple M2, OpenGL 4.1)",
        "MacIntel",
    ),
    (
        "Google Inc. (NVIDIA)",
        "ANGLE (NVIDIA, NVIDIA GeForce RTX 3080, OpenGL 4.1)",
        "Win32",
    ),
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) UHD Graphics 770, OpenGL 4.6)",
        "Win32",
    ),
    (
        "Google Inc. (AMD)",
        "ANGLE (AMD, AMD Radeon RX 6800 XT Direct3D11 vs_5_0 ps_5_0)",
        "Win32",
    ),
];

// Windows-only GPU pool (2-tuple; no platform tag needed)
const WINDOWS_WEBGL_PROFILES: &[(&str, &str)] = &[
    (
        "Google Inc. (NVIDIA)",
        "ANGLE (NVIDIA, NVIDIA GeForce RTX 3080, OpenGL 4.1)",
    ),
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) UHD Graphics 770, OpenGL 4.6)",
    ),
    (
        "Google Inc. (AMD)",
        "ANGLE (AMD, AMD Radeon RX 6800 XT Direct3D11 vs_5_0 ps_5_0)",
    ),
];

// macOS-only GPU pool
const MACOS_WEBGL_PROFILES: &[(&str, &str)] = &[
    ("Intel Inc.", "Intel Iris OpenGL Engine"),
    ("Intel Inc.", "Intel UHD Graphics 630"),
    ("Google Inc. (Apple)", "ANGLE (Apple, Apple M2, OpenGL 4.1)"),
];

// Mobile screen resolution pools
const MOBILE_ANDROID_RESOLUTIONS: &[(u32, u32)] =
    &[(393, 851), (390, 844), (412, 915), (414, 896), (360, 780)];

const MOBILE_IOS_RESOLUTIONS: &[(u32, u32)] =
    &[(390, 844), (393, 852), (375, 667), (414, 896), (428, 926)];

// Mobile GPU pools
const ANDROID_WEBGL_PROFILES: &[(&str, &str)] = &[
    ("Qualcomm", "Adreno (TM) 730"),
    ("ARM", "Mali-G710 MC10"),
    (
        "Google Inc. (Qualcomm)",
        "ANGLE (Qualcomm, Adreno (TM) 730, OpenGL ES 3.2)",
    ),
    ("Google Inc. (ARM)", "ANGLE (ARM, Mali-G610, OpenGL ES 3.2)"),
];

const IOS_WEBGL_PROFILES: &[(&str, &str)] = &[
    ("Apple Inc.", "Apple A16 GPU"),
    ("Apple Inc.", "Apple A15 GPU"),
    ("Apple Inc.", "Apple A14 GPU"),
    ("Apple Inc.", "Apple M1"),
];

// System font pools representative of each OS
const WINDOWS_FONTS: &[&str] = &[
    "Arial",
    "Calibri",
    "Cambria",
    "Comic Sans MS",
    "Consolas",
    "Courier New",
    "Georgia",
    "Impact",
    "Segoe UI",
    "Tahoma",
    "Times New Roman",
    "Trebuchet MS",
    "Verdana",
];

const MACOS_FONTS: &[&str] = &[
    "Arial",
    "Avenir",
    "Baskerville",
    "Courier New",
    "Futura",
    "Georgia",
    "Helvetica Neue",
    "Lucida Grande",
    "Optima",
    "Palatino",
    "Times New Roman",
    "Verdana",
];

const LINUX_FONTS: &[&str] = &[
    "Arial",
    "DejaVu Sans",
    "DejaVu Serif",
    "FreeMono",
    "Liberation Mono",
    "Liberation Sans",
    "Liberation Serif",
    "Times New Roman",
    "Ubuntu",
];

const MOBILE_ANDROID_FONTS: &[&str] = &[
    "Roboto",
    "Noto Sans",
    "Droid Sans",
    "sans-serif",
    "serif",
    "monospace",
];

const MOBILE_IOS_FONTS: &[&str] = &[
    "Helvetica Neue",
    "Arial",
    "Georgia",
    "Times New Roman",
    "Courier New",
];

// Browser version pools
const CHROME_VERSIONS: &[u32] = &[120, 121, 122, 123, 124, 125];
const EDGE_VERSIONS: &[u32] = &[120, 121, 122, 123, 124];
const FIREFOX_VERSIONS: &[u32] = &[121, 122, 123, 124, 125, 126];
const SAFARI_VERSIONS: &[&str] = &["17.0", "17.1", "17.2", "17.3", "17.4"];
const IOS_OS_VERSIONS: &[&str] = &["16_6", "17_0", "17_1", "17_2", "17_3"];

// ── entropy helpers ──────────────────────────────────────────────────────────

/// Splitmix64-style hash — mixes `seed` with a `step` multiplier so every
/// call with a unique `step` produces an independent random-looking value.
const fn rng(seed: u64, step: u64) -> u64 {
    let x = seed.wrapping_add(step.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick<T: Copy + Default>(items: &[T], entropy: u64) -> T {
    let idx = usize::try_from(entropy).unwrap_or(usize::MAX) % items.len().max(1);
    items.get(idx).copied().unwrap_or_default()
}

// ── public types ─────────────────────────────────────────────────────────────

/// A complete browser fingerprint used to make each session look unique.
///
/// # Example
///
/// ```
/// use stygian_browser::fingerprint::Fingerprint;
///
/// let fp = Fingerprint::random();
/// let (w, h) = fp.screen_resolution;
/// assert!(w > 0 && h > 0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Full user-agent string.
    pub user_agent: String,

    /// Physical screen resolution `(width, height)` in pixels.
    pub screen_resolution: (u32, u32),

    /// IANA timezone identifier, e.g. `"America/New_York"`.
    pub timezone: String,

    /// BCP 47 primary language tag, e.g. `"en-US"`.
    pub language: String,

    /// Navigator platform string, e.g. `"MacIntel"` or `"Win32"`.
    pub platform: String,

    /// Logical CPU core count reported to JavaScript.
    pub hardware_concurrency: u32,

    /// Device memory in GiB reported to JavaScript.
    pub device_memory: u32,

    /// WebGL `GL_VENDOR` string.
    pub webgl_vendor: Option<String>,

    /// WebGL `GL_RENDERER` string.
    pub webgl_renderer: Option<String>,

    /// Whether to inject imperceptible canvas pixel noise.
    pub canvas_noise: bool,

    /// System fonts available on this device.
    ///
    /// Populated by [`Fingerprint::from_device_profile`]. Empty when created
    /// via [`Fingerprint::random`] or `Default`.
    pub fonts: Vec<String>,
}

impl Default for Fingerprint {
    fn default() -> Self {
        Self {
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                         AppleWebKit/537.36 (KHTML, like Gecko) \
                         Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
            screen_resolution: (1920, 1080),
            timezone: "America/New_York".to_string(),
            language: "en-US".to_string(),
            platform: "MacIntel".to_string(),
            hardware_concurrency: 8,
            device_memory: 8,
            webgl_vendor: Some("Intel Inc.".to_string()),
            webgl_renderer: Some("Intel Iris OpenGL Engine".to_string()),
            canvas_noise: true,
            fonts: vec![],
        }
    }
}

impl Fingerprint {
    /// Generate a realistic randomised fingerprint.
    ///
    /// Values are selected from curated pools representative of real-world
    /// browser distributions.  Each call uses sub-second system entropy so
    /// consecutive calls within the same second may differ.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::Fingerprint;
    ///
    /// let fp = Fingerprint::random();
    /// assert!(fp.hardware_concurrency > 0);
    /// assert!(fp.device_memory > 0);
    /// ```
    pub fn random() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x5a5a_5a5a_5a5a_5a5a, |d| {
                d.as_secs() ^ u64::from(d.subsec_nanos())
            });

        let res = pick(SCREEN_RESOLUTIONS, rng(seed, 1));
        let tz = pick(TIMEZONES, rng(seed, 2));
        let lang = pick(LANGUAGES, rng(seed, 3));
        let hw = pick(HARDWARE_CONCURRENCY, rng(seed, 4));
        let dm = pick(DEVICE_MEMORY, rng(seed, 5));
        let (wv, wr, platform) = pick(WEBGL_PROFILES, rng(seed, 6));

        let user_agent = if platform == "Win32" {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/120.0.0.0 Safari/537.36"
                .to_string()
        } else {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/120.0.0.0 Safari/537.36"
                .to_string()
        };

        let fonts: Vec<String> = if platform == "Win32" {
            WINDOWS_FONTS.iter().map(|s| (*s).to_string()).collect()
        } else {
            MACOS_FONTS.iter().map(|s| (*s).to_string()).collect()
        };

        Self {
            user_agent,
            screen_resolution: res,
            timezone: tz.to_string(),
            language: lang.to_string(),
            platform: platform.to_string(),
            hardware_concurrency: hw,
            device_memory: dm,
            webgl_vendor: Some(wv.to_string()),
            webgl_renderer: Some(wr.to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    /// Clone a fingerprint from a [`FingerprintProfile`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::{Fingerprint, FingerprintProfile};
    ///
    /// let profile = FingerprintProfile::new("test".to_string());
    /// let fp = Fingerprint::from_profile(&profile);
    /// assert!(!fp.user_agent.is_empty());
    /// ```
    pub fn from_profile(profile: &FingerprintProfile) -> Self {
        profile.fingerprint.clone()
    }

    /// Generate a fingerprint consistent with a specific [`DeviceProfile`].
    ///
    /// All properties — user agent, platform, GPU, fonts — are internally
    /// consistent.  A Mac profile will never carry a Windows GPU, for example.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::{Fingerprint, DeviceProfile};
    ///
    /// let fp = Fingerprint::from_device_profile(DeviceProfile::DesktopMac, 42);
    /// assert_eq!(fp.platform, "MacIntel");
    /// assert!(!fp.fonts.is_empty());
    /// ```
    pub fn from_device_profile(device: DeviceProfile, seed: u64) -> Self {
        match device {
            DeviceProfile::DesktopWindows => Self::for_windows(seed),
            DeviceProfile::DesktopMac => Self::for_mac(seed),
            DeviceProfile::DesktopLinux => Self::for_linux(seed),
            DeviceProfile::MobileAndroid => Self::for_android(seed),
            DeviceProfile::MobileIOS => Self::for_ios(seed),
        }
    }

    /// Check that all fingerprint fields are internally consistent.
    ///
    /// Returns a `Vec<String>` of human-readable inconsistency descriptions.
    /// An empty vec means the fingerprint passes every check.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::Fingerprint;
    ///
    /// let fp = Fingerprint::default();
    /// assert!(fp.validate_consistency().is_empty());
    /// ```
    pub fn validate_consistency(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // UA / platform cross-check
        if self.platform == "Win32" && self.user_agent.contains("Mac OS X") {
            issues.push("Win32 platform but user-agent says Mac OS X".to_string());
        }
        if self.platform == "MacIntel" && self.user_agent.contains("Windows NT") {
            issues.push("MacIntel platform but user-agent says Windows NT".to_string());
        }
        if self.platform.starts_with("Linux") && self.user_agent.contains("Windows NT") {
            issues.push("Linux platform but user-agent says Windows NT".to_string());
        }

        // WebGL vendor / platform cross-check
        if let Some(vendor) = &self.webgl_vendor {
            if (self.platform == "Win32" || self.platform == "MacIntel")
                && (vendor.contains("Qualcomm")
                    || vendor.contains("Adreno")
                    || vendor.contains("Mali"))
            {
                issues.push(format!(
                    "Desktop platform '{}' has mobile GPU vendor '{vendor}'",
                    self.platform
                ));
            }
            if self.platform == "Win32" && vendor.starts_with("Apple") {
                issues.push(format!("Win32 platform has Apple GPU vendor '{vendor}'"));
            }
        }

        // Font / platform cross-check (only when fonts are populated)
        if !self.fonts.is_empty() {
            let has_win_exclusive = self
                .fonts
                .iter()
                .any(|f| matches!(f.as_str(), "Segoe UI" | "Calibri" | "Consolas" | "Tahoma"));
            let has_mac_exclusive = self.fonts.iter().any(|f| {
                matches!(
                    f.as_str(),
                    "Lucida Grande" | "Avenir" | "Optima" | "Futura" | "Baskerville"
                )
            });
            let has_linux_exclusive = self.fonts.iter().any(|f| {
                matches!(
                    f.as_str(),
                    "DejaVu Sans" | "Liberation Sans" | "Ubuntu" | "FreeMono"
                )
            });

            if self.platform == "MacIntel" && has_win_exclusive {
                issues.push("MacIntel platform has Windows-exclusive fonts".to_string());
            }
            if self.platform == "Win32" && has_mac_exclusive {
                issues.push("Win32 platform has macOS-exclusive fonts".to_string());
            }
            if self.platform == "Win32" && has_linux_exclusive {
                issues.push("Win32 platform has Linux-exclusive fonts".to_string());
            }
        }

        issues
    }

    // ── Private per-OS fingerprint builders ───────────────────────────────────

    fn for_windows(seed: u64) -> Self {
        let browser = BrowserKind::for_device(DeviceProfile::DesktopWindows, seed);
        let user_agent = match browser {
            BrowserKind::Chrome | BrowserKind::Safari => {
                let ver = pick(CHROME_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                     AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/{ver}.0.0.0 Safari/537.36"
                )
            }
            BrowserKind::Edge => {
                let ver = pick(EDGE_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                     AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/{ver}.0.0.0 Safari/537.36 Edg/{ver}.0.0.0"
                )
            }
            BrowserKind::Firefox => {
                let ver = pick(FIREFOX_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{ver}.0) \
                     Gecko/20100101 Firefox/{ver}.0"
                )
            }
        };

        let (webgl_vendor, webgl_renderer) = pick(WINDOWS_WEBGL_PROFILES, rng(seed, 7));
        let fonts = WINDOWS_FONTS.iter().map(|s| (*s).to_string()).collect();

        Self {
            user_agent,
            screen_resolution: pick(SCREEN_RESOLUTIONS, rng(seed, 1)),
            timezone: pick(TIMEZONES, rng(seed, 2)).to_string(),
            language: pick(LANGUAGES, rng(seed, 3)).to_string(),
            platform: "Win32".to_string(),
            hardware_concurrency: pick(HARDWARE_CONCURRENCY, rng(seed, 4)),
            device_memory: pick(DEVICE_MEMORY, rng(seed, 5)),
            webgl_vendor: Some(webgl_vendor.to_string()),
            webgl_renderer: Some(webgl_renderer.to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    fn for_mac(seed: u64) -> Self {
        let browser = BrowserKind::for_device(DeviceProfile::DesktopMac, seed);
        let user_agent = match browser {
            BrowserKind::Chrome | BrowserKind::Edge => {
                let ver = pick(CHROME_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                     AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/{ver}.0.0.0 Safari/537.36"
                )
            }
            BrowserKind::Safari => {
                let ver = pick(SAFARI_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                     AppleWebKit/605.1.15 (KHTML, like Gecko) \
                     Version/{ver} Safari/605.1.15"
                )
            }
            BrowserKind::Firefox => {
                let ver = pick(FIREFOX_VERSIONS, rng(seed, 10));
                format!(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:{ver}.0) \
                     Gecko/20100101 Firefox/{ver}.0"
                )
            }
        };

        let (webgl_vendor, webgl_renderer) = pick(MACOS_WEBGL_PROFILES, rng(seed, 7));
        let fonts = MACOS_FONTS.iter().map(|s| (*s).to_string()).collect();

        Self {
            user_agent,
            screen_resolution: pick(SCREEN_RESOLUTIONS, rng(seed, 1)),
            timezone: pick(TIMEZONES, rng(seed, 2)).to_string(),
            language: pick(LANGUAGES, rng(seed, 3)).to_string(),
            platform: "MacIntel".to_string(),
            hardware_concurrency: pick(HARDWARE_CONCURRENCY, rng(seed, 4)),
            device_memory: pick(DEVICE_MEMORY, rng(seed, 5)),
            webgl_vendor: Some(webgl_vendor.to_string()),
            webgl_renderer: Some(webgl_renderer.to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    fn for_linux(seed: u64) -> Self {
        let browser = BrowserKind::for_device(DeviceProfile::DesktopLinux, seed);
        let user_agent = if browser == BrowserKind::Firefox {
            let ver = pick(FIREFOX_VERSIONS, rng(seed, 10));
            format!(
                "Mozilla/5.0 (X11; Linux x86_64; rv:{ver}.0) \
                 Gecko/20100101 Firefox/{ver}.0"
            )
        } else {
            let ver = pick(CHROME_VERSIONS, rng(seed, 10));
            format!(
                "Mozilla/5.0 (X11; Linux x86_64) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/{ver}.0.0.0 Safari/537.36"
            )
        };

        let fonts = LINUX_FONTS.iter().map(|s| (*s).to_string()).collect();

        Self {
            user_agent,
            screen_resolution: pick(SCREEN_RESOLUTIONS, rng(seed, 1)),
            timezone: pick(TIMEZONES, rng(seed, 2)).to_string(),
            language: pick(LANGUAGES, rng(seed, 3)).to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: pick(HARDWARE_CONCURRENCY, rng(seed, 4)),
            device_memory: pick(DEVICE_MEMORY, rng(seed, 5)),
            webgl_vendor: Some("Mesa/X.org".to_string()),
            webgl_renderer: Some("llvmpipe (LLVM 15.0.7, 256 bits)".to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    fn for_android(seed: u64) -> Self {
        let browser = BrowserKind::for_device(DeviceProfile::MobileAndroid, seed);
        let user_agent = if browser == BrowserKind::Firefox {
            let ver = pick(FIREFOX_VERSIONS, rng(seed, 10));
            format!(
                "Mozilla/5.0 (Android 14; Mobile; rv:{ver}.0) \
                 Gecko/20100101 Firefox/{ver}.0"
            )
        } else {
            let ver = pick(CHROME_VERSIONS, rng(seed, 10));
            format!(
                "Mozilla/5.0 (Linux; Android 14; Pixel 7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/{ver}.0.6099.144 Mobile Safari/537.36"
            )
        };

        let (webgl_vendor, webgl_renderer) = pick(ANDROID_WEBGL_PROFILES, rng(seed, 6));
        let fonts = MOBILE_ANDROID_FONTS
            .iter()
            .map(|s| (*s).to_string())
            .collect();

        Self {
            user_agent,
            screen_resolution: pick(MOBILE_ANDROID_RESOLUTIONS, rng(seed, 1)),
            timezone: pick(TIMEZONES, rng(seed, 2)).to_string(),
            language: pick(LANGUAGES, rng(seed, 3)).to_string(),
            platform: "Linux armv8l".to_string(),
            hardware_concurrency: pick(HARDWARE_CONCURRENCY, rng(seed, 4)),
            device_memory: pick(DEVICE_MEMORY, rng(seed, 5)),
            webgl_vendor: Some(webgl_vendor.to_string()),
            webgl_renderer: Some(webgl_renderer.to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    fn for_ios(seed: u64) -> Self {
        let safari_ver = pick(SAFARI_VERSIONS, rng(seed, 10));
        let ios_ver = pick(IOS_OS_VERSIONS, rng(seed, 11));
        let user_agent = format!(
            "Mozilla/5.0 (iPhone; CPU iPhone OS {ios_ver} like Mac OS X) \
             AppleWebKit/605.1.15 (KHTML, like Gecko) \
             Version/{safari_ver} Mobile/15E148 Safari/604.1"
        );

        let (webgl_vendor, webgl_renderer) = pick(IOS_WEBGL_PROFILES, rng(seed, 6));
        let fonts = MOBILE_IOS_FONTS.iter().map(|s| (*s).to_string()).collect();

        Self {
            user_agent,
            screen_resolution: pick(MOBILE_IOS_RESOLUTIONS, rng(seed, 1)),
            timezone: pick(TIMEZONES, rng(seed, 2)).to_string(),
            language: pick(LANGUAGES, rng(seed, 3)).to_string(),
            platform: "iPhone".to_string(),
            hardware_concurrency: 6,
            device_memory: 4,
            webgl_vendor: Some(webgl_vendor.to_string()),
            webgl_renderer: Some(webgl_renderer.to_string()),
            canvas_noise: true,
            fonts,
        }
    }

    /// Produce a JavaScript IIFE that spoofs browser fingerprint APIs.
    ///
    /// The returned script is intended to be passed to the CDP command
    /// `Page.addScriptToEvaluateOnNewDocument` so it runs before page JS.
    ///
    /// Covers: screen dimensions, timezone, language, hardware concurrency,
    /// device memory, WebGL parameters, canvas noise, and audio fingerprint
    /// defence.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::Fingerprint;
    ///
    /// let fp = Fingerprint::default();
    /// let script = fp.injection_script();
    /// assert!(script.contains("1920"));
    /// assert!(script.contains("screen"));
    /// ```
    pub fn injection_script(&self) -> String {
        let mut parts = vec![
            screen_script(self.screen_resolution),
            timezone_script(&self.timezone),
            language_script(&self.language, &self.user_agent),
            hardware_script(self.hardware_concurrency, self.device_memory),
        ];

        if let (Some(vendor), Some(renderer)) = (&self.webgl_vendor, &self.webgl_renderer) {
            parts.push(webgl_script(vendor, renderer));
        }

        if self.canvas_noise {
            parts.push(canvas_noise_script());
        }

        parts.push(audio_fingerprint_script());
        parts.push(connection_spoof_script());
        parts.push(font_measurement_intercept_script());
        parts.push(storage_estimate_spoof_script());
        parts.push(battery_spoof_script());
        parts.push(plugins_spoof_script());

        format!("(function() {{\n{}\n}})();", parts.join("\n\n"))
    }
}

/// A named, reusable fingerprint identity.
///
/// # Example
///
/// ```
/// use stygian_browser::fingerprint::FingerprintProfile;
///
/// let profile = FingerprintProfile::new("my-session".to_string());
/// assert_eq!(profile.name, "my-session");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintProfile {
    /// Human-readable profile name.
    pub name: String,

    /// The fingerprint data for this profile.
    pub fingerprint: Fingerprint,
}

impl FingerprintProfile {
    /// Create a new profile with a freshly randomised fingerprint.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::FingerprintProfile;
    ///
    /// let p = FingerprintProfile::new("bot-1".to_string());
    /// assert!(!p.fingerprint.user_agent.is_empty());
    /// ```
    pub fn new(name: String) -> Self {
        Self {
            name,
            fingerprint: Fingerprint::random(),
        }
    }

    /// Create a new profile whose fingerprint is weighted by real-world market share.
    ///
    /// Device type (Windows/macOS/Linux) is selected via
    /// [`DeviceProfile::random_weighted`], then a fully consistent fingerprint
    /// is generated for that device.  The resulting fingerprint is guaranteed
    /// to pass [`Fingerprint::validate_consistency`].
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::FingerprintProfile;
    ///
    /// let profile = FingerprintProfile::random_weighted("session-1".to_string());
    /// assert!(!profile.fingerprint.fonts.is_empty());
    /// assert!(profile.fingerprint.validate_consistency().is_empty());
    /// ```
    pub fn random_weighted(name: String) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x5a5a_5a5a_5a5a_5a5a, |d| {
                d.as_secs() ^ u64::from(d.subsec_nanos())
            });

        let device = DeviceProfile::random_weighted(seed);
        Self {
            name,
            fingerprint: Fingerprint::from_device_profile(device, seed),
        }
    }
}

// ── public helper ────────────────────────────────────────────────────────────

/// Return a JavaScript injection script for `fingerprint`.
///
/// Equivalent to calling [`Fingerprint::injection_script`] directly; provided
/// as a standalone function for convenient use without importing the type.
///
/// The script should be passed to `Page.addScriptToEvaluateOnNewDocument`.
///
/// # Example
///
/// ```
/// use stygian_browser::fingerprint::{Fingerprint, inject_fingerprint};
///
/// let fp = Fingerprint::default();
/// let script = inject_fingerprint(&fp);
/// assert!(script.starts_with("(function()"));
/// ```
pub fn inject_fingerprint(fingerprint: &Fingerprint) -> String {
    fingerprint.injection_script()
}

// ── Device profile types ─────────────────────────────────────────────────────

/// Device profile type for consistent fingerprint generation.
///
/// Determines the OS, platform string, GPU pool, and font set used when
/// building a fingerprint via [`Fingerprint::from_device_profile`].
///
/// # Example
///
/// ```
/// use stygian_browser::fingerprint::DeviceProfile;
///
/// let profile = DeviceProfile::random_weighted(12345);
/// assert!(!profile.is_mobile());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeviceProfile {
    /// Windows 10/11 desktop (≈70% of desktop market share).
    #[default]
    DesktopWindows,
    /// macOS desktop (≈20% of desktop market share).
    DesktopMac,
    /// Linux desktop (≈10% of desktop market share).
    DesktopLinux,
    /// Android mobile device.
    MobileAndroid,
    /// iOS mobile device (iPhone/iPad).
    MobileIOS,
}

impl DeviceProfile {
    /// Select a device profile weighted by real-world desktop market share.
    ///
    /// Distribution: Windows 70%, macOS 20%, Linux 10%.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::DeviceProfile;
    ///
    /// // Most seeds produce DesktopWindows (70% weight).
    /// let profile = DeviceProfile::random_weighted(0);
    /// assert_eq!(profile, DeviceProfile::DesktopWindows);
    /// ```
    pub const fn random_weighted(seed: u64) -> Self {
        let v = rng(seed, 97) % 100;
        match v {
            0..=69 => Self::DesktopWindows,
            70..=89 => Self::DesktopMac,
            _ => Self::DesktopLinux,
        }
    }

    /// Returns `true` for mobile device profiles (Android or iOS).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::DeviceProfile;
    ///
    /// assert!(DeviceProfile::MobileAndroid.is_mobile());
    /// assert!(!DeviceProfile::DesktopWindows.is_mobile());
    /// ```
    pub const fn is_mobile(self) -> bool {
        matches!(self, Self::MobileAndroid | Self::MobileIOS)
    }
}

/// Browser kind for user-agent string generation.
///
/// Used internally by [`Fingerprint::from_device_profile`] to construct
/// realistic user-agent strings consistent with the selected device.
///
/// # Example
///
/// ```
/// use stygian_browser::fingerprint::{BrowserKind, DeviceProfile};
///
/// let kind = BrowserKind::for_device(DeviceProfile::MobileIOS, 42);
/// assert_eq!(kind, BrowserKind::Safari);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BrowserKind {
    /// Google Chrome — most common desktop browser.
    #[default]
    Chrome,
    /// Microsoft Edge — Chromium-based, Windows-primary.
    Edge,
    /// Apple Safari — macOS/iOS only.
    Safari,
    /// Mozilla Firefox.
    Firefox,
}

impl BrowserKind {
    /// Select a browser weighted by market share for the given device profile.
    ///
    /// - iOS always returns [`BrowserKind::Safari`] (`WebKit` required).
    /// - macOS: Chrome 56%, Safari 36%, Firefox 8%.
    /// - Android: Chrome 90%, Firefox 10%.
    /// - Windows/Linux: Chrome 65%, Edge 16%, Firefox 19%.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::fingerprint::{BrowserKind, DeviceProfile};
    ///
    /// let kind = BrowserKind::for_device(DeviceProfile::MobileIOS, 0);
    /// assert_eq!(kind, BrowserKind::Safari);
    /// ```
    pub const fn for_device(device: DeviceProfile, seed: u64) -> Self {
        match device {
            DeviceProfile::MobileIOS => Self::Safari,
            DeviceProfile::MobileAndroid => {
                let v = rng(seed, 201) % 100;
                if v < 90 { Self::Chrome } else { Self::Firefox }
            }
            DeviceProfile::DesktopMac => {
                let v = rng(seed, 201) % 100;
                match v {
                    0..=55 => Self::Chrome,
                    56..=91 => Self::Safari,
                    _ => Self::Firefox,
                }
            }
            _ => {
                // Windows / Linux
                let v = rng(seed, 201) % 100;
                match v {
                    0..=64 => Self::Chrome,
                    65..=80 => Self::Edge,
                    _ => Self::Firefox,
                }
            }
        }
    }
}

// ── JavaScript generation helpers ────────────────────────────────────────────

fn screen_script((width, height): (u32, u32)) -> String {
    // availHeight leaves ~40 px for a taskbar / dock.
    let avail_height = height.saturating_sub(40);
    // availLeft/availTop: on Windows the taskbar is usually at the bottom so
    // both are 0. Spoofing to 0 matches the most common real-device value and
    // avoids the headless default which Turnstile checks explicitly.
    format!(
        r"  // Screen dimensions
  const _defineScreen = (prop, val) =>
    Object.defineProperty(screen, prop, {{ get: () => val, configurable: false }});
  _defineScreen('width',       {width});
  _defineScreen('height',      {height});
  _defineScreen('availWidth',  {width});
  _defineScreen('availHeight', {avail_height});
  _defineScreen('availLeft',   0);
  _defineScreen('availTop',    0);
  _defineScreen('colorDepth',  24);
  _defineScreen('pixelDepth',  24);
  // outerWidth/outerHeight: headless Chrome may return 0; spoof to viewport size.
  try {{
    Object.defineProperty(window, 'outerWidth',  {{ get: () => {width},  configurable: true }});
    Object.defineProperty(window, 'outerHeight', {{ get: () => {height}, configurable: true }});
  }} catch(_) {{}}"
    )
}

fn timezone_script(timezone: &str) -> String {
    format!(
        r"  // Timezone via Intl.DateTimeFormat
  const _origResolvedOptions = Intl.DateTimeFormat.prototype.resolvedOptions;
  Intl.DateTimeFormat.prototype.resolvedOptions = function() {{
    const opts = _origResolvedOptions.apply(this, arguments);
    opts.timeZone = {timezone:?};
    return opts;
  }};"
    )
}

fn language_script(language: &str, user_agent: &str) -> String {
    // Build a plausible accept-languages list from the primary tag.
    let primary = language.split('-').next().unwrap_or("en");
    format!(
        r"  // Language + userAgent
  Object.defineProperty(navigator, 'language',   {{ get: () => {language:?}, configurable: false }});
  Object.defineProperty(navigator, 'languages',  {{ get: () => Object.freeze([{language:?}, {primary:?}]), configurable: false }});
  Object.defineProperty(navigator, 'userAgent',  {{ get: () => {user_agent:?}, configurable: false }});"
    )
}

fn hardware_script(concurrency: u32, memory: u32) -> String {
    format!(
        r"  // Hardware concurrency + device memory
  Object.defineProperty(navigator, 'hardwareConcurrency', {{ get: () => {concurrency}, configurable: false }});
  Object.defineProperty(navigator, 'deviceMemory',        {{ get: () => {memory}, configurable: false }});"
    )
}

fn webgl_script(vendor: &str, renderer: &str) -> String {
    format!(
        r"  // WebGL vendor + renderer
  (function() {{
    const _getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function(type, attrs) {{
      const ctx = _getContext.call(this, type, attrs);
      if (!ctx) return ctx;
      if (type === 'webgl' || type === 'webgl2' || type === 'experimental-webgl') {{
        const _getParam = ctx.getParameter.bind(ctx);
        ctx.getParameter = function(param) {{
          if (param === 0x1F00) return {vendor:?};    // GL_VENDOR
          if (param === 0x1F01) return {renderer:?};  // GL_RENDERER
          return _getParam(param);
        }};
      }}
      return ctx;
    }};
  }})();"
    )
}

fn canvas_noise_script() -> String {
    r"  // Canvas noise: flip lowest bit of R/G/B channels to defeat pixel readback
  (function() {
    const _getImageData = CanvasRenderingContext2D.prototype.getImageData;
    CanvasRenderingContext2D.prototype.getImageData = function() {
      const id = _getImageData.apply(this, arguments);
      const d  = id.data;
      for (let i = 0; i < d.length; i += 4) {
        d[i]     ^= 1;
        d[i + 1] ^= 1;
        d[i + 2] ^= 1;
      }
      return id;
    };
  })();"
        .to_string()
}

fn audio_fingerprint_script() -> String {
    r"  // Audio fingerprint defence: add sub-epsilon noise to frequency data
  (function() {
    if (typeof AnalyserNode === 'undefined') return;
    const _getFloatFreq = AnalyserNode.prototype.getFloatFrequencyData;
    AnalyserNode.prototype.getFloatFrequencyData = function(arr) {
      _getFloatFreq.apply(this, arguments);
      for (let i = 0; i < arr.length; i++) {
        arr[i] += (Math.random() - 0.5) * 1e-7;
      }
    };
  })();"
        .to_string()
}

/// Spoof the `NetworkInformation` API (`navigator.connection`) so headless sessions
/// report a realistic `WiFi` connection rather than the undefined/zero-RTT default
/// that Akamai Bot Manager v3 uses as a headless indicator.
fn connection_spoof_script() -> String {
    // _seed is 0–996; derived from performance.timeOrigin (epoch ms with sub-ms
    // precision) so the RTT/downlink values vary realistically across sessions.
    r"  // NetworkInformation API spoof (navigator.connection)
  (function() {
    const _seed = Math.floor(performance.timeOrigin % 997);
    const conn = {
      rtt:           50 + _seed % 100,
      downlink:      5 + _seed % 15,
      effectiveType: '4g',
      type:          'wifi',
      saveData:      false,
      onchange:      null,
      ontypechange:  null,
      addEventListener:    function() {},
      removeEventListener: function() {},
      dispatchEvent:       function() { return true; },
    };
    try {
      Object.defineProperty(navigator, 'connection', {
        get: () => conn,
        enumerable: true,
        configurable: false,
      });
    } catch (_) {}
  })();"
        .to_string()
}

/// Intercept `getBoundingClientRect` on hidden font-probe elements.
///
/// Turnstile creates a hidden `<div>`, renders a string, and measures
/// `getBoundingClientRect` to verify the font physically renders.  In headless
/// Chrome the sandbox may not have the font installed, so the browser returns a
/// zero-size rect.  This intercept detects zero-size rects on elements that are
/// hidden (`visibility:hidden` or `aria-hidden`) with absolute/fixed positioning
/// (the canonical font-probe pattern) and returns plausible non-zero dimensions
/// drawn from a seeded deterministic jitter so the same call always returns the
/// same value within a session.
fn font_measurement_intercept_script() -> String {
    r"  // getBoundingClientRect font-probe intercept (Turnstile Layer 1)
  (function() {
    const _origGBCR = Element.prototype.getBoundingClientRect;
    const _seed = Math.floor(performance.timeOrigin % 9973);
    function _jitter(base, range) {
      return base + ((_seed * 1103515245 + 12345) & 0x7fffffff) % range;
    }
    Element.prototype.getBoundingClientRect = function() {
      const rect = _origGBCR.call(this);
      // Only intercept zero-size rects on hidden probe elements (the font-
      // measurement pattern: position absolute/fixed, visibility hidden).
      if (rect.width === 0 && rect.height === 0) {
        const st = window.getComputedStyle(this);
        const vis = st.getPropertyValue('visibility');
        const pos = st.getPropertyValue('position');
        const ariaHidden = this.getAttribute('aria-hidden');
        if ((vis === 'hidden' || ariaHidden === 'true') &&
            (pos === 'absolute' || pos === 'fixed')) {
          const w = _jitter(10, 8);
          const h = _jitter(14, 4);
          return new DOMRect(0, 0, w, h);
        }
      }
      return rect;
    };
  })();"
        .to_string()
}

/// Spoof `navigator.storage.estimate()` to return realistic quota/usage values.
///
/// Headless Chrome returns a very low `quota` (typically 60–120 MB) vs a real
/// browser profile which accumulates gigabytes of quota.  Turnstile explicitly
/// reads `quota` and `usage` to compare against expected real-profile ranges.
fn storage_estimate_spoof_script() -> String {
    r"  // navigator.storage.estimate() spoof (Turnstile Layer 1 — storage)
  (function() {
    if (!navigator.storage || typeof navigator.storage.estimate !== 'function') return;
    const _origEstimate = navigator.storage.estimate.bind(navigator.storage);
    const _seed = Math.floor(performance.timeOrigin % 9973);
    // Realistic Chrome profile: ~250 GB quota, small stable usage.
    const quota = (240 + _seed % 20) * 1073741824;
    const usage = (5  + _seed % 10) * 1048576;
    navigator.storage.estimate = function() {
      return _origEstimate().then(function(result) {
                return Object.assign({}, result, {
                    quota: quota,
                    usage: usage
                });
      });
    };
  })();"
        .to_string()
}

/// Normalize `navigator.getBattery()` away from the suspicious fully-charged
/// headless default (`level: 1.0`, `charging: true`) that many bot detectors
/// flag.  Resolves to a realistic mid-charge, discharging state.
/// Spoof `navigator.plugins` and `navigator.mimeTypes`.
///
/// An empty `PluginArray` (length 0) is the single most-flagged headless
/// indicator on services like pixelscan.net and Akamai Bot Manager.  Real
/// Chrome always exposes at least the built-in PDF Viewer plugin.
fn plugins_spoof_script() -> String {
    r"  // navigator.plugins / mimeTypes — empty array = instant headless flag
  (function() {
    // Build minimal objects that survive instanceof checks.
    var mime0 = { type: 'application/pdf', description: 'Portable Document Format', suffixes: 'pdf', enabledPlugin: null };
    var mime1 = { type: 'text/pdf',        description: 'Portable Document Format', suffixes: 'pdf', enabledPlugin: null };
    var pdfPlugin = {
      name: 'PDF Viewer',
      description: 'Portable Document Format',
      filename: 'internal-pdf-viewer',
      length: 2,
      0: mime0, 1: mime1,
      item: function(i) { return [mime0, mime1][i] || null; },
      namedItem: function(n) {
        if (n === 'application/pdf') return mime0;
        if (n === 'text/pdf')        return mime1;
        return null;
      },
    };
    mime0.enabledPlugin = pdfPlugin;
    mime1.enabledPlugin = pdfPlugin;

    var fakePlugins = {
      length: 1,
      0: pdfPlugin,
      item: function(i) { return i === 0 ? pdfPlugin : null; },
      namedItem: function(n) { return n === 'PDF Viewer' ? pdfPlugin : null; },
      refresh: function() {},
    };
    var fakeMimes = {
      length: 2,
      0: mime0, 1: mime1,
      item: function(i) { return [mime0, mime1][i] || null; },
      namedItem: function(n) {
        if (n === 'application/pdf') return mime0;
        if (n === 'text/pdf')        return mime1;
        return null;
      },
    };

    try {
      Object.defineProperty(navigator, 'plugins',   { get: function() { return fakePlugins; }, configurable: false });
      Object.defineProperty(navigator, 'mimeTypes', { get: function() { return fakeMimes; },   configurable: false });
    } catch(_) {}
  })();"
        .to_string()
}

fn battery_spoof_script() -> String {
    r"  // Battery API normalization (navigator.getBattery)
  (function() {
    if (typeof navigator.getBattery !== 'function') return;
    const _seed = Math.floor(performance.timeOrigin % 997);
    const battery = {
      charging:        false,
      chargingTime:    Infinity,
      dischargingTime: 3600 + _seed * 7,
      level:           0.65 + (_seed % 30) / 100,
      onchargingchange:        null,
      onchargingtimechange:    null,
      ondischargingtimechange: null,
      onlevelchange:           null,
      addEventListener:    function() {},
      removeEventListener: function() {},
      dispatchEvent:       function() { return true; },
    };
    navigator.getBattery = function() {
      return Promise.resolve(battery);
    };
  })();"
        .to_string()
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_fingerprint_has_valid_ranges() {
        let fp = Fingerprint::random();
        let (w, h) = fp.screen_resolution;
        assert!(
            (1280..=3840).contains(&w),
            "width {w} out of expected range"
        );
        assert!(
            (768..=2160).contains(&h),
            "height {h} out of expected range"
        );
        assert!(
            HARDWARE_CONCURRENCY.contains(&fp.hardware_concurrency),
            "hardware_concurrency {} not in pool",
            fp.hardware_concurrency
        );
        assert!(
            DEVICE_MEMORY.contains(&fp.device_memory),
            "device_memory {} not in pool",
            fp.device_memory
        );
        assert!(
            TIMEZONES.contains(&fp.timezone.as_str()),
            "timezone {} not in pool",
            fp.timezone
        );
        assert!(
            LANGUAGES.contains(&fp.language.as_str()),
            "language {} not in pool",
            fp.language
        );
    }

    #[test]
    fn random_generates_different_values_over_time() {
        // Two calls should eventually differ across seeds; at minimum the
        // function must not panic and must return valid data.
        let fp1 = Fingerprint::random();
        let fp2 = Fingerprint::random();
        // Both are well-formed whether or not they happen to be equal.
        assert!(!fp1.user_agent.is_empty());
        assert!(!fp2.user_agent.is_empty());
    }

    #[test]
    fn injection_script_contains_screen_dimensions() {
        let fp = Fingerprint {
            screen_resolution: (2560, 1440),
            ..Fingerprint::default()
        };
        let script = fp.injection_script();
        assert!(script.contains("2560"), "missing width in script");
        assert!(script.contains("1440"), "missing height in script");
    }

    #[test]
    fn injection_script_contains_timezone() {
        let fp = Fingerprint {
            timezone: "Europe/Berlin".to_string(),
            ..Fingerprint::default()
        };
        let script = fp.injection_script();
        assert!(script.contains("Europe/Berlin"), "timezone missing");
    }

    #[test]
    fn injection_script_contains_canvas_noise_when_enabled() {
        let fp = Fingerprint {
            canvas_noise: true,
            ..Fingerprint::default()
        };
        let script = fp.injection_script();
        assert!(
            script.contains("getImageData"),
            "canvas noise block missing"
        );
    }

    #[test]
    fn injection_script_omits_canvas_noise_when_disabled() {
        let fp = Fingerprint {
            canvas_noise: false,
            ..Fingerprint::default()
        };
        let script = fp.injection_script();
        assert!(
            !script.contains("getImageData"),
            "canvas noise should be absent"
        );
    }

    #[test]
    fn injection_script_contains_webgl_vendor() {
        let fp = Fingerprint {
            webgl_vendor: Some("TestVendor".to_string()),
            webgl_renderer: Some("TestRenderer".to_string()),
            canvas_noise: false,
            ..Fingerprint::default()
        };
        let script = fp.injection_script();
        assert!(script.contains("TestVendor"), "WebGL vendor missing");
        assert!(script.contains("TestRenderer"), "WebGL renderer missing");
    }

    #[test]
    fn inject_fingerprint_fn_equals_method() {
        let fp = Fingerprint::default();
        assert_eq!(inject_fingerprint(&fp), fp.injection_script());
    }

    #[test]
    fn from_profile_returns_profile_fingerprint() {
        let profile = FingerprintProfile::new("test".to_string());
        let fp = Fingerprint::from_profile(&profile);
        assert_eq!(fp.user_agent, profile.fingerprint.user_agent);
    }

    #[test]
    fn script_is_wrapped_in_iife() {
        let script = Fingerprint::default().injection_script();
        assert!(script.starts_with("(function()"), "must start with IIFE");
        assert!(script.ends_with("})();"), "must end with IIFE call");
    }

    #[test]
    fn rng_produces_distinct_values_for_different_steps() {
        let seed = 0xdead_beef_cafe_babe_u64;
        let v1 = rng(seed, 1);
        let v2 = rng(seed, 2);
        let v3 = rng(seed, 3);
        assert_ne!(v1, v2);
        assert_ne!(v2, v3);
    }

    // ── T08 — DeviceProfile / BrowserKind / from_device_profile tests ─────────

    #[test]
    fn device_profile_windows_is_consistent() {
        let fp = Fingerprint::from_device_profile(DeviceProfile::DesktopWindows, 42);
        assert_eq!(fp.platform, "Win32");
        assert!(fp.user_agent.contains("Windows NT"), "UA must be Windows");
        assert!(!fp.fonts.is_empty(), "Windows profile must have fonts");
        assert!(
            fp.validate_consistency().is_empty(),
            "must pass consistency"
        );
    }

    #[test]
    fn device_profile_mac_is_consistent() {
        let fp = Fingerprint::from_device_profile(DeviceProfile::DesktopMac, 42);
        assert_eq!(fp.platform, "MacIntel");
        assert!(
            fp.user_agent.contains("Mac OS X"),
            "UA must be macOS: {}",
            fp.user_agent
        );
        assert!(!fp.fonts.is_empty(), "Mac profile must have fonts");
        assert!(
            fp.validate_consistency().is_empty(),
            "must pass consistency"
        );
    }

    #[test]
    fn device_profile_linux_is_consistent() {
        let fp = Fingerprint::from_device_profile(DeviceProfile::DesktopLinux, 42);
        assert_eq!(fp.platform, "Linux x86_64");
        assert!(fp.user_agent.contains("Linux"), "UA must be Linux");
        assert!(!fp.fonts.is_empty(), "Linux profile must have fonts");
        assert!(
            fp.validate_consistency().is_empty(),
            "must pass consistency"
        );
    }

    #[test]
    fn device_profile_android_is_mobile() {
        let fp = Fingerprint::from_device_profile(DeviceProfile::MobileAndroid, 42);
        assert!(
            fp.platform.starts_with("Linux"),
            "Android platform should be Linux-based"
        );
        assert!(
            fp.user_agent.contains("Android") || fp.user_agent.contains("Firefox"),
            "Android UA mismatch: {}",
            fp.user_agent
        );
        assert!(!fp.fonts.is_empty());
        assert!(DeviceProfile::MobileAndroid.is_mobile());
    }

    #[test]
    fn device_profile_ios_is_mobile() {
        let fp = Fingerprint::from_device_profile(DeviceProfile::MobileIOS, 42);
        assert_eq!(fp.platform, "iPhone");
        assert!(
            fp.user_agent.contains("iPhone"),
            "iOS UA must contain iPhone"
        );
        assert!(!fp.fonts.is_empty());
        assert!(DeviceProfile::MobileIOS.is_mobile());
    }

    #[test]
    fn desktop_profiles_are_not_mobile() {
        assert!(!DeviceProfile::DesktopWindows.is_mobile());
        assert!(!DeviceProfile::DesktopMac.is_mobile());
        assert!(!DeviceProfile::DesktopLinux.is_mobile());
    }

    #[test]
    fn browser_kind_ios_always_safari() {
        for seed in [0u64, 1, 42, 999, u64::MAX] {
            assert_eq!(
                BrowserKind::for_device(DeviceProfile::MobileIOS, seed),
                BrowserKind::Safari,
                "iOS must always return Safari (seed={seed})"
            );
        }
    }

    #[test]
    fn device_profile_random_weighted_distribution() {
        // Run 1000 samples and verify Windows dominates (expect ≥50%)
        let windows_count = (0u64..1000)
            .filter(|&i| {
                DeviceProfile::random_weighted(i * 13 + 7) == DeviceProfile::DesktopWindows
            })
            .count();
        assert!(
            windows_count >= 500,
            "Expected ≥50% Windows, got {windows_count}/1000"
        );
    }

    #[test]
    fn validate_consistency_catches_platform_ua_mismatch() {
        let fp = Fingerprint {
            platform: "Win32".to_string(),
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                         AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
            ..Fingerprint::default()
        };
        let issues = fp.validate_consistency();
        assert!(!issues.is_empty(), "should detect Win32+Mac UA mismatch");
    }

    #[test]
    fn validate_consistency_catches_platform_font_mismatch() {
        let fp = Fingerprint {
            platform: "MacIntel".to_string(),
            fonts: vec!["Segoe UI".to_string(), "Calibri".to_string()],
            ..Fingerprint::default()
        };
        let issues = fp.validate_consistency();
        assert!(
            !issues.is_empty(),
            "should detect MacIntel + Windows fonts mismatch"
        );
    }

    #[test]
    fn validate_consistency_passes_for_default() {
        let fp = Fingerprint::default();
        assert!(fp.validate_consistency().is_empty());
    }

    #[test]
    fn fingerprint_profile_random_weighted_has_fonts() {
        let profile = FingerprintProfile::random_weighted("sess-1".to_string());
        assert_eq!(profile.name, "sess-1");
        assert!(!profile.fingerprint.fonts.is_empty());
        assert!(profile.fingerprint.validate_consistency().is_empty());
    }

    #[test]
    fn from_device_profile_serializes_to_json() -> Result<(), Box<dyn std::error::Error>> {
        let fp = Fingerprint::from_device_profile(DeviceProfile::DesktopWindows, 123);
        let json = serde_json::to_string(&fp)?;
        let back: Fingerprint = serde_json::from_str(&json)?;
        assert_eq!(back.platform, fp.platform);
        assert_eq!(back.fonts, fp.fonts);
        Ok(())
    }

    // ─── Property-based tests (proptest) ──────────────────────────────────────

    proptest::proptest! {
        /// For any seed, a device-profile fingerprint must pass `validate_consistency()`.
        #[test]
        fn prop_seeded_fingerprint_always_consistent(seed in 0u64..10_000) {
            let profile = DeviceProfile::random_weighted(seed);
            let fp = Fingerprint::from_device_profile(profile, seed);
            let issues = fp.validate_consistency();
            proptest::prop_assert!(
                issues.is_empty(),
                "validate_consistency() failed for seed {seed}: {issues:?}"
            );
        }

        /// Hardware concurrency must always be in [1, 32].
        #[test]
        fn prop_hardware_concurrency_is_sensible(_seed in 0u64..10_000) {
            let fp = Fingerprint::random();
            proptest::prop_assert!(
                fp.hardware_concurrency >= 1 && fp.hardware_concurrency <= 32,
                "hardware_concurrency {} out of [1,32]", fp.hardware_concurrency
            );
        }

        /// Device memory must be in the valid JS set {4, 8, 16} (gb as reported to JS).
        #[test]
        fn prop_device_memory_is_valid_value(_seed in 0u64..10_000) {
            let fp = Fingerprint::random();
            let valid: &[u32] = &[4, 8, 16];
            proptest::prop_assert!(
                valid.contains(&fp.device_memory),
                "device_memory {} is not a valid value", fp.device_memory
            );
        }

        /// Screen dimensions must be plausible for a real monitor.
        #[test]
        fn prop_screen_dimensions_are_plausible(_seed in 0u64..10_000) {
            let fp = Fingerprint::random();
            let (w, h) = fp.screen_resolution;
            proptest::prop_assert!((320..=7680).contains(&w));
            proptest::prop_assert!((240..=4320).contains(&h));
        }

        /// FingerprintProfile::random_weighted must always pass consistency.
        #[test]
        fn prop_fingerprint_profile_passes_consistency(name in "[a-z][a-z0-9]{0,31}") {
            let profile = FingerprintProfile::random_weighted(name.clone());
            let issues = profile.fingerprint.validate_consistency();
            proptest::prop_assert!(
                issues.is_empty(),
                "FingerprintProfile for '{name}' has issues: {issues:?}"
            );
        }

        /// Injection script is always non-empty and mentions navigator.
        #[test]
        fn prop_injection_script_non_empty(_seed in 0u64..10_000) {
            let fp = Fingerprint::random();
            let script = inject_fingerprint(&fp);
            proptest::prop_assert!(!script.is_empty());
            proptest::prop_assert!(script.contains("navigator"));
        }
    }
}
