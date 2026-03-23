//! Browser configuration and options
//!
//! All configuration can be overridden via environment variables at runtime.
//! See individual fields for the corresponding `STYGIAN_*` variable names.
//!
//! ## Configuration priority
//!
//! Programmatic (builder) > environment variables > JSON file > compiled-in defaults.
//!
//! Use [`BrowserConfig::from_json_file`] or [`BrowserConfig::from_json_str`] to
//! load a base configuration from disk, then override individual settings via
//! the builder or environment variables.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::cdp_protection::CdpFixMode;

#[cfg(feature = "stealth")]
use crate::webrtc::WebRtcConfig;

// ─── HeadlessMode ───────────────────────────────────────────────────────────────

/// Controls which headless mode Chrome is launched in.
///
/// The *new* headless mode (`--headless=new`, available since Chromium 112)
/// shares the same rendering pipeline as a headed Chrome window and is
/// significantly harder to fingerprint-detect. It is the default.
///
/// Fall back to [`Legacy`][HeadlessMode::Legacy] only when targeting very old
/// Chromium builds that do not support `--headless=new`.
///
/// Env: `STYGIAN_HEADLESS_MODE` (`new`/`legacy`, default: `new`)
///
/// # Example
///
/// ```
/// use stygian_browser::BrowserConfig;
/// use stygian_browser::config::HeadlessMode;
/// let cfg = BrowserConfig::builder()
///     .headless(true)
///     .headless_mode(HeadlessMode::New)
///     .build();
/// assert_eq!(cfg.headless_mode, HeadlessMode::New);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HeadlessMode {
    /// `--headless=new` — shares Chrome's headed rendering pipeline.
    /// Default. Requires Chromium 112+.
    #[default]
    New,
    /// Classic `--headless` flag. Use only for Chromium < 112.
    Legacy,
}

impl HeadlessMode {
    /// Read from `STYGIAN_HEADLESS_MODE` env var (`new`/`legacy`).
    pub fn from_env() -> Self {
        match std::env::var("STYGIAN_HEADLESS_MODE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "legacy" => Self::Legacy,
            _ => Self::New,
        }
    }
}

// ─── StealthLevel ─────────────────────────────────────────────────────────────

/// Anti-detection intensity level.
///
/// Higher levels apply more fingerprint spoofing and behavioral mimicry at the
/// cost of additional CPU/memory overhead.
///
/// # Example
///
/// ```
/// use stygian_browser::config::StealthLevel;
/// let level = StealthLevel::Advanced;
/// assert!(level.is_active());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StealthLevel {
    /// No anti-detection applied. Useful for trusted, internal targets.
    None,
    /// Core protections only: `navigator.webdriver` removal and CDP leak fix.
    Basic,
    /// Full suite: fingerprint injection, human behavior, WebRTC spoofing.
    #[default]
    Advanced,
}

impl StealthLevel {
    /// Returns `true` for any level other than [`StealthLevel::None`].
    #[must_use]
    pub fn is_active(self) -> bool {
        self != Self::None
    }

    /// Parse `source_url` from `STYGIAN_SOURCE_URL` (`0` disables).
    pub fn from_env() -> Self {
        match std::env::var("STYGIAN_STEALTH_LEVEL")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "none" => Self::None,
            "basic" => Self::Basic,
            _ => Self::Advanced,
        }
    }
}

// ─── PoolConfig ───────────────────────────────────────────────────────────────

/// Browser pool sizing and lifecycle settings.
///
/// # Example
///
/// ```
/// use stygian_browser::config::PoolConfig;
/// let cfg = PoolConfig::default();
/// assert_eq!(cfg.min_size, 2);
/// assert_eq!(cfg.max_size, 10);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Minimum warm instances kept ready at all times.
    ///
    /// Env: `STYGIAN_POOL_MIN` (default: `2`)
    pub min_size: usize,

    /// Maximum concurrent browser instances.
    ///
    /// Env: `STYGIAN_POOL_MAX` (default: `10`)
    pub max_size: usize,

    /// How long an idle browser is kept before eviction.
    ///
    /// Env: `STYGIAN_POOL_IDLE_SECS` (default: `300`)
    #[serde(with = "duration_secs")]
    pub idle_timeout: Duration,

    /// Maximum time to wait for a pool slot before returning
    /// [`PoolExhausted`][crate::error::BrowserError::PoolExhausted].
    ///
    /// Env: `STYGIAN_POOL_ACQUIRE_SECS` (default: `5`)
    #[serde(with = "duration_secs")]
    pub acquire_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_size: env_usize("STYGIAN_POOL_MIN", 2),
            max_size: env_usize("STYGIAN_POOL_MAX", 10),
            idle_timeout: Duration::from_secs(env_u64("STYGIAN_POOL_IDLE_SECS", 300)),
            acquire_timeout: Duration::from_secs(env_u64("STYGIAN_POOL_ACQUIRE_SECS", 5)),
        }
    }
}

