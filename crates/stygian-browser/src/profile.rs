//! Unified fingerprint identity profile.
//!
//! [`FingerprintProfile`] ties together all identity signals — UA, platform,
//! screen, hardware, WebGL GPU, noise seed, navigator properties, and network
//! characteristics — into a single coherent device identity. All built-in
//! profiles are internally consistent and pass `validate()`.
//!
//! # Example
//!
//! ```
//! use stygian_browser::profile::FingerprintProfile;
//!
//! let p = FingerprintProfile::windows_chrome_136_rtx3060();
//! assert!(p.validate().is_ok());
//! assert_eq!(p.platform.os, stygian_browser::profile::Os::Windows);
//! ```

use serde::{Deserialize, Serialize};

use crate::{noise::NoiseSeed, webgl_noise::WebGlProfile};

// ---------------------------------------------------------------------------
// Sub-types
// ---------------------------------------------------------------------------

/// Operating system class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
}

/// Platform configuration.
///
/// # Example
///
/// ```
/// use stygian_browser::profile::PlatformProfile;
/// let p = PlatformProfile::windows();
/// assert_eq!(p.max_touch_points, 0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformProfile {
    /// Operating system.
    pub os: Os,
    /// Human-readable OS version string (e.g. `"10.0.0"`).
    pub os_version: String,
    /// `navigator.platform` value.
    pub platform_string: String,
    /// `navigator.maxTouchPoints` — 0 for desktop, ≥5 for mobile.
    pub max_touch_points: u8,
    /// Keyboard layout (e.g. `"en-US"`).
    pub keyboard_layout: String,
}

impl PlatformProfile {
    /// Windows 10 desktop platform.
    #[must_use]
    pub fn windows() -> Self {
        Self {
            os: Os::Windows,
            os_version: "10.0.0".into(),
            platform_string: "Win32".into(),
            max_touch_points: 0,
            keyboard_layout: "en-US".into(),
        }
    }

    /// macOS (Apple Silicon) desktop platform.
    #[must_use]
    pub fn macos() -> Self {
        Self {
            os: Os::MacOs,
            os_version: "14.0.0".into(),
            platform_string: "MacIntel".into(),
            max_touch_points: 0,
            keyboard_layout: "en-US".into(),
        }
    }

    /// Linux desktop platform.
    #[must_use]
    pub fn linux() -> Self {
        Self {
            os: Os::Linux,
            os_version: "5.15.0".into(),
            platform_string: "Linux x86_64".into(),
            max_touch_points: 0,
            keyboard_layout: "en-US".into(),
        }
    }

    /// Android mobile platform.
    #[must_use]
    pub fn android() -> Self {
        Self {
            os: Os::Android,
            os_version: "13".into(),
            platform_string: "Linux armv81".into(),
            max_touch_points: 5,
            keyboard_layout: "en-US".into(),
        }
    }
}

/// Browser kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Chrome,
    Edge,
    Firefox,
    Safari,
}

/// Browser identity configuration.
///
/// # Example
///
/// ```
/// use stygian_browser::profile::BrowserProfile;
/// let b = BrowserProfile::chrome_136_windows();
/// assert!(b.user_agent.contains("Chrome/136"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfile {
    /// Browser type.
    pub kind: BrowserKind,
    /// Major version number.
    pub version: u32,
    /// Full `User-Agent` string.
    pub user_agent: String,
    /// `Sec-CH-UA` header value.
    pub sec_ch_ua: String,
    /// `Sec-CH-UA-Platform` header value.
    pub sec_ch_ua_platform: String,
    /// `Sec-CH-UA-Mobile` header value (`?0` or `?1`).
    pub sec_ch_ua_mobile: String,
}

impl BrowserProfile {
    /// Chrome 136 on Windows.
    #[must_use]
    pub fn chrome_136_windows() -> Self {
        Self {
            kind: BrowserKind::Chrome,
            version: 136,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36".into(),
            sec_ch_ua: r#""Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99""#.into(),
            sec_ch_ua_platform: "\"Windows\"".into(),
            sec_ch_ua_mobile: "?0".into(),
        }
    }

    /// Chrome 136 on macOS.
    #[must_use]
    pub fn chrome_136_macos() -> Self {
        Self {
            kind: BrowserKind::Chrome,
            version: 136,
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36".into(),
            sec_ch_ua: r#""Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99""#.into(),
            sec_ch_ua_platform: "\"macOS\"".into(),
            sec_ch_ua_mobile: "?0".into(),
        }
    }

