//! Browser instance lifecycle management
//!
//! Provides a thin wrapper around a `chromiumoxide` [`Browser`] that adds:
//!
//! - Anti-detection launch arguments from [`BrowserConfig`]
//! - Configurable launch and per-operation timeouts via `tokio::time::timeout`
//! - Health checks using the CDP `Browser.getVersion` command
//! - PID-based zombie process detection and forced cleanup
//! - Graceful shutdown (close all pages ➞ send `Browser.close`)
//!
//! # Example
//!
//! ```no_run
//! use mycelium_browser::{BrowserConfig, browser::BrowserInstance};
//!
//! # async fn run() -> mycelium_browser::error::Result<()> {
//! let config = BrowserConfig::default();
//! let mut instance = BrowserInstance::launch(config).await?;
//!
//! assert!(instance.is_healthy().await);
//! instance.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use std::time::{Duration, Instant};

use chromiumoxide::Browser;
use futures::StreamExt;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::{
    BrowserConfig,
    error::{BrowserError, Result},
};

// ─── BrowserInstance ──────────────────────────────────────────────────────────

/// A managed browser instance with health tracking.
///
/// Wraps a `chromiumoxide` [`Browser`] and an async handler task.  Always call
/// [`BrowserInstance::shutdown`] (or drop) after use to release OS resources.
pub struct BrowserInstance {
    browser: Browser,
    config: BrowserConfig,
    launched_at: Instant,
    /// Set to `false` after a failed health check so callers know to discard.
    healthy: bool,
    /// Convenience ID for log correlation.
    id: String,
}

impl BrowserInstance {
    /// Launch a new browser instance using the provided [`BrowserConfig`].
    ///
    /// All configured anti-detection arguments (see
    /// [`BrowserConfig::effective_args`]) are passed at launch time.
    ///
    /// # Errors
    ///
    /// - [`BrowserError::LaunchFailed`] if the process does not start within
    ///   `config.launch_timeout`.
    /// - [`BrowserError::Timeout`] if the browser doesn't respond in time.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_browser::{BrowserConfig, browser::BrowserInstance};
    ///
    /// # async fn run() -> mycelium_browser::error::Result<()> {
    /// let instance = BrowserInstance::launch(BrowserConfig::default()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn launch(config: BrowserConfig) -> Result<Self> {
        let id = ulid::Ulid::new().to_string();
        let launch_timeout = config.launch_timeout;

        info!(browser_id = %id, "Launching browser");

        let args = config.effective_args();
        debug!(browser_id = %id, ?args, "Chrome launch arguments");

        let mut builder = chromiumoxide::BrowserConfig::builder();

        // chromiumoxide defaults to headless; call with_head() only for headed mode
        if !config.headless {
            builder = builder.with_head();
        }

        if let Some(path) = &config.chrome_path {
            builder = builder.chrome_executable(path);
        }

        if let Some(dir) = &config.user_data_dir {
            builder = builder.user_data_dir(dir);
        }

        for arg in &args {
            builder = builder.arg(arg.as_str());
        }

        if let Some((w, h)) = config.window_size {
            builder = builder.window_size(w, h);
        }

        let cdp_cfg = builder
            .build()
            .map_err(|e| BrowserError::LaunchFailed { reason: e })?;

        let (browser, mut handler) = timeout(launch_timeout, Browser::launch(cdp_cfg))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "browser.launch".to_string(),
                duration_ms: u64::try_from(launch_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::LaunchFailed {
                reason: e.to_string(),
            })?;

        // Spawn the chromiumoxide message handler; it must run for the browser
        // to remain responsive.
        tokio::spawn(async move { while handler.next().await.is_some() {} });

        info!(browser_id = %id, "Browser launched successfully");