// ─── BrowserConfig ────────────────────────────────────────────────────────────

/// Top-level configuration for a browser session.
///
/// # Example
///
/// ```
/// use stygian_browser::BrowserConfig;
///
/// let config = BrowserConfig::builder()
///     .headless(true)
///     .window_size(1920, 1080)
///     .build();
///
/// assert!(config.headless);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Path to the Chrome/Chromium executable.
    ///
    /// Env: `STYGIAN_CHROME_PATH`
    pub chrome_path: Option<PathBuf>,

    /// Extra Chrome launch arguments appended after the defaults.
    pub args: Vec<String>,

    /// Run in headless mode (no visible window).
    ///
    /// Env: `STYGIAN_HEADLESS` (`true`/`false`, default: `true`)
    pub headless: bool,

    /// Persistent user profile directory. `None` = temporary profile.
    pub user_data_dir: Option<PathBuf>,

    /// Which headless mode to use when `headless` is `true`.
    ///
    /// Defaults to [`HeadlessMode::New`] (`--headless=new`).
    ///
    /// Env: `STYGIAN_HEADLESS_MODE` (`new`/`legacy`)
    pub headless_mode: HeadlessMode,

    /// Browser window size in pixels (width, height).
    pub window_size: Option<(u32, u32)>,

    /// Attach `DevTools` on launch (useful for debugging, disable in production).
    pub devtools: bool,

    /// HTTP/SOCKS proxy URL, e.g. `http://user:pass@host:port`.
    pub proxy: Option<String>,

    /// Comma-separated list of hosts that bypass the proxy.
    ///
    /// Env: `STYGIAN_PROXY_BYPASS` (e.g. `"<local>,localhost,127.0.0.1"`)
    pub proxy_bypass_list: Option<String>,

    /// WebRTC IP-leak prevention and geolocation consistency settings.
    ///
    /// Only active when the `stealth` feature is enabled.
    #[cfg(feature = "stealth")]
    pub webrtc: WebRtcConfig,

    /// Anti-detection intensity level.
    pub stealth_level: StealthLevel,

    /// Disable Chromium's built-in renderer sandbox (`--no-sandbox`).
    ///
    /// Chromium's sandbox requires user namespaces, which are unavailable inside
    /// most container runtimes. When running in Docker or similar, set this to
    /// `true` (or set `STYGIAN_DISABLE_SANDBOX=true`) and rely on the
    /// container's own isolation instead.
    ///
    /// **Never set this on a bare-metal host without an alternative isolation
    /// boundary.** Doing so removes a meaningful security layer.
    ///
    /// Env: `STYGIAN_DISABLE_SANDBOX` (`true`/`false`, default: auto-detect)
    pub disable_sandbox: bool,

    /// CDP Runtime.enable leak-mitigation mode.
    ///
    /// Env: `STYGIAN_CDP_FIX_MODE` (`add_binding`/`isolated_world`/`enable_disable`/`none`)
    pub cdp_fix_mode: CdpFixMode,

    /// Source URL injected into `Function.prototype.toString` patches, or
    /// `None` to use the default (`"app.js"`).
    ///
    /// Set to `"0"` (as a string) to disable sourceURL patching entirely.
    ///
    /// Env: `STYGIAN_SOURCE_URL`
    pub source_url: Option<String>,

    /// Browser pool settings.
    pub pool: PoolConfig,

    /// Browser launch timeout.
    ///
    /// Env: `STYGIAN_LAUNCH_TIMEOUT_SECS` (default: `10`)
    #[serde(with = "duration_secs")]
    pub launch_timeout: Duration,

    /// Per-operation CDP timeout.
    ///
    /// Env: `STYGIAN_CDP_TIMEOUT_SECS` (default: `30`)
    #[serde(with = "duration_secs")]
    pub cdp_timeout: Duration,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            chrome_path: std::env::var("STYGIAN_CHROME_PATH").ok().map(PathBuf::from),
            args: vec![],
            headless: env_bool("STYGIAN_HEADLESS", true),
            user_data_dir: None,
            headless_mode: HeadlessMode::from_env(),
            window_size: Some((1920, 1080)),
            devtools: false,
            proxy: std::env::var("STYGIAN_PROXY").ok(),
            proxy_bypass_list: std::env::var("STYGIAN_PROXY_BYPASS").ok(),
            #[cfg(feature = "stealth")]
            webrtc: WebRtcConfig::default(),
            disable_sandbox: env_bool("STYGIAN_DISABLE_SANDBOX", is_containerized()),
            stealth_level: StealthLevel::from_env(),
            cdp_fix_mode: CdpFixMode::from_env(),
            source_url: std::env::var("STYGIAN_SOURCE_URL").ok(),
            pool: PoolConfig::default(),
            launch_timeout: Duration::from_secs(env_u64("STYGIAN_LAUNCH_TIMEOUT_SECS", 10)),
            cdp_timeout: Duration::from_secs(env_u64("STYGIAN_CDP_TIMEOUT_SECS", 30)),
        }
    }
}