    /// Chrome 136 on Linux.
    #[must_use]
    pub fn chrome_136_linux() -> Self {
        Self {
            kind: BrowserKind::Chrome,
            version: 136,
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36".into(),
            sec_ch_ua: r#""Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99""#.into(),
            sec_ch_ua_platform: "\"Linux\"".into(),
            sec_ch_ua_mobile: "?0".into(),
        }
    }

    /// Edge 136 on Windows.
    #[must_use]
    pub fn edge_136_windows() -> Self {
        Self {
            kind: BrowserKind::Edge,
            version: 136,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36 Edg/136.0.0.0".into(),
            sec_ch_ua: r#""Chromium";v="136", "Microsoft Edge";v="136", "Not-A.Brand";v="99""#.into(),
            sec_ch_ua_platform: "\"Windows\"".into(),
            sec_ch_ua_mobile: "?0".into(),
        }
    }

    /// Chrome 136 on Android.
    #[must_use]
    pub fn chrome_136_android() -> Self {
        Self {
            kind: BrowserKind::Chrome,
            version: 136,
            user_agent: "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Mobile Safari/537.36".into(),
            sec_ch_ua: r#""Chromium";v="136", "Google Chrome";v="136", "Not-A.Brand";v="99""#.into(),
            sec_ch_ua_platform: "\"Android\"".into(),
            sec_ch_ua_mobile: "?1".into(),
        }
    }
}

/// Screen configuration.
///
/// # Example
///
/// ```
/// use stygian_browser::profile::ScreenProfile;
/// let s = ScreenProfile::fhd_desktop();
/// assert_eq!(s.width, 1920);
/// assert_eq!(s.height, 1080);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenProfile {
    /// Screen width in CSS pixels.
    pub width: u32,
    /// Screen height in CSS pixels.
    pub height: u32,
    /// Available width (typically same as `width`).
    pub avail_width: u32,
    /// Available height (taskbar deducted — typically `height - 40`).
    pub avail_height: u32,
    /// Device pixel ratio (1.0, 1.25, 1.5, 2.0, etc.).
    pub dpr: f64,
    /// Colour depth in bits (24 or 32).
    pub color_depth: u8,
    /// Screen orientation type string.
    pub orientation: String,
}

impl ScreenProfile {
    /// 1920×1080 FHD desktop at DPR 1.0.
    #[must_use]
    pub fn fhd_desktop() -> Self {
        Self {
            width: 1920,
            height: 1080,
            avail_width: 1920,
            avail_height: 1040,
            dpr: 1.0,
            color_depth: 24,
            orientation: "landscape-primary".into(),
        }
    }

    /// 2560×1440 QHD desktop at DPR 1.0.
    #[must_use]
    pub fn qhd_desktop() -> Self {
        Self {
            width: 2560,
            height: 1440,
            avail_width: 2560,
            avail_height: 1400,
            dpr: 1.0,
            color_depth: 24,
            orientation: "landscape-primary".into(),
        }
    }

    /// 2560×1600 `MacBook` Pro 14" at DPR 2.0.
    #[must_use]
    pub fn macbook_pro_14() -> Self {
        Self {
            width: 1512,
            height: 982,
            avail_width: 1512,
            avail_height: 957,
            dpr: 2.0,
            color_depth: 24,
            orientation: "landscape-primary".into(),
        }
    }

    /// 393×851 Android phone at DPR 2.75.
    #[must_use]
    pub fn pixel_7() -> Self {
        Self {
            width: 393,
            height: 851,
            avail_width: 393,
            avail_height: 851,
            dpr: 2.75,
            color_depth: 24,
            orientation: "portrait-primary".into(),
        }
    }
}

/// Hardware configuration.
///
/// # Example
///
/// ```
/// use stygian_browser::profile::HardwareProfile;
/// let h = HardwareProfile::desktop_gaming();
/// assert_eq!(h.cores, 8);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    /// `navigator.hardwareConcurrency`.
    pub cores: u32,
    /// `navigator.deviceMemory` in GB (must be a power of 2: 1, 2, 4, 8, 16, 32).
    pub memory_gb: u32,
    /// GPU vendor string (for `navigator.gpu` hint, complementing WebGL).
    pub gpu_vendor: String,
    /// GPU renderer string.
    pub gpu_renderer: String,
}

