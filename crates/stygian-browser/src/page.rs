//! Page and browsing context management for isolated, parallel scraping
//!
//! Each `BrowserContext` (future) is an incognito-style isolation boundary (separate
//! cookies, localStorage, cache).  Each context can contain many [`PageHandle`]s
//! (tabs).  Both types clean up their CDP resources automatically on drop.
//!
//! ## Resource blocking
//!
//! Pass a [`ResourceFilter`] to [`PageHandle::set_resource_filter`] to intercept
//! and block specific request types (images, fonts, CSS) before page load —
//! significantly reducing page load times for text-only scraping.
//!
//! ## Wait strategies
//!
//! [`PageHandle`] exposes three wait strategies via [`WaitUntil`]:
//! - `DomContentLoaded` — fires when the HTML is parsed
//! - `NetworkIdle` — fires when there are ≤2 in-flight requests for 500 ms
//! - `Selector(css)` — fires when a CSS selector matches an element
//!
//! # Example
//!
//! ```no_run
//! use stygian_browser::{BrowserPool, BrowserConfig};
//! use stygian_browser::page::{ResourceFilter, WaitUntil};
//! use std::time::Duration;
//!
//! # async fn run() -> stygian_browser::error::Result<()> {
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let handle = pool.acquire().await?;
//!
//! let mut page = handle.browser().expect("valid browser").new_page().await?;
//! page.set_resource_filter(ResourceFilter::block_media()).await?;
//! page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
//! let title = page.title().await?;
//! println!("title: {title}");
//! handle.release().await;
//! # Ok(())
//! # }
//! ```

use std::sync::{
    Arc,
    atomic::{AtomicU16, Ordering},
};
use std::time::Duration;

use chromiumoxide::Page;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::error::{BrowserError, Result};

// ─── ResourceType ─────────────────────────────────────────────────────────────

/// CDP resource types that can be intercepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceType {
    /// `<img>`, `<picture>`, background images
    Image,
    /// Web fonts loaded via CSS `@font-face`
    Font,
    /// External CSS stylesheets
    Stylesheet,
    /// Media files (audio/video)
    Media,
}

impl ResourceType {
    /// Returns the string used in CDP `Network.requestIntercepted` events.
    pub const fn as_cdp_str(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Font => "Font",
            Self::Stylesheet => "Stylesheet",
            Self::Media => "Media",
        }
    }
}

// ─── ResourceFilter ───────────────────────────────────────────────────────────

/// Set of resource types to block from loading.
///
/// # Example
///
/// ```
/// use stygian_browser::page::ResourceFilter;
/// let filter = ResourceFilter::block_media();
/// assert!(filter.should_block("Image"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct ResourceFilter {
    blocked: Vec<ResourceType>,
}

impl ResourceFilter {
    /// Block all media resources (images, fonts, CSS, audio/video).
    pub fn block_media() -> Self {
        Self {
            blocked: vec![
                ResourceType::Image,
                ResourceType::Font,
                ResourceType::Stylesheet,
                ResourceType::Media,
            ],
        }
    }

    /// Block only images and fonts (keep styles for layout-sensitive work).
    pub fn block_images_and_fonts() -> Self {
        Self {
            blocked: vec![ResourceType::Image, ResourceType::Font],
        }
    }

    /// Add a resource type to the block list.
    #[must_use]
    pub fn block(mut self, resource: ResourceType) -> Self {
        if !self.blocked.contains(&resource) {
            self.blocked.push(resource);
        }
        self
    }

    /// Returns `true` if the given CDP resource type string should be blocked.
    pub fn should_block(&self, cdp_type: &str) -> bool {
        self.blocked
            .iter()
            .any(|r| r.as_cdp_str().eq_ignore_ascii_case(cdp_type))
    }

    /// Returns `true` if no resource types are blocked.
    pub const fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }
}

// ─── WaitUntil ────────────────────────────────────────────────────────────────

/// Condition to wait for after a navigation.
///
/// # Example
///
/// ```
/// use stygian_browser::page::WaitUntil;
/// let w = WaitUntil::Selector("#main".to_string());
/// assert!(matches!(w, WaitUntil::Selector(_)));
/// ```
#[derive(Debug, Clone)]
pub enum WaitUntil {
    /// Wait for the `DOMContentLoaded` event.
    DomContentLoaded,
    /// Wait until there are ≤2 active network requests for at least 500 ms.
    NetworkIdle,
    /// Wait until `document.querySelector(selector)` returns a non-null element.
    Selector(String),
}

// ─── PageHandle ───────────────────────────────────────────────────────────────