impl BrowserConfig {
    /// Create a configuration builder with defaults pre-populated.
    pub fn builder() -> BrowserConfigBuilder {
        BrowserConfigBuilder {
            config: Self::default(),
        }
    }

    /// Collect the effective Chrome launch arguments.
    ///
    /// Returns the anti-detection baseline args merged with any user-supplied
    /// extras from [`BrowserConfig::args`].
    pub fn effective_args(&self) -> Vec<String> {
        let mut args = vec![
            "--disable-blink-features=AutomationControlled".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--disable-infobars".to_string(),
            "--disable-background-timer-throttling".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
            "--disable-renderer-backgrounding".to_string(),
        ];

        if self.disable_sandbox {
            args.push("--no-sandbox".to_string());
        }

        if let Some(proxy) = &self.proxy {
            args.push(format!("--proxy-server={proxy}"));
        }

        if let Some(bypass) = &self.proxy_bypass_list {
            args.push(format!("--proxy-bypass-list={bypass}"));
        }

        #[cfg(feature = "stealth")]
        args.extend(self.webrtc.chrome_args());

        if let Some((w, h)) = self.window_size {
            args.push(format!("--window-size={w},{h}"));
        }

        args.extend_from_slice(&self.args);
        args
    }

    /// Validate the configuration, returning a list of human-readable errors.
    ///
    /// Returns `Ok(())` when valid, or `Err(errors)` with a non-empty list.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// use stygian_browser::config::PoolConfig;
    /// use std::time::Duration;
    ///
    /// let mut cfg = BrowserConfig::default();
    /// cfg.pool.min_size = 0;
    /// cfg.pool.max_size = 0; // invalid: max must be >= 1
    /// let errors = cfg.validate().unwrap_err();
    /// assert!(!errors.is_empty());
    /// ```
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();