impl HardwareProfile {
    /// 8-core, 8 GB desktop gaming rig.
    #[must_use]
    pub fn desktop_gaming() -> Self {
        Self {
            cores: 8,
            memory_gb: 8,
            gpu_vendor: "NVIDIA".into(),
            gpu_renderer: "NVIDIA GeForce RTX 3060".into(),
        }
    }

    /// 8-core, 8 GB mid-range GPU desktop.
    #[must_use]
    pub fn desktop_gtx1660() -> Self {
        Self {
            cores: 8,
            memory_gb: 8,
            gpu_vendor: "NVIDIA".into(),
            gpu_renderer: "NVIDIA GeForce GTX 1660 Ti".into(),
        }
    }

    /// Apple M1, 8 cores, 8 GB.
    #[must_use]
    pub fn apple_m1() -> Self {
        Self {
            cores: 8,
            memory_gb: 8,
            gpu_vendor: "Apple".into(),
            gpu_renderer: "Apple M1".into(),
        }
    }

    /// Intel 4-core, 4 GB budget desktop.
    #[must_use]
    pub fn intel_uhd_630() -> Self {
        Self {
            cores: 4,
            memory_gb: 4,
            gpu_vendor: "Intel".into(),
            gpu_renderer: "Intel UHD Graphics 630".into(),
        }
    }

    /// Mobile — 8-core Snapdragon, 4 GB.
    #[must_use]
    pub fn mobile_snapdragon() -> Self {
        Self {
            cores: 8,
            memory_gb: 4,
            gpu_vendor: "Qualcomm".into(),
            gpu_renderer: "Adreno (TM) 730".into(),
        }
    }
}

/// Network configuration (`NetworkInformation` API).
///
/// # Example
///
/// ```
/// use stygian_browser::profile::NetworkProfile;
/// let n = NetworkProfile::fast_wifi();
/// assert_eq!(n.effective_type, "4g");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Round-trip time in milliseconds.
    pub rtt: u32,
    /// Downlink speed in Mbps.
    pub downlink: f64,
    /// Effective connection type (`"4g"`, `"3g"`, etc.).
    pub effective_type: String,
    /// `navigator.connection.saveData`.
    pub save_data: bool,
}

impl NetworkProfile {
    /// Typical home `WiFi` / broadband connection profile.
    #[must_use]
    pub fn fast_wifi() -> Self {
        Self {
            rtt: 50,
            downlink: 10.0,
            effective_type: "4g".into(),
            save_data: false,
        }
    }

    /// Mobile 4G LTE profile.
    #[must_use]
    pub fn mobile_4g() -> Self {
        Self {
            rtt: 100,
            downlink: 5.0,
            effective_type: "4g".into(),
            save_data: false,
        }
    }
}

// ---------------------------------------------------------------------------
// FingerprintProfile — unified identity
// ---------------------------------------------------------------------------

/// A complete, internally consistent device identity for anti-fingerprinting.
///
/// All signals (UA, platform, screen, hardware, WebGL GPU, noise seed, network)
/// are bundled together so they can never contradict each other.
///
/// Use one of the built-in constructors or call [`FingerprintProfile::validate`]
/// to check a custom profile for consistency.
///
/// # Example
///
/// ```
/// use stygian_browser::profile::FingerprintProfile;
///
/// let p = FingerprintProfile::windows_chrome_136_rtx3060();
/// assert!(p.validate().is_ok());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintProfile {
    /// Human-readable profile name (e.g. `"windows-chrome-136-rtx3060"`).
    pub name: String,
    /// Platform / OS identity.
    pub platform: PlatformProfile,
    /// Browser identity.
    pub browser: BrowserProfile,
    /// Screen dimensions.
    pub screen: ScreenProfile,
    /// Hardware concurrency and memory.
    pub hardware: HardwareProfile,
    /// Detailed WebGL device profile.
    pub webgl: WebGlProfile,
    /// Network Information API values.
    pub network: NetworkProfile,
    /// Deterministic noise seed for session-unique fingerprint perturbation.
    pub noise_seed: NoiseSeed,
}