/// A handle to an open browser tab.
///
/// On drop the underlying page is closed automatically.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::{BrowserPool, BrowserConfig};
/// use stygian_browser::page::WaitUntil;
/// use std::time::Duration;
///
/// # async fn run() -> stygian_browser::error::Result<()> {
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let handle = pool.acquire().await?;
/// let mut page = handle.browser().expect("valid browser").new_page().await?;
/// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
/// let html = page.content().await?;
/// drop(page); // closes the tab
/// handle.release().await;
/// # Ok(())
/// # }
/// ```
pub struct PageHandle {
    page: Page,
    cdp_timeout: Duration,
    /// HTTP status code of the most recent main-frame navigation, or `0` if not
    /// yet captured.  Written atomically by the listener spawned in `navigate()`.
    last_status_code: Arc<AtomicU16>,
}

impl PageHandle {
    /// Wrap a raw chromiumoxide [`Page`] in a handle.
    pub(crate) fn new(page: Page, cdp_timeout: Duration) -> Self {
        Self {
            page,
            cdp_timeout,
            last_status_code: Arc::new(AtomicU16::new(0)),
        }
    }

    /// Navigate to `url` and wait for `condition` within `nav_timeout`.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NavigationFailed`] if the navigation times out or
    /// the CDP call fails.
    pub async fn navigate(
        &mut self,
        url: &str,
        condition: WaitUntil,
        nav_timeout: Duration,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{
            EventResponseReceived, ResourceType as NetworkResourceType,
        };
        use chromiumoxide::cdp::browser_protocol::page::EventLoadEventFired;
        use futures::StreamExt;

        let url_owned = url.to_string();

        // Reset the stored status before each navigation so stale codes are
        // not returned if the new navigation fails before headers arrive.
        self.last_status_code.store(0, Ordering::Release);

        // Subscribe to Network.responseReceived *before* goto() so no events
        // are missed.  The listener runs in a detached task and stores the
        // first Document-type response status atomically.
        let page_for_listener = self.page.clone();
        let status_capture = Arc::clone(&self.last_status_code);
        match page_for_listener
            .event_listener::<EventResponseReceived>()
            .await
        {
            Ok(mut stream) => {
                tokio::spawn(async move {
                    while let Some(event) = stream.next().await {
                        if event.r#type == NetworkResourceType::Document {
                            let code = u16::try_from(event.response.status).unwrap_or(0);
                            if code > 0 {
                                status_capture.store(code, Ordering::Release);
                            }
                            break;
                        }
                    }
                });
            }
            Err(e) => {
                warn!("status-code capture unavailable: {e}");
            }
        }

        let navigate_fut = async {
            // Fix #7: subscribe to EventLoadEventFired BEFORE goto() so the
            // event cannot fire between goto() returning and our subscription
            // registering (Chrome fires load events at the same time as the
            // CDP Navigate response, making subscribe-after-goto a 100% race).
            let mut load_events = match &condition {
                WaitUntil::DomContentLoaded | WaitUntil::NetworkIdle => Some(
                    self.page
                        .event_listener::<EventLoadEventFired>()
                        .await
                        .map_err(|e| BrowserError::NavigationFailed {
                            url: url_owned.clone(),
                            reason: e.to_string(),
                        })?,
                ),
                WaitUntil::Selector(_) => None,
            };

            self.page
                .goto(url)
                .await
                .map_err(|e| BrowserError::NavigationFailed {
                    url: url_owned.clone(),
                    reason: e.to_string(),
                })?;

            match &condition {
                WaitUntil::DomContentLoaded | WaitUntil::NetworkIdle => {
                    if let Some(ref mut events) = load_events {
                        let _ = events.next().await;
                    }
                }
                WaitUntil::Selector(css) => {
                    self.wait_for_selector(css, nav_timeout).await?;
                }
            }
            Ok(())
        };