        if self.pool.min_size > self.pool.max_size {
            errors.push(format!(
                "pool.min_size ({}) must be <= pool.max_size ({})",
                self.pool.min_size, self.pool.max_size
            ));
        }
        if self.pool.max_size == 0 {
            errors.push("pool.max_size must be >= 1".to_string());
        }
        if self.launch_timeout.is_zero() {
            errors.push("launch_timeout must be positive".to_string());
        }
        if self.cdp_timeout.is_zero() {
            errors.push("cdp_timeout must be positive".to_string());
        }
        if let Some(proxy) = &self.proxy
            && !proxy.starts_with("http://")
            && !proxy.starts_with("https://")
            && !proxy.starts_with("socks4://")
            && !proxy.starts_with("socks5://")
        {
            errors.push(format!(
                "proxy URL must start with http://, https://, socks4:// or socks5://; got: {proxy}"
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Serialize this configuration to a JSON string.
    ///
    /// # Errors
    ///
    /// Returns a [`serde_json::Error`] if serialization fails (very rare).
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::default();
    /// let json = cfg.to_json().unwrap();
    /// assert!(json.contains("headless"));
    /// ```
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize a [`BrowserConfig`] from a JSON string.
    ///
    /// Environment variable overrides will NOT be re-applied — the JSON values
    /// are used verbatim.  Chain with builder methods to override individual
    /// fields after loading.
    ///
    /// # Errors
    ///
    /// Returns a [`serde_json::Error`] if the input is invalid JSON or has
    /// missing required fields.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::default();
    /// let json = cfg.to_json().unwrap();
    /// let back = BrowserConfig::from_json_str(&json).unwrap();
    /// assert_eq!(back.headless, cfg.headless);
    /// ```
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Load a [`BrowserConfig`] from a JSON file on disk.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::error::BrowserError::ConfigError`] wrapping any I/O
    /// or parse error.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::from_json_file("/etc/stygian/config.json").unwrap();
    /// ```
    pub fn from_json_file(path: impl AsRef<std::path::Path>) -> crate::error::Result<Self> {
        use crate::error::BrowserError;
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            BrowserError::ConfigError(format!(
                "cannot read config file {}: {e}",
                path.as_ref().display()
            ))
        })?;
        serde_json::from_str(&content).map_err(|e| {
            BrowserError::ConfigError(format!(
                "invalid JSON in config file {}: {e}",
                path.as_ref().display()
            ))
        })
    }
}

// ─── Builder ──────────────────────────────────────────────────────────────────

/// Fluent builder for [`BrowserConfig`].
pub struct BrowserConfigBuilder {
    config: BrowserConfig,
}

impl BrowserConfigBuilder {
    /// Set path to the Chrome executable.
    #[must_use]
    pub fn chrome_path(mut self, path: PathBuf) -> Self {
        self.config.chrome_path = Some(path);
        self
    }

    /// Set a custom user profile directory.
    ///
    /// When not set, each browser instance automatically uses a unique
    /// temporary directory derived from its instance ID, preventing
    /// `SingletonLock` races between concurrent pools or instances.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::builder()
    ///     .user_data_dir("/tmp/my-profile")
    ///     .build();
    /// assert!(cfg.user_data_dir.is_some());
    /// ```
    #[must_use]
    pub fn user_data_dir(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.config.user_data_dir = Some(path.into());
        self
    }

    /// Set headless mode.
    #[must_use]
    pub const fn headless(mut self, headless: bool) -> Self {
        self.config.headless = headless;
        self
    }

    /// Choose between `--headless=new` (default) and the legacy `--headless` flag.
    ///
    /// Only relevant when [`headless`][Self::headless] is `true`. Has no effect
    /// in headed mode.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// use stygian_browser::config::HeadlessMode;
    /// let cfg = BrowserConfig::builder()
    ///     .headless_mode(HeadlessMode::Legacy)
    ///     .build();
    /// assert_eq!(cfg.headless_mode, HeadlessMode::Legacy);
    /// ```
    #[must_use]
    pub const fn headless_mode(mut self, mode: HeadlessMode) -> Self {
        self.config.headless_mode = mode;
        self
    }

    /// Set browser viewport / window size.
    #[must_use]
    pub const fn window_size(mut self, width: u32, height: u32) -> Self {
        self.config.window_size = Some((width, height));
        self
    }

    /// Enable or disable `DevTools` attachment.
    #[must_use]
    pub const fn devtools(mut self, enabled: bool) -> Self {
        self.config.devtools = enabled;
        self
    }

    /// Set proxy URL.
    #[must_use]
    pub fn proxy(mut self, proxy: String) -> Self {
        self.config.proxy = Some(proxy);
        self
    }

    /// Set a comma-separated proxy bypass list.
    ///
    /// # Example
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::builder()
    ///     .proxy("http://proxy:8080".to_string())
    ///     .proxy_bypass_list("<local>,localhost".to_string())
    ///     .build();
    /// assert!(cfg.effective_args().iter().any(|a| a.contains("proxy-bypass")));
    /// ```
    #[must_use]
    pub fn proxy_bypass_list(mut self, bypass: String) -> Self {
        self.config.proxy_bypass_list = Some(bypass);
        self
    }