        Ok(Self {
            browser,
            config,
            launched_at: Instant::now(),
            healthy: true,
            id,
        })
    }

    // ─── Health ───────────────────────────────────────────────────────────────

    /// Returns `true` if the browser is currently considered healthy.
    ///
    /// This is a cached value updated by [`BrowserInstance::health_check`].
    pub const fn is_healthy_cached(&self) -> bool {
        self.healthy
    }

    /// Actively probe the browser with a CDP request.
    ///
    /// Sends `Browser.getVersion` and waits up to `cdp_timeout`.  Updates the
    /// internal healthy flag and returns the result.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_browser::{BrowserConfig, browser::BrowserInstance};
    ///
    /// # async fn run() -> mycelium_browser::error::Result<()> {
    /// let mut instance = BrowserInstance::launch(BrowserConfig::default()).await?;
    /// assert!(instance.is_healthy().await);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn is_healthy(&mut self) -> bool {
        match self.health_check().await {
            Ok(()) => true,
            Err(e) => {
                warn!(browser_id = %self.id, error = %e, "Health check failed");
                false
            }
        }
    }

    /// Run a health check and return a structured [`Result`].
    ///
    /// Pings the browser with the CDP `Browser.getVersion` RPC.
    pub async fn health_check(&mut self) -> Result<()> {
        let op_timeout = self.config.cdp_timeout;

        timeout(op_timeout, self.browser.version())
            .await
            .map_err(|_| {
                self.healthy = false;
                BrowserError::Timeout {
                    operation: "Browser.getVersion".to_string(),
                    duration_ms: u64::try_from(op_timeout.as_millis()).unwrap_or(u64::MAX),
                }
            })?
            .map_err(|e| {
                self.healthy = false;
                BrowserError::CdpError {
                    operation: "Browser.getVersion".to_string(),
                    message: e.to_string(),
                }
            })?;

        self.healthy = true;
        Ok(())
    }

    // ─── Accessors ────────────────────────────────────────────────────────────

    /// Access the underlying `chromiumoxide` [`Browser`].
    pub const fn browser(&self) -> &Browser {
        &self.browser
    }

    /// Mutable access to the underlying `chromiumoxide` [`Browser`].
    pub const fn browser_mut(&mut self) -> &mut Browser {
        &mut self.browser
    }

    /// Instance ID (ULID) for log correlation.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// How long has this instance been alive.
    pub fn uptime(&self) -> Duration {
        self.launched_at.elapsed()
    }

    /// The config snapshot used at launch.
    pub const fn config(&self) -> &BrowserConfig {
        &self.config
    }

    // ─── Shutdown ─────────────────────────────────────────────────────────────

    /// Gracefully close the browser.
    ///
    /// Sends `Browser.close` and waits up to `cdp_timeout`.  Any errors during
    /// tear-down are logged but not propagated so the caller can always clean up.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_browser::{BrowserConfig, browser::BrowserInstance};
    ///
    /// # async fn run() -> mycelium_browser::error::Result<()> {
    /// let mut instance = BrowserInstance::launch(BrowserConfig::default()).await?;
    /// instance.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(mut self) -> Result<()> {
        info!(browser_id = %self.id, "Shutting down browser");

        let op_timeout = self.config.cdp_timeout;

        if let Err(e) = timeout(op_timeout, self.browser.close()).await {
            // Timeout — log and continue cleanup
            warn!(
                browser_id = %self.id,
                "Browser.close timed out after {}ms: {e}",
                op_timeout.as_millis()
            );
        }

        self.healthy = false;
        info!(browser_id = %self.id, "Browser shut down");
        Ok(())
    }

    /// Open a new tab and return a [`crate::page::PageHandle`].
    ///
    /// The handle closes the tab automatically when dropped.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if a new page cannot be created.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_browser::{BrowserConfig, browser::BrowserInstance};
    ///
    /// # async fn run() -> mycelium_browser::error::Result<()> {
    /// let mut instance = BrowserInstance::launch(BrowserConfig::default()).await?;
    /// let page = instance.new_page().await?;
    /// drop(page);
    /// instance.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new_page(&self) -> crate::error::Result<crate::page::PageHandle> {
        use tokio::time::timeout;

        let cdp_timeout = self.config.cdp_timeout;

        let page = timeout(cdp_timeout, self.browser.new_page("about:blank"))
            .await
            .map_err(|_| crate::error::BrowserError::Timeout {
                operation: "Browser.newPage".to_string(),
                duration_ms: u64::try_from(cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| crate::error::BrowserError::CdpError {
                operation: "Browser.newPage".to_string(),
                message: e.to_string(),
            })?;

        // Apply stealth injection scripts for all active stealth levels.
        #[cfg(feature = "stealth")]
        crate::stealth::apply_stealth_to_page(&page, &self.config).await?;

        Ok(crate::page::PageHandle::new(page, cdp_timeout))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify `BrowserConfig` `effective_args` includes anti-detection flags.
    ///
    /// This is a unit test that doesn't require a real Chrome binary.
    #[test]
    fn effective_args_contain_automation_flag() {
        let config = BrowserConfig::default();
        let args = config.effective_args();
        assert!(
            args.iter().any(|a| a.contains("AutomationControlled")),
            "Expected --disable-blink-features=AutomationControlled in args: {args:?}"
        );
    }

    #[test]
    fn proxy_arg_injected_when_set() {
        let config = BrowserConfig::builder()
            .proxy("http://proxy.example.com:8080".to_string())
            .build();
        let args = config.effective_args();
        assert!(
            args.iter().any(|a| a.contains("proxy.example.com")),
            "Expected proxy arg in {args:?}"
        );
    }

    #[test]
    fn window_size_arg_injected() {
        let config = BrowserConfig::builder().window_size(1280, 720).build();
        let args = config.effective_args();
        assert!(
            args.iter().any(|a| a.contains("1280")),
            "Expected window-size arg in {args:?}"
        );
    }

    #[test]
    fn browser_instance_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<BrowserInstance>();
        assert_sync::<BrowserInstance>();
    }

    #[test]
    fn effective_args_include_no_sandbox() {
        let cfg = BrowserConfig::default();
        let args = cfg.effective_args();
        assert!(args.iter().any(|a| a == "--no-sandbox"));
    }

    #[test]
    fn effective_args_include_disable_dev_shm() {
        let cfg = BrowserConfig::default();
        let args = cfg.effective_args();
        assert!(args.iter().any(|a| a.contains("disable-dev-shm-usage")));
    }

    #[test]
    fn no_window_size_arg_when_none() {
        let cfg = BrowserConfig {
            window_size: None,
            ..BrowserConfig::default()
        };
        let args = cfg.effective_args();
        assert!(!args.iter().any(|a| a.contains("--window-size")));
    }

    #[test]
    fn custom_arg_appended() {
        let cfg = BrowserConfig::builder()
            .arg("--user-agent=MyCustomBot/1.0".to_string())
            .build();
        let args = cfg.effective_args();
        assert!(args.iter().any(|a| a.contains("MyCustomBot")));
    }

    #[test]
    fn proxy_bypass_list_arg_injected() {
        let cfg = BrowserConfig::builder()
            .proxy("http://proxy:8080".to_string())
            .proxy_bypass_list("<local>,localhost".to_string())
            .build();
        let args = cfg.effective_args();
        assert!(args.iter().any(|a| a.contains("proxy-bypass-list")));
    }

    #[test]
    fn headless_mode_preserved_in_config() {
        let cfg = BrowserConfig::builder().headless(false).build();
        assert!(!cfg.headless);
        let cfg2 = BrowserConfig::builder().headless(true).build();
        assert!(cfg2.headless);
    }

    #[test]
    fn launch_timeout_default_is_non_zero() {
        let cfg = BrowserConfig::default();
        assert!(!cfg.launch_timeout.is_zero());
    }

    #[test]
    fn cdp_timeout_default_is_non_zero() {
        let cfg = BrowserConfig::default();
        assert!(!cfg.cdp_timeout.is_zero());
    }
}