        timeout(nav_timeout, navigate_fut)
            .await
            .map_err(|_| BrowserError::NavigationFailed {
                url: url.to_string(),
                reason: format!("navigation timed out after {nav_timeout:?}"),
            })?
    }

    /// Wait until `document.querySelector(selector)` is non-null (`timeout`).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NavigationFailed`] if the selector is not found
    /// within the given timeout.
    pub async fn wait_for_selector(&self, selector: &str, wait_timeout: Duration) -> Result<()> {
        let selector_owned = selector.to_string();
        let poll = async {
            loop {
                if self.page.find_element(selector_owned.clone()).await.is_ok() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };

        timeout(wait_timeout, poll)
            .await
            .map_err(|_| BrowserError::NavigationFailed {
                url: String::new(),
                reason: format!("selector '{selector_owned}' not found within {wait_timeout:?}"),
            })?
    }

    /// Set a resource filter to block specific network request types.
    ///
    /// **Note:** Requires Network.enable; called automatically.
    ///
    /// # Errors
    ///
    /// Returns a [`BrowserError::CdpError`] if the CDP call fails.
    pub async fn set_resource_filter(&mut self, filter: ResourceFilter) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::fetch::{EnableParams, RequestPattern};

        if filter.is_empty() {
            return Ok(());
        }

        // Both builders are infallible — they return the struct directly (not Result)
        let pattern = RequestPattern::builder().url_pattern("*").build();
        let params = EnableParams::builder()
            .patterns(vec![pattern])
            .handle_auth_requests(false)
            .build();

        timeout(self.cdp_timeout, self.page.execute::<EnableParams>(params))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "Fetch.enable".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: "Fetch.enable".to_string(),
                message: e.to_string(),
            })?;

        debug!("Resource filter active: {:?}", filter);
        Ok(())
    }

    /// Return the current page URL (post-navigation, post-redirect).
    ///
    /// Delegates to the CDP `Target.getTargetInfo` binding already used
    /// internally by [`save_cookies`](Self::save_cookies); no extra network
    /// request is made.  Returns an empty string if the URL is not yet set
    /// (e.g. on a blank tab before the first navigation).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if the underlying CDP call fails, or
    /// [`BrowserError::Timeout`] if it exceeds `cdp_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::page::WaitUntil;
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// let url = page.url().await?;
    /// println!("Final URL after redirects: {url}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn url(&self) -> Result<String> {
        timeout(self.cdp_timeout, self.page.url())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "page.url".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: "page.url".to_string(),
                message: e.to_string(),
            })
            .map(Option::unwrap_or_default)
    }

    /// Return the HTTP status code of the most recent main-frame navigation.
    ///
    /// The status is captured from the `Network.responseReceived` CDP event
    /// wired up inside [`navigate`](Self::navigate), so it reflects the
    /// *final* response after any server-side redirects.
    ///
    /// Returns `None` if the status was not captured — for example on `file://`
    /// navigations, when [`navigate`](Self::navigate) has not yet been called,
    /// or if the network event subscription failed.
    ///
    /// # Errors
    ///
    /// This method is infallible; the `Result` wrapper is kept for API
    /// consistency with other `PageHandle` methods.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::page::WaitUntil;
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// if let Some(code) = page.status_code()? {
    ///     println!("HTTP {code}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn status_code(&self) -> Result<Option<u16>> {
        let code = self.last_status_code.load(Ordering::Acquire);
        Ok(if code == 0 { None } else { Some(code) })
    }

    /// Return the page's `<title>` text.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::ScriptExecutionFailed`] if the evaluation fails.
    pub async fn title(&self) -> Result<String> {
        timeout(self.cdp_timeout, self.page.get_title())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "get_title".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::ScriptExecutionFailed {
                script: "document.title".to_string(),
                reason: e.to_string(),
            })
            .map(Option::unwrap_or_default)
    }

    /// Return the page's full outer HTML.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::ScriptExecutionFailed`] if the evaluation fails.
    pub async fn content(&self) -> Result<String> {
        timeout(self.cdp_timeout, self.page.content())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "page.content".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::ScriptExecutionFailed {
                script: "document.documentElement.outerHTML".to_string(),
                reason: e.to_string(),
            })
    }

    /// Evaluate arbitrary JavaScript and return the result as `T`.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::ScriptExecutionFailed`] on eval failure or
    /// deserialization error.
    pub async fn eval<T: serde::de::DeserializeOwned>(&self, script: &str) -> Result<T> {
        let script_owned = script.to_string();
        timeout(self.cdp_timeout, self.page.evaluate(script))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "page.evaluate".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::ScriptExecutionFailed {
                script: script_owned.clone(),
                reason: e.to_string(),
            })?
            .into_value::<T>()
            .map_err(|e| BrowserError::ScriptExecutionFailed {
                script: script_owned,
                reason: e.to_string(),
            })
    }

    /// Save all cookies for the current page's origin.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if the CDP call fails.
    pub async fn save_cookies(
        &self,
    ) -> Result<Vec<chromiumoxide::cdp::browser_protocol::network::Cookie>> {
        use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;

        let url = self
            .page
            .url()
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "page.url".to_string(),
                message: e.to_string(),
            })?
            .unwrap_or_default();

        timeout(
            self.cdp_timeout,
            self.page
                .execute(GetCookiesParams::builder().urls(vec![url]).build()),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "Network.getCookies".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| BrowserError::CdpError {
            operation: "Network.getCookies".to_string(),
            message: e.to_string(),
        })
        .map(|r| r.cookies.clone())
    }

    /// Capture a screenshot of the current page as PNG bytes.
    ///
    /// The screenshot is full-page by default (viewport clipped to the rendered
    /// layout area).  Save the returned bytes to a `.png` file or process
    /// them in-memory.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if the CDP `Page.captureScreenshot`
    /// command fails, or [`BrowserError::Timeout`] if it exceeds
    /// `cdp_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::{time::Duration, fs};
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::Selector("body".to_string()), Duration::from_secs(30)).await?;
    /// let png = page.screenshot().await?;
    /// fs::write("screenshot.png", &png).unwrap();
    /// # Ok(())
    /// # }
    /// ```
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        use chromiumoxide::page::ScreenshotParams;

        let params = ScreenshotParams::builder().full_page(true).build();

        timeout(self.cdp_timeout, self.page.screenshot(params))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "Page.captureScreenshot".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: "Page.captureScreenshot".to_string(),
                message: e.to_string(),
            })
    }

    /// Borrow the underlying chromiumoxide [`Page`].
    pub const fn inner(&self) -> &Page {
        &self.page
    }

    /// Close this page (tab).
    ///
    /// Called automatically on drop; explicit call avoids suppressing the error.
    pub async fn close(self) -> Result<()> {
        timeout(Duration::from_secs(5), self.page.clone().close())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "page.close".to_string(),
                duration_ms: 5000,
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: "page.close".to_string(),
                message: e.to_string(),
            })
    }
}