    /// Set WebRTC IP-leak prevention config.
    ///
    /// # Example
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// use stygian_browser::webrtc::{WebRtcConfig, WebRtcPolicy};
    /// let cfg = BrowserConfig::builder()
    ///     .webrtc(WebRtcConfig { policy: WebRtcPolicy::BlockAll, ..Default::default() })
    ///     .build();
    /// assert!(cfg.effective_args().iter().any(|a| a.contains("disable_non_proxied")));
    /// ```
    #[cfg(feature = "stealth")]
    #[must_use]
    pub fn webrtc(mut self, webrtc: WebRtcConfig) -> Self {
        self.config.webrtc = webrtc;
        self
    }

    /// Append a custom Chrome argument.
    #[must_use]
    pub fn arg(mut self, arg: String) -> Self {
        self.config.args.push(arg);
        self
    }

    /// Add Chrome launch flags that constrain TLS to match a [`TlsProfile`].
    ///
    /// Appends version-constraint flags (e.g. `--ssl-version-max=tls1.2`)
    /// to the extra args list. See [`chrome_tls_args`] for details on what
    /// Chrome can and cannot control via flags.
    ///
    /// [`TlsProfile`]: crate::tls::TlsProfile
    /// [`chrome_tls_args`]: crate::tls::chrome_tls_args
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// use stygian_browser::tls::CHROME_131;
    ///
    /// let cfg = BrowserConfig::builder()
    ///     .tls_profile(&CHROME_131)
    ///     .build();
    /// // Chrome 131 supports both TLS 1.2 and 1.3 — no extra flags needed.
    /// ```
    #[cfg(feature = "stealth")]
    #[must_use]
    pub fn tls_profile(mut self, profile: &crate::tls::TlsProfile) -> Self {
        self.config
            .args
            .extend(crate::tls::chrome_tls_args(profile));
        self
    }

    /// Set the stealth level.
    #[must_use]
    pub const fn stealth_level(mut self, level: StealthLevel) -> Self {
        self.config.stealth_level = level;
        self
    }

    /// Explicitly control whether `--no-sandbox` is passed to Chrome.
    ///
    /// By default this is auto-detected: `true` inside containers, `false` on
    /// bare metal. Override only when the auto-detection is wrong.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// // Force sandbox on (bare-metal host)
    /// let cfg = BrowserConfig::builder().disable_sandbox(false).build();
    /// assert!(!cfg.effective_args().iter().any(|a| a == "--no-sandbox"));
    /// ```
    #[must_use]
    pub const fn disable_sandbox(mut self, disable: bool) -> Self {
        self.config.disable_sandbox = disable;
        self
    }

    /// Set the CDP leak-mitigation mode.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// use stygian_browser::cdp_protection::CdpFixMode;
    /// let cfg = BrowserConfig::builder()
    ///     .cdp_fix_mode(CdpFixMode::IsolatedWorld)
    ///     .build();
    /// assert_eq!(cfg.cdp_fix_mode, CdpFixMode::IsolatedWorld);
    /// ```
    #[must_use]
    pub const fn cdp_fix_mode(mut self, mode: CdpFixMode) -> Self {
        self.config.cdp_fix_mode = mode;
        self
    }

    /// Override the `sourceURL` injected into CDP scripts, or pass `None` to
    /// disable sourceURL patching.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::BrowserConfig;
    /// let cfg = BrowserConfig::builder()
    ///     .source_url(Some("main.js".to_string()))
    ///     .build();
    /// assert_eq!(cfg.source_url.as_deref(), Some("main.js"));
    /// ```
    #[must_use]
    pub fn source_url(mut self, url: Option<String>) -> Self {
        self.config.source_url = url;
        self
    }

    /// Override pool settings.
    #[must_use]
    pub const fn pool(mut self, pool: PoolConfig) -> Self {
        self.config.pool = pool;
        self
    }

    /// Build the final [`BrowserConfig`].
    pub fn build(self) -> BrowserConfig {
        self.config
    }
}

// ─── Serde helpers ────────────────────────────────────────────────────────────

/// Serialize/deserialize `Duration` as integer seconds.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

// ─── Env helpers (private) ────────────────────────────────────────────────────

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .map(|v| !matches!(v.to_lowercase().as_str(), "false" | "0" | "no"))
        .unwrap_or(default)
}