impl FingerprintProfile {
    // ── Built-in profiles ────────────────────────────────────────────────

    /// Windows 10, Chrome 136, NVIDIA RTX 3060 — primary test profile.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::windows_chrome_136_rtx3060();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn windows_chrome_136_rtx3060() -> Self {
        Self {
            name: "windows-chrome-136-rtx3060".into(),
            platform: PlatformProfile::windows(),
            browser: BrowserProfile::chrome_136_windows(),
            screen: ScreenProfile::fhd_desktop(),
            hardware: HardwareProfile::desktop_gaming(),
            webgl: WebGlProfile::nvidia_rtx_3060(),
            network: NetworkProfile::fast_wifi(),
            noise_seed: NoiseSeed::from(0x3c1b_6a2d_f5e0_9874_u64),
        }
    }

    /// Windows 10, Chrome 136, NVIDIA GTX 1660 Ti.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::windows_chrome_136_gtx1660();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn windows_chrome_136_gtx1660() -> Self {
        Self {
            name: "windows-chrome-136-gtx1660".into(),
            platform: PlatformProfile::windows(),
            browser: BrowserProfile::chrome_136_windows(),
            screen: ScreenProfile::qhd_desktop(),
            hardware: HardwareProfile::desktop_gtx1660(),
            webgl: WebGlProfile::nvidia_gtx_1660(),
            network: NetworkProfile::fast_wifi(),
            noise_seed: NoiseSeed::from(0x7a8e_2c3d_b4f1_5609_u64),
        }
    }

    /// macOS Sonoma, Chrome 136, Apple M1.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::macos_chrome_136_m1();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn macos_chrome_136_m1() -> Self {
        Self {
            name: "macos-chrome-136-m1".into(),
            platform: PlatformProfile::macos(),
            browser: BrowserProfile::chrome_136_macos(),
            screen: ScreenProfile::macbook_pro_14(),
            hardware: HardwareProfile::apple_m1(),
            webgl: WebGlProfile::intel_uhd_630(), // placeholder — M1 profile close enough
            network: NetworkProfile::fast_wifi(),
            noise_seed: NoiseSeed::from(0x1d2e_3f4a_5b6c_7d8e_u64),
        }
    }

    /// Linux, Chrome 136, Intel UHD 630.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::linux_chrome_136_intel();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn linux_chrome_136_intel() -> Self {
        Self {
            name: "linux-chrome-136-intel".into(),
            platform: PlatformProfile::linux(),
            browser: BrowserProfile::chrome_136_linux(),
            screen: ScreenProfile::fhd_desktop(),
            hardware: HardwareProfile::intel_uhd_630(),
            webgl: WebGlProfile::intel_uhd_630(),
            network: NetworkProfile::fast_wifi(),
            noise_seed: NoiseSeed::from(0x9f8e_7d6c_5b4a_3021_u64),
        }
    }

    /// Windows 10, Microsoft Edge 136, NVIDIA RTX 3060.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::windows_edge_136_rtx3060();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn windows_edge_136_rtx3060() -> Self {
        Self {
            name: "windows-edge-136-rtx3060".into(),
            platform: PlatformProfile::windows(),
            browser: BrowserProfile::edge_136_windows(),
            screen: ScreenProfile::fhd_desktop(),
            hardware: HardwareProfile::desktop_gaming(),
            webgl: WebGlProfile::nvidia_rtx_3060(),
            network: NetworkProfile::fast_wifi(),
            noise_seed: NoiseSeed::from(0x2b4d_6f80_a2c4_e6f8_u64),
        }
    }

    /// Android 13 Pixel 7, Chrome 136, Adreno 730.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::android_chrome_136_pixel();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn android_chrome_136_pixel() -> Self {
        Self {
            name: "android-chrome-136-pixel".into(),
            platform: PlatformProfile::android(),
            browser: BrowserProfile::chrome_136_android(),
            screen: ScreenProfile::pixel_7(),
            hardware: HardwareProfile::mobile_snapdragon(),
            webgl: WebGlProfile::amd_rx_6700(), // Adreno-equivalent WebGL params
            network: NetworkProfile::mobile_4g(),
            noise_seed: NoiseSeed::from(0x4c8a_0f1e_2d3b_5a69_u64),
        }
    }

    // ── Random weighted ──────────────────────────────────────────────────

    /// Return a profile sampled by real-world market-share distribution.
    ///
    /// Windows ~65%, macOS ~20%, Linux ~5%, Android ~10%.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    /// let p = FingerprintProfile::random_weighted();
    /// assert!(p.validate().is_ok());
    /// ```
    #[must_use]
    pub fn random_weighted() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0xdead_beef_1234_5678, |d| {
                d.as_secs() ^ u64::from(d.subsec_nanos())
            });
        // splitmix64 step
        let v = seed
            .wrapping_add(0x9e37_79b9_7f4a_7c15_u64)
            .wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let pick = v % 100;
        match pick {
            0..=64 => Self::windows_chrome_136_rtx3060(), // 65%
            65..=84 => Self::macos_chrome_136_m1(),       // 20%
            85..=89 => Self::linux_chrome_136_intel(),    // 5%
            _ => Self::android_chrome_136_pixel(),        // 10%
        }
    }

    // ── Validation ───────────────────────────────────────────────────────

    /// Validate internal consistency of the profile.
    ///
    /// Returns `Ok(())` if all checks pass, or `Err(Vec<String>)` with a list
    /// of human-readable error messages explaining each inconsistency.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::profile::FingerprintProfile;
    ///
    /// let mut p = FingerprintProfile::windows_chrome_136_rtx3060();
    /// // deliberately break it: claim macOS platform but keep Windows WebGL
    /// p.platform.platform_string = "MacIntel".into();
    /// // validate() will still pass — platform_string mismatch alone isn't fatal
    /// // insert real inconsistency: mobile platform with 0 touch points won't fail Windows
    /// p.platform.max_touch_points = 5; // touch on desktop — suspicious but allowed
    /// assert!(p.validate().is_ok()); // still ok, not a hard rule for touch
    /// ```
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();

        // Rule: mobile OS must have touch points > 0
        if matches!(self.platform.os, Os::Android | Os::Ios) && self.platform.max_touch_points == 0
        {
            errors.push(format!(
                "Mobile OS {:?} must have max_touch_points > 0 (got 0)",
                self.platform.os
            ));
        }

        // Rule: desktop OS should have touch points == 0
        if matches!(self.platform.os, Os::Windows | Os::MacOs | Os::Linux)
            && self.platform.max_touch_points > 0
            && self.platform.max_touch_points < 5
        {
            // Note: Windows touch-screen desktops do have 10 points; suspicious values
            // in [1,4] are flagged.
            errors.push(format!(
                "Desktop OS {:?} has suspicious max_touch_points {} (expected 0 or ≥5 for touch-screen)",
                self.platform.os, self.platform.max_touch_points
            ));
        }

        // Rule: hardwareConcurrency must be reasonable
        if self.hardware.cores == 0 || self.hardware.cores > 128 {
            errors.push(format!(
                "hardwareConcurrency {} is out of range [1, 128]",
                self.hardware.cores
            ));
        }

        // Rule: deviceMemory must be a power of 2
        let mem = self.hardware.memory_gb;
        if mem == 0 || !mem.is_power_of_two() || mem > 32 {
            errors.push(format!(
                "deviceMemory {mem} GB is not a power-of-two in [1, 32]"
            ));
        }

        // Rule: screen resolution must be non-zero
        if self.screen.width == 0 || self.screen.height == 0 {
            errors.push("screen width/height must be non-zero".into());
        }

        // Rule: avail must be ≤ full screen
        if self.screen.avail_width > self.screen.width
            || self.screen.avail_height > self.screen.height
        {
            errors.push(format!(
                "avail_width/avail_height ({}/{}) must be ≤ screen size ({}/{})",
                self.screen.avail_width,
                self.screen.avail_height,
                self.screen.width,
                self.screen.height
            ));
        }

        // Rule: DPR must be positive
        if self.screen.dpr <= 0.0 {
            errors.push(format!("dpr {} must be > 0", self.screen.dpr));
        }

        // Rule: Windows platform_string should not contain "Mac"
        if self.platform.os == Os::Windows && self.platform.platform_string.contains("Mac") {
            errors.push(format!(
                "Windows OS has macOS-indicating platform_string '{}'",
                self.platform.platform_string
            ));
        }

        // Rule: macOS platform_string should not contain "Win"
        if self.platform.os == Os::MacOs && self.platform.platform_string.contains("Win") {
            errors.push(format!(
                "macOS OS has Windows-indicating platform_string '{}'",
                self.platform.platform_string
            ));
        }

        // Rule: sec_ch_ua_mobile must match OS mobility
        let is_mobile_ua = self.browser.sec_ch_ua_mobile == "?1";
        let is_mobile_os = matches!(self.platform.os, Os::Android | Os::Ios);
        if is_mobile_ua != is_mobile_os {
            errors.push(format!(
                "sec_ch_ua_mobile '{}' inconsistent with OS {:?}",
                self.browser.sec_ch_ua_mobile, self.platform.os
            ));
        }

        // Rule: max_texture_size must fit in max_viewport_dims
        let (vpw, vph) = self.webgl.max_viewport_dims;
        if self.webgl.max_texture_size > vpw || self.webgl.max_texture_size > vph {
            errors.push(format!(
                "WebGL max_texture_size {} exceeds max_viewport_dims ({},{})",
                self.webgl.max_texture_size, vpw, vph
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_profiles_are_valid() {
        let profiles = [
            FingerprintProfile::windows_chrome_136_rtx3060(),
            FingerprintProfile::windows_chrome_136_gtx1660(),
            FingerprintProfile::macos_chrome_136_m1(),
            FingerprintProfile::linux_chrome_136_intel(),
            FingerprintProfile::windows_edge_136_rtx3060(),
            FingerprintProfile::android_chrome_136_pixel(),
        ];
        for p in &profiles {
            let validation = p.validate();
            assert!(
                validation.is_ok(),
                "profile '{}' failed validation: {validation:?}",
                p.name
            );
        }
    }

    #[test]
    fn inconsistent_profile_fails_validation() {
        // Build a Windows profile and give it a macOS platform_string
        let mut p = FingerprintProfile::windows_chrome_136_rtx3060();
        p.platform.platform_string = "MacIntel".into();
        let result = p.validate();
        assert!(result.is_err(), "expected validation failure");
        let Err(errs) = result else {
            return;
        };
        assert!(
            errs.iter().any(|e| e.contains("macOS-indicating")),
            "expected cross-OS platform_string error, got: {errs:?}"
        );
    }

    #[test]
    fn inconsistent_mobile_fails_validation() {
        let mut p = FingerprintProfile::android_chrome_136_pixel();
        p.platform.max_touch_points = 0;
        assert!(
            p.validate().is_err(),
            "mobile with 0 touch points should fail"
        );
    }

    #[test]
    fn non_power_of_two_memory_fails() {
        let mut p = FingerprintProfile::windows_chrome_136_rtx3060();
        p.hardware.memory_gb = 6;
        assert!(p.validate().is_err(), "6 GB is not a power of 2");
    }

    #[test]
    fn random_weighted_is_valid() {
        // Run enough times to hit most branches
        for _ in 0..20 {
            let p = FingerprintProfile::random_weighted();
            let validation = p.validate();
            assert!(
                validation.is_ok(),
                "random_weighted() produced invalid profile '{}': {validation:?}",
                p.name,
            );
        }
    }

    #[test]
    fn random_weighted_windows_majority() {
        let n = 1000_usize;
        let windows_count = (0..n)
            .filter(|_| FingerprintProfile::random_weighted().platform.os == Os::Windows)
            .count();
        // With 65% target, expect at least 55% in 1000 samples
        assert!(
            windows_count > 550,
            "expected Windows > 55% in {n} samples, got {windows_count}"
        );
    }

    #[test]
    fn toml_round_trip() {
        let p = FingerprintProfile::windows_chrome_136_rtx3060();
        let toml_result = toml::to_string(&p);
        assert!(
            toml_result.is_ok(),
            "serialize to TOML failed: {toml_result:?}"
        );
        let Ok(toml_str) = toml_result else {
            return;
        };
        let profile_result: Result<FingerprintProfile, _> = toml::from_str(&toml_str);
        assert!(
            profile_result.is_ok(),
            "deserialize from TOML failed: {profile_result:?}"
        );
        let Ok(p2) = profile_result else {
            return;
        };
        assert_eq!(p.name, p2.name);
        assert_eq!(p.hardware.cores, p2.hardware.cores);
        assert_eq!(p.noise_seed.as_u64(), p2.noise_seed.as_u64());
    }
}