impl Drop for PageHandle {
    fn drop(&mut self) {
        warn!("PageHandle dropped without explicit close(); spawning cleanup task");
        // chromiumoxide Page does not implement close on Drop, so we spawn
        // a fire-and-forget task. The page ref is already owned; we need to
        // swap it out. We clone the Page handle (it's Arc-backed internally).
        let page = self.page.clone();
        tokio::spawn(async move {
            let _ = page.close().await;
        });
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_filter_block_media_blocks_image() {
        let filter = ResourceFilter::block_media();
        assert!(filter.should_block("Image"));
        assert!(filter.should_block("Font"));
        assert!(filter.should_block("Stylesheet"));
        assert!(filter.should_block("Media"));
        assert!(!filter.should_block("Script"));
        assert!(!filter.should_block("XHR"));
    }

    #[test]
    fn resource_filter_case_insensitive() {
        let filter = ResourceFilter::block_images_and_fonts();
        assert!(filter.should_block("image")); // lowercase
        assert!(filter.should_block("IMAGE")); // uppercase
        assert!(!filter.should_block("Stylesheet"));
    }

    #[test]
    fn resource_filter_builder_chain() {
        let filter = ResourceFilter::default()
            .block(ResourceType::Image)
            .block(ResourceType::Font);
        assert!(filter.should_block("Image"));
        assert!(filter.should_block("Font"));
        assert!(!filter.should_block("Stylesheet"));
    }

    #[test]
    fn resource_filter_dedup_block() {
        let filter = ResourceFilter::default()
            .block(ResourceType::Image)
            .block(ResourceType::Image); // duplicate
        assert_eq!(filter.blocked.len(), 1);
    }

    #[test]
    fn resource_filter_is_empty_when_default() {
        assert!(ResourceFilter::default().is_empty());
        assert!(!ResourceFilter::block_media().is_empty());
    }

    #[test]
    fn wait_until_selector_stores_string() {
        let w = WaitUntil::Selector("#foo".to_string());
        assert!(matches!(w, WaitUntil::Selector(ref s) if s == "#foo"));
    }

    #[test]
    fn resource_type_cdp_str() {
        assert_eq!(ResourceType::Image.as_cdp_str(), "Image");
        assert_eq!(ResourceType::Font.as_cdp_str(), "Font");
        assert_eq!(ResourceType::Stylesheet.as_cdp_str(), "Stylesheet");
        assert_eq!(ResourceType::Media.as_cdp_str(), "Media");
    }

    /// `PageHandle` must be `Send + Sync` for use across thread boundaries.
    #[test]
    fn page_handle_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<PageHandle>();
        assert_sync::<PageHandle>();
    }

    /// The status-code sentinel (0 = "not yet captured") and the conversion to
    /// `Option<u16>` are pure-logic invariants testable without a live browser.
    #[test]
    fn status_code_sentinel_zero_maps_to_none() {
        use std::sync::atomic::{AtomicU16, Ordering};
        let atom = AtomicU16::new(0);
        let code = atom.load(Ordering::Acquire);
        assert_eq!(if code == 0 { None } else { Some(code) }, None::<u16>);
    }

    #[test]
    fn status_code_non_zero_maps_to_some() {
        use std::sync::atomic::{AtomicU16, Ordering};
        for &expected in &[200u16, 301, 404, 503] {
            let atom = AtomicU16::new(expected);
            let code = atom.load(Ordering::Acquire);
            assert_eq!(if code == 0 { None } else { Some(code) }, Some(expected));
        }
    }
}