/// Heuristic: returns `true` when the process appears to be running inside a
/// container (Docker, Kubernetes, etc.) where Chromium's renderer sandbox may
/// not function because user namespaces are unavailable.
///
/// Detection checks (Linux only):
/// - `/.dockerenv` file exists
/// - `/proc/1/cgroup` contains "docker" or "kubepods"
///
/// On non-Linux platforms this always returns `false` (macOS/Windows have
/// their own sandbox mechanisms and don't need `--no-sandbox`).
#[allow(clippy::missing_const_for_fn)] // Linux branch uses runtime file I/O (Path::exists, fs::read_to_string)
fn is_containerized() -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/.dockerenv").exists() {
            return true;
        }
        if let Ok(cgroup) = std::fs::read_to_string("/proc/1/cgroup")
            && (cgroup.contains("docker") || cgroup.contains("kubepods"))
        {
            return true;
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_headless() {
        let cfg = BrowserConfig::default();
        assert!(cfg.headless);
    }

    #[test]
    fn builder_roundtrip() {
        let cfg = BrowserConfig::builder()
            .headless(false)
            .window_size(1280, 720)
            .stealth_level(StealthLevel::Basic)
            .build();

        assert!(!cfg.headless);
        assert_eq!(cfg.window_size, Some((1280, 720)));
        assert_eq!(cfg.stealth_level, StealthLevel::Basic);
    }

    #[test]
    fn effective_args_include_anti_detection_flag() {
        let cfg = BrowserConfig::default();
        let args = cfg.effective_args();
        assert!(args.iter().any(|a| a.contains("AutomationControlled")));
    }

    #[test]
    fn no_sandbox_only_when_explicitly_enabled() {
        let with_sandbox_disabled = BrowserConfig::builder().disable_sandbox(true).build();
        assert!(
            with_sandbox_disabled
                .effective_args()
                .iter()
                .any(|a| a == "--no-sandbox")
        );

        let with_sandbox_enabled = BrowserConfig::builder().disable_sandbox(false).build();
        assert!(
            !with_sandbox_enabled
                .effective_args()
                .iter()
                .any(|a| a == "--no-sandbox")
        );
    }

    #[test]
    fn pool_config_defaults() {
        let p = PoolConfig::default();
        assert_eq!(p.min_size, 2);
        assert_eq!(p.max_size, 10);
    }

    #[test]
    fn stealth_level_none_not_active() {
        assert!(!StealthLevel::None.is_active());
        assert!(StealthLevel::Basic.is_active());
        assert!(StealthLevel::Advanced.is_active());
    }

    #[test]
    fn config_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = BrowserConfig::default();
        let json = serde_json::to_string(&cfg)?;
        let back: BrowserConfig = serde_json::from_str(&json)?;
        assert_eq!(back.headless, cfg.headless);
        assert_eq!(back.stealth_level, cfg.stealth_level);
        Ok(())
    }

    #[test]
    fn validate_default_config_is_valid() {
        let cfg = BrowserConfig::default();
        assert!(cfg.validate().is_ok(), "default config must be valid");
    }

    #[test]
    fn validate_detects_pool_size_inversion() {
        let cfg = BrowserConfig {
            pool: PoolConfig {
                min_size: 10,
                max_size: 5,
                ..PoolConfig::default()
            },
            ..BrowserConfig::default()
        };
        let result = cfg.validate();
        assert!(result.is_err());
        if let Err(errors) = result {
            assert!(errors.iter().any(|e| e.contains("min_size")));
        }
    }

    #[test]
    fn validate_detects_zero_max_pool() {
        let cfg = BrowserConfig {
            pool: PoolConfig {
                max_size: 0,
                ..PoolConfig::default()
            },
            ..BrowserConfig::default()
        };
        let result = cfg.validate();
        assert!(result.is_err());
        if let Err(errors) = result {
            assert!(errors.iter().any(|e| e.contains("max_size")));
        }
    }

    #[test]
    fn validate_detects_zero_timeouts() {
        let cfg = BrowserConfig {
            launch_timeout: std::time::Duration::ZERO,
            cdp_timeout: std::time::Duration::ZERO,
            ..BrowserConfig::default()
        };
        let result = cfg.validate();
        assert!(result.is_err());
        if let Err(errors) = result {
            assert_eq!(errors.len(), 2);
        }
    }

    #[test]
    fn validate_detects_bad_proxy_scheme() {
        let cfg = BrowserConfig {
            proxy: Some("ftp://bad.proxy:1234".to_string()),
            ..BrowserConfig::default()
        };
        let result = cfg.validate();
        assert!(result.is_err());
        if let Err(errors) = result {
            assert!(errors.iter().any(|e| e.contains("proxy URL")));
        }
    }

    #[test]
    fn validate_accepts_valid_proxy() {
        let cfg = BrowserConfig {
            proxy: Some("socks5://user:pass@127.0.0.1:1080".to_string()),
            ..BrowserConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn to_json_and_from_json_str_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = BrowserConfig::builder()
            .headless(false)
            .stealth_level(StealthLevel::Basic)
            .build();
        let json = cfg.to_json()?;
        assert!(json.contains("headless"));
        let back = BrowserConfig::from_json_str(&json)?;
        assert!(!back.headless);
        assert_eq!(back.stealth_level, StealthLevel::Basic);
        Ok(())
    }

    #[test]
    fn from_json_str_error_on_invalid_json() {
        let err = BrowserConfig::from_json_str("not json at all");
        assert!(err.is_err());
    }

    #[test]
    fn builder_cdp_fix_mode_and_source_url() {
        use crate::cdp_protection::CdpFixMode;
        let cfg = BrowserConfig::builder()
            .cdp_fix_mode(CdpFixMode::IsolatedWorld)
            .source_url(Some("stealth.js".to_string()))
            .build();
        assert_eq!(cfg.cdp_fix_mode, CdpFixMode::IsolatedWorld);
        assert_eq!(cfg.source_url.as_deref(), Some("stealth.js"));
    }

    #[test]
    fn builder_source_url_none_disables_sourceurl() {
        let cfg = BrowserConfig::builder().source_url(None).build();
        assert!(cfg.source_url.is_none());
    }

    // ─── Env-var override tests ────────────────────────────────────────────────
    //
    // These tests set env vars and call BrowserConfig::default() to verify
    // the overrides are picked up.  Tests use a per-test unique var name to
    // prevent cross-test pollution, but the real STYGIAN_* paths are also
    // exercised via a serial test that saves/restores the env.

    #[test]
    fn stealth_level_from_env_none() {
        // env_bool / StealthLevel::from_env are pure functions — we test the
        // conversion logic indirectly via a temporary override.
        temp_env::with_var("STYGIAN_STEALTH_LEVEL", Some("none"), || {
            let level = StealthLevel::from_env();
            assert_eq!(level, StealthLevel::None);
        });
    }

    #[test]
    fn stealth_level_from_env_basic() {
        temp_env::with_var("STYGIAN_STEALTH_LEVEL", Some("basic"), || {
            assert_eq!(StealthLevel::from_env(), StealthLevel::Basic);
        });
    }

    #[test]
    fn stealth_level_from_env_advanced_is_default() {
        temp_env::with_var("STYGIAN_STEALTH_LEVEL", Some("anything_else"), || {
            assert_eq!(StealthLevel::from_env(), StealthLevel::Advanced);
        });
    }

    #[test]
    fn stealth_level_from_env_missing_defaults_to_advanced() {
        // When the key is absent, from_env() falls through to Advanced.
        temp_env::with_var("STYGIAN_STEALTH_LEVEL", None::<&str>, || {
            assert_eq!(StealthLevel::from_env(), StealthLevel::Advanced);
        });
    }

    #[test]
    fn cdp_fix_mode_from_env_variants() {
        use crate::cdp_protection::CdpFixMode;
        let cases = [
            ("add_binding", CdpFixMode::AddBinding),
            ("isolatedworld", CdpFixMode::IsolatedWorld),
            ("enable_disable", CdpFixMode::EnableDisable),
            ("none", CdpFixMode::None),
            ("unknown_value", CdpFixMode::AddBinding), // falls back to default
        ];
        for (val, expected) in cases {
            temp_env::with_var("STYGIAN_CDP_FIX_MODE", Some(val), || {
                assert_eq!(
                    CdpFixMode::from_env(),
                    expected,
                    "STYGIAN_CDP_FIX_MODE={val}"
                );
            });
        }
    }

    #[test]
    fn pool_config_from_env_min_max() {
        temp_env::with_vars(
            [
                ("STYGIAN_POOL_MIN", Some("3")),
                ("STYGIAN_POOL_MAX", Some("15")),
            ],
            || {
                let p = PoolConfig::default();
                assert_eq!(p.min_size, 3);
                assert_eq!(p.max_size, 15);
            },
        );
    }

    #[test]
    fn headless_from_env_false() {
        temp_env::with_var("STYGIAN_HEADLESS", Some("false"), || {
            // env_bool parses the value via BrowserConfig::default()
            assert!(!env_bool("STYGIAN_HEADLESS", true));
        });
    }

    #[test]
    fn headless_from_env_zero_means_false() {
        temp_env::with_var("STYGIAN_HEADLESS", Some("0"), || {
            assert!(!env_bool("STYGIAN_HEADLESS", true));
        });
    }

    #[test]
    fn headless_from_env_no_means_false() {
        temp_env::with_var("STYGIAN_HEADLESS", Some("no"), || {
            assert!(!env_bool("STYGIAN_HEADLESS", true));
        });
    }

    #[test]
    fn validate_accepts_socks4_proxy() {
        let cfg = BrowserConfig {
            proxy: Some("socks4://127.0.0.1:1080".to_string()),
            ..BrowserConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_multiple_errors_returned_together() {
        let cfg = BrowserConfig {
            pool: PoolConfig {
                min_size: 10,
                max_size: 5,
                ..PoolConfig::default()
            },
            launch_timeout: std::time::Duration::ZERO,
            proxy: Some("ftp://bad".to_string()),
            ..BrowserConfig::default()
        };
        let result = cfg.validate();
        assert!(result.is_err());
        if let Err(errors) = result {
            assert!(errors.len() >= 3, "expected ≥3 errors, got: {errors:?}");
        }
    }

    #[test]
    fn json_file_error_on_missing_file() {
        let result = BrowserConfig::from_json_file("/nonexistent/path/config.json");
        assert!(result.is_err());
        if let Err(e) = result {
            let err_str = e.to_string();
            assert!(err_str.contains("cannot read config file") || err_str.contains("config"));
        }
    }

    #[test]
    fn json_roundtrip_preserves_cdp_fix_mode() -> Result<(), Box<dyn std::error::Error>> {
        use crate::cdp_protection::CdpFixMode;
        let cfg = BrowserConfig::builder()
            .cdp_fix_mode(CdpFixMode::EnableDisable)
            .build();
        let json = cfg.to_json()?;
        let back = BrowserConfig::from_json_str(&json)?;
        assert_eq!(back.cdp_fix_mode, CdpFixMode::EnableDisable);
        Ok(())
    }
}

// ─── temp_env helper (test-only) ─────────────────────────────────────────────
//
// Lightweight env-var scoping without an external dep.  Uses std::env +
// cleanup to isolate side effects.

#[cfg(test)]
#[allow(unsafe_code)] // env::set_var / remove_var are unsafe in Rust ≥1.93; guarded by ENV_LOCK
mod temp_env {
    use std::env;
    use std::ffi::OsStr;
    use std::sync::Mutex;

    // Serialise all env-var mutations so parallel tests don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run `f` with the environment variable `key` set to `value` (or unset if
    /// `None`), then restore the previous value.
    pub fn with_var<K, V, F>(key: K, value: Option<V>, f: F)
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
        F: FnOnce(),
    {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = key.as_ref();
        let prev = env::var_os(key);
        match value {
            Some(v) => unsafe { env::set_var(key, v.as_ref()) },
            None => unsafe { env::remove_var(key) },
        }
        f();
        match prev {
            Some(v) => unsafe { env::set_var(key, v) },
            None => unsafe { env::remove_var(key) },
        }
    }

    /// Run `f` with multiple env vars set/unset simultaneously.
    pub fn with_vars<K, V, F>(pairs: impl IntoIterator<Item = (K, Option<V>)>, f: F)
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
        F: FnOnce(),
    {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pairs: Vec<_> = pairs
            .into_iter()
            .map(|(k, v)| {
                let key = k.as_ref().to_os_string();
                let prev = env::var_os(&key);
                let new_val = v.map(|v| v.as_ref().to_os_string());
                (key, prev, new_val)
            })
            .collect();

        for (key, _, new_val) in &pairs {
            match new_val {
                Some(v) => unsafe { env::set_var(key, v) },
                None => unsafe { env::remove_var(key) },
            }
        }

        f();

        for (key, prev, _) in &pairs {
            match prev {
                Some(v) => unsafe { env::set_var(key, v) },
                None => unsafe { env::remove_var(key) },
            }
        }
    }
}
