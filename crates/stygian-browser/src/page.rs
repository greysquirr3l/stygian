//!
//! ## Resource blocking
//!
//! ## Wait strategies
//!
//! [`PageHandle`] exposes three wait strategies via [`WaitUntil`]:
//! - `DomContentLoaded` — fires when the HTML is parsed
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

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU16, Ordering},
};
use std::time::Duration;

use chromiumoxide::Page;
use serde::{Deserialize, Serialize};
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

    pub fn block_images_and_fonts() -> Self {
        Self {
            blocked: vec![ResourceType::Image, ResourceType::Font],
        }
    }

    #[must_use]
    pub fn block(mut self, resource: ResourceType) -> Self {
        if !self.blocked.contains(&resource) {
            self.blocked.push(resource);
        }
        self
    }

    pub fn should_block(&self, cdp_type: &str) -> bool {
        self.blocked
            .iter()
            .any(|r| r.as_cdp_str().eq_ignore_ascii_case(cdp_type))
    }

    pub const fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }
}

// ─── WaitUntil ────────────────────────────────────────────────────────────────

///
/// # Example
///
/// ```
/// use stygian_browser::page::WaitUntil;
/// ```
/// Specifies what condition to wait for after a page navigation.
#[derive(Debug, Clone)]
pub enum WaitUntil {
    /// Fires when the initial HTML is fully parsed, without waiting for
    /// subresources such as images and stylesheets to finish loading.
    DomContentLoaded,
    NetworkIdle,
    Selector(String),
}

// ─── NodeHandle ───────────────────────────────────────────────────────────────

///
/// more CDP `Runtime.callFunctionOn` calls against the held V8 remote object
/// reference — no HTML serialisation occurs.
///
/// A handle becomes **stale** after page navigation or if the underlying DOM
/// node is removed.  Stale calls return [`BrowserError::StaleNode`] so callers
/// can distinguish them from other CDP failures.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
/// use std::time::Duration;
///
/// # async fn run() -> stygian_browser::error::Result<()> {
/// let pool = BrowserPool::new(BrowserConfig::default()).await?;
/// let handle = pool.acquire().await?;
/// let mut page = handle.browser().expect("valid browser").new_page().await?;
/// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
/// # let nodes = page.query_selector_all("a").await?;
/// # for node in &nodes {
///     let href = node.attr("href").await?;
///     let text = node.text_content().await?;
///     println!("{text}: {href:?}");
/// # }
/// # Ok(())
/// # }
/// ```
pub struct NodeHandle {
    element: chromiumoxide::element::Element,
    /// Shared via `Arc<str>` so all handles from a single query reuse the
    /// same allocation rather than cloning a `String` per node.
    selector: Arc<str>,
    cdp_timeout: Duration,
    /// during DOM traversal (parent / sibling navigation).
    page: chromiumoxide::Page,
}

impl NodeHandle {
    /// Return a single attribute value, or `None` if the attribute is absent.
    ///
    /// Issues one `Runtime.callFunctionOn` CDP call (`el.getAttribute(name)`).
    ///
    /// # Errors
    ///
    /// invalidated, or [`BrowserError::Timeout`] / [`BrowserError::CdpError`]
    /// on transport-level failures.
    pub async fn attr(&self, name: &str) -> Result<Option<String>> {
        timeout(self.cdp_timeout, self.element.attribute(name))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::attr".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "attr"))
    }

    /// Return all attributes as a `HashMap<name, value>` in a **single**
    /// CDP round-trip.
    ///
    /// Uses `DOM.getAttributes` (via the chromiumoxide `attributes()` API)
    /// which returns a flat `[name, value, name, value, …]` list from the node
    /// description — no per-attribute calls are needed.
    ///
    /// # Errors
    ///
    /// invalidated.
    pub async fn attr_map(&self) -> Result<HashMap<String, String>> {
        let flat = timeout(self.cdp_timeout, self.element.attributes())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::attr_map".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "attr_map"))?;

        let mut map = HashMap::with_capacity(flat.len() / 2);
        for pair in flat.chunks_exact(2) {
            if let [name, value] = pair {
                map.insert(name.clone(), value.clone());
            }
        }
        Ok(map)
    }

    /// Return the element's `textContent` (all text inside, no markup).
    ///
    /// Reads the DOM `textContent` property via a single JS eval — this is the
    /// raw text concatenation of all descendant text nodes, independent of
    /// layout or visibility (unlike `innerText`).
    ///
    ///
    /// # Errors
    ///
    /// invalidated.
    pub async fn text_content(&self) -> Result<String> {
        let returns = timeout(
            self.cdp_timeout,
            self.element
                .call_js_fn(r"function() { return this.textContent ?? ''; }", true),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "NodeHandle::text_content".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| self.cdp_err_or_stale(&e, "text_content"))?;

        Ok(returns
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// Return the element's `innerHTML`.
    ///
    ///
    /// # Errors
    ///
    /// invalidated.
    pub async fn inner_html(&self) -> Result<String> {
        timeout(self.cdp_timeout, self.element.inner_html())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::inner_html".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "inner_html"))
            .map(Option::unwrap_or_default)
    }

    /// Return the element's `outerHTML`.
    ///
    ///
    /// # Errors
    ///
    /// invalidated.
    pub async fn outer_html(&self) -> Result<String> {
        timeout(self.cdp_timeout, self.element.outer_html())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::outer_html".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "outer_html"))
            .map(Option::unwrap_or_default)
    }

    ///
    /// Executes a single `Runtime.callFunctionOn` JavaScript function that
    /// walks `parentElement` and collects tag names — no repeated CDP calls.
    ///
    /// ```text
    /// ["p", "article", "body", "html"]
    /// ```
    ///
    /// # Errors
    ///
    /// invalidated, or [`BrowserError::ScriptExecutionFailed`] when CDP
    pub async fn ancestors(&self) -> Result<Vec<String>> {
        let returns = timeout(
            self.cdp_timeout,
            self.element.call_js_fn(
                r"function() {
                    const a = [];
                    let n = this.parentElement;
                    while (n) { a.push(n.tagName.toLowerCase()); n = n.parentElement; }
                    return a;
                }",
                true,
            ),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "NodeHandle::ancestors".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| self.cdp_err_or_stale(&e, "ancestors"))?;

        // With returnByValue=true and an array return, CDP delivers the value
        // as a JSON array directly — no JSON.stringify/re-parse needed.
        // A missing or wrong-type value indicates an unexpected CDP failure.
        let arr = returns
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_array())
            .ok_or_else(|| BrowserError::ScriptExecutionFailed {
                script: "NodeHandle::ancestors".to_string(),
                reason: "CDP returned no value or a non-array value for ancestors()".to_string(),
            })?;

        arr.iter()
            .map(|v| {
                v.as_str().map(ToString::to_string).ok_or_else(|| {
                    BrowserError::ScriptExecutionFailed {
                        script: "NodeHandle::ancestors".to_string(),
                        reason: format!("ancestor entry is not a string: {v}"),
                    }
                })
            })
            .collect()
    }

    ///
    ///
    ///
    /// # Errors
    ///
    /// invalidated, or [`BrowserError::CdpError`] on transport failure.
    pub async fn children_matching(&self, selector: &str) -> Result<Vec<Self>> {
        let elements = timeout(self.cdp_timeout, self.element.find_elements(selector))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::children_matching".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "children_matching"))?;

        let selector_arc: Arc<str> = Arc::from(selector);
        Ok(elements
            .into_iter()
            .map(|el| Self {
                element: el,
                selector: selector_arc.clone(),
                cdp_timeout: self.cdp_timeout,
                page: self.page.clone(),
            })
            .collect())
    }

    /// Return the immediate parent element, or `None` if this element has no
    /// parent (i.e. it is the document root).
    ///
    /// Issues a single `Runtime.callFunctionOn` CDP call that temporarily tags
    /// the parent element with a unique attribute, then resolves it via a
    /// CSS attribute selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails or the page handle is invalidated.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// # let nodes = page.query_selector_all("a").await?;
    /// if let Some(parent) = nodes[0].parent().await? {
    ///     let html = parent.outer_html().await?;
    ///     println!("parent: {}", &html[..html.len().min(80)]);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn parent(&self) -> Result<Option<Self>> {
        let attr = format!(
            "data-stygian-t-{}",
            ulid::Ulid::new().to_string().to_lowercase()
        );
        let js = format!(
            "function() {{ \
                var t = this.parentElement; \
                if (!t) {{ return false; }} \
                t.setAttribute('{attr}', '1'); \
                return true; \
            }}"
        );
        self.call_traversal(&js, &attr, "parent").await
    }

    /// Return the next element sibling, or `None` if this element is the last
    /// child of its parent.
    ///
    /// Uses `nextElementSibling` (skips text/comment nodes).
    ///
    /// # Errors
    ///
    /// invalidated.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// # let nodes = page.query_selector_all("a").await?;
    /// if let Some(next) = nodes[0].next_sibling().await? {
    ///     println!("next sibling: {}", next.text_content().await?);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn next_sibling(&self) -> Result<Option<Self>> {
        let attr = format!(
            "data-stygian-t-{}",
            ulid::Ulid::new().to_string().to_lowercase()
        );
        let js = format!(
            "function() {{ \
                var t = this.nextElementSibling; \
                if (!t) {{ return false; }} \
                t.setAttribute('{attr}', '1'); \
                return true; \
            }}"
        );
        self.call_traversal(&js, &attr, "next").await
    }

    /// Return the previous element sibling, or `None` if this element is the
    /// first child of its parent.
    ///
    /// Uses `previousElementSibling` (skips text/comment nodes).
    ///
    /// # Errors
    ///
    /// invalidated.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// # let nodes = page.query_selector_all("a").await?;
    /// if let Some(prev) = nodes[1].previous_sibling().await? {
    ///     println!("prev sibling: {}", prev.text_content().await?);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn previous_sibling(&self) -> Result<Option<Self>> {
        let attr = format!(
            "data-stygian-t-{}",
            ulid::Ulid::new().to_string().to_lowercase()
        );
        let js = format!(
            "function() {{ \
                var t = this.previousElementSibling; \
                if (!t) {{ return false; }} \
                t.setAttribute('{attr}', '1'); \
                return true; \
            }}"
        );
        self.call_traversal(&js, &attr, "prev").await
    }

    /// Shared traversal implementation used by [`parent`], [`next_sibling`],
    /// and [`previous_sibling`].
    ///
    /// The caller provides a JS function that:
    /// 1. Computes the traversal target (for example, the parent, next
    ///    sibling, or previous sibling) and stores it in a local variable.
    /// 2. If the target is non-null, sets a unique attribute (`attr_name`)
    ///    on it and returns `true`.
    /// 3. Returns `false` when the target is null (no such neighbour).
    ///
    /// This helper then resolves the tagged element from the document root,
    /// removes the temporary attribute, and wraps the result in a
    /// `NodeHandle`.
    ///
    /// [`parent`]: Self::parent
    /// [`next_sibling`]: Self::next_sibling
    /// [`previous_sibling`]: Self::previous_sibling
    async fn call_traversal(
        &self,
        js_fn: &str,
        attr_name: &str,
        selector_suffix: &str,
    ) -> Result<Option<Self>> {
        // Step 1: Run the JS that tags the target element and reports null/non-null.
        let op_tag = format!("NodeHandle::{selector_suffix}::tag");
        let returns = timeout(self.cdp_timeout, self.element.call_js_fn(js_fn, false))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: op_tag.clone(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, selector_suffix))?;

        // JS returns false → no such neighbour.
        let has_target = returns
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !has_target {
            return Ok(None);
        }

        let css = format!("[{attr_name}]");
        let op_resolve = format!("NodeHandle::{selector_suffix}::resolve");
        let element = timeout(self.cdp_timeout, self.page.find_element(css))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: op_resolve.clone(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: op_resolve,
                message: format!("{e:?}"),
            })?;

        // is non-fatal — it leaves a harmless stale attribute in the DOM).
        let cleanup = format!("function() {{ this.removeAttribute('{attr_name}'); }}");
        let _ = element.call_js_fn(cleanup, false).await;

        let new_selector: Arc<str> =
            Arc::from(format!("{}::{selector_suffix}", self.selector).as_str());
        Ok(Some(Self {
            element,
            selector: new_selector,
            cdp_timeout: self.cdp_timeout,
            page: self.page.clone(),
        }))
    }

    /// (when the remote object reference has been invalidated) or
    fn cdp_err_or_stale(
        &self,
        err: &chromiumoxide::error::CdpError,
        operation: &str,
    ) -> BrowserError {
        let msg = format!("{err:?}");
        if msg.contains("Cannot find object with id")
            || msg.contains("context with specified id")
            || msg.contains("Cannot find context")
        {
            BrowserError::StaleNode {
                selector: self.selector.to_string(),
            }
        } else {
            BrowserError::CdpError {
                operation: operation.to_string(),
                message: msg,
            }
        }
    }
}

// ─── PageHandle ───────────────────────────────────────────────────────────────

///
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
    last_status_code: Arc<AtomicU16>,
    /// Background task processing `Fetch.requestPaused` events. Aborted and
    /// replaced each time `set_resource_filter` is called.
    resource_filter_task: Option<tokio::task::JoinHandle<()>>,
}

impl PageHandle {
    /// Wrap a raw chromiumoxide [`Page`] in a handle.
    pub(crate) fn new(page: Page, cdp_timeout: Duration) -> Self {
        Self {
            page,
            cdp_timeout,
            last_status_code: Arc::new(AtomicU16::new(0)),
            resource_filter_task: None,
        }
    }

    ///
    /// # Errors
    ///
    /// the CDP call fails.
    pub async fn navigate(
        &mut self,
        url: &str,
        condition: WaitUntil,
        nav_timeout: Duration,
    ) -> Result<()> {
        self.setup_status_capture().await;
        timeout(
            nav_timeout,
            self.navigate_inner(url, condition, nav_timeout),
        )
        .await
        .map_err(|_| BrowserError::NavigationFailed {
            url: url.to_string(),
            reason: format!("navigation timed out after {nav_timeout:?}"),
        })?
    }

    /// Reset the last status code and wire up the `Network.responseReceived`
    /// so that a missing network domain never blocks navigation.
    async fn setup_status_capture(&self) {
        use chromiumoxide::cdp::browser_protocol::network::{
            EventResponseReceived, ResourceType as NetworkResourceType,
        };
        use futures::StreamExt;

        // Reset so a stale code is not returned if the new navigation fails
        self.last_status_code.store(0, Ordering::Release);

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
            Err(e) => warn!("status-code capture unavailable: {e}"),
        }
    }

    /// described in issue #7.
    async fn navigate_inner(
        &self,
        url: &str,
        condition: WaitUntil,
        nav_timeout: Duration,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::{
            EventDomContentEventFired, EventLoadEventFired,
        };
        use futures::StreamExt;

        let url_owned = url.to_string();

        let mut dom_events = match &condition {
            WaitUntil::DomContentLoaded => Some(
                self.page
                    .event_listener::<EventDomContentEventFired>()
                    .await
                    .map_err(|e| BrowserError::NavigationFailed {
                        url: url_owned.clone(),
                        reason: format!("{e:?}"),
                    })?,
            ),
            _ => None,
        };

        let mut load_events = match &condition {
            WaitUntil::NetworkIdle => Some(
                self.page
                    .event_listener::<EventLoadEventFired>()
                    .await
                    .map_err(|e| BrowserError::NavigationFailed {
                        url: url_owned.clone(),
                        reason: e.to_string(),
                    })?,
            ),
            _ => None,
        };

        let inflight = if matches!(condition, WaitUntil::NetworkIdle) {
            Some(self.subscribe_inflight_counter().await)
        } else {
            None
        };

        self.page
            .goto(url)
            .await
            .map_err(|e| BrowserError::NavigationFailed {
                url: url_owned.clone(),
                reason: e.to_string(),
            })?;

        match &condition {
            WaitUntil::DomContentLoaded => {
                if let Some(ref mut events) = dom_events {
                    let _ = events.next().await;
                }
            }
            WaitUntil::NetworkIdle => {
                if let Some(ref mut events) = load_events {
                    let _ = events.next().await;
                }
                if let Some(ref counter) = inflight {
                    Self::wait_network_idle(counter).await;
                }
            }
            WaitUntil::Selector(css) => {
                self.wait_for_selector(css, nav_timeout).await?;
            }
        }
        Ok(())
    }

    /// Spawn three detached tasks that maintain a signed in-flight request
    /// counter via `Network.requestWillBeSent` (+1) and
    /// `Network.loadingFinished`/`Network.loadingFailed` (−1 each).
    async fn subscribe_inflight_counter(&self) -> Arc<std::sync::atomic::AtomicI32> {
        use std::sync::atomic::AtomicI32;

        use chromiumoxide::cdp::browser_protocol::network::{
            EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
        };
        use futures::StreamExt;

        let counter: Arc<AtomicI32> = Arc::new(AtomicI32::new(0));
        let pairs: [(Arc<AtomicI32>, i32); 3] = [
            (Arc::clone(&counter), 1),
            (Arc::clone(&counter), -1),
            (Arc::clone(&counter), -1),
        ];
        let [p1, p2, p3] = [self.page.clone(), self.page.clone(), self.page.clone()];

        macro_rules! spawn_tracker {
            ($page:expr, $event:ty, $c:expr, $delta:expr) => {
                match $page.event_listener::<$event>().await {
                    Ok(mut s) => {
                        let c = $c;
                        let d = $delta;
                        tokio::spawn(async move {
                            while s.next().await.is_some() {
                                c.fetch_add(d, Ordering::Relaxed);
                            }
                        });
                    }
                    Err(e) => warn!("network-idle tracker unavailable: {e}"),
                }
            };
        }

        let [(c1, d1), (c2, d2), (c3, d3)] = pairs;
        spawn_tracker!(p1, EventRequestWillBeSent, c1, d1);
        spawn_tracker!(p2, EventLoadingFinished, c2, d2);
        spawn_tracker!(p3, EventLoadingFailed, c3, d3);

        counter
    }

    async fn wait_network_idle(counter: &Arc<std::sync::atomic::AtomicI32>) {
        const IDLE_THRESHOLD: i32 = 2;
        const SETTLE: Duration = Duration::from_millis(500);
        loop {
            if counter.load(Ordering::Relaxed) <= IDLE_THRESHOLD {
                tokio::time::sleep(SETTLE).await;
                if counter.load(Ordering::Relaxed) <= IDLE_THRESHOLD {
                    break;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    ///
    /// # Errors
    ///
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

    ///
    /// Enables `Fetch` interception and spawns a background task that continues
    /// allowed requests and fails blocked ones with `BlockedByClient`. Any
    /// previously set filter task is cancelled first.
    ///
    /// # Errors
    ///
    pub async fn set_resource_filter(&mut self, filter: ResourceFilter) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::fetch::{
            ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams,
            RequestPattern,
        };
        use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
        use futures::StreamExt as _;

        if filter.is_empty() {
            return Ok(());
        }

        // Cancel any previously running filter task.
        if let Some(task) = self.resource_filter_task.take() {
            task.abort();
        }

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

        // is never blocked. Without this handler Chrome holds every intercepted
        // request indefinitely and the page hangs.
        let mut events = self
            .page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(|e| BrowserError::CdpError {
                operation: "Fetch.requestPaused subscribe".to_string(),
                message: e.to_string(),
            })?;

        let page = self.page.clone();
        debug!("Resource filter active: {:?}", filter);
        let task = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let request_id = event.request_id.clone();
                if filter.should_block(event.resource_type.as_ref()) {
                    let params = FailRequestParams::new(request_id, ErrorReason::BlockedByClient);
                    let _ = page.execute(params).await;
                } else {
                    let _ = page.execute(ContinueRequestParams::new(request_id)).await;
                }
            }
        });

        self.resource_filter_task = Some(task);
        Ok(())
    }

    /// Return the current page URL (post-navigation, post-redirect).
    ///
    /// internally by [`save_cookies`](Self::save_cookies); no extra network
    /// request is made.  Returns an empty string if the URL is not yet set
    ///
    /// # Errors
    ///
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
    /// navigations, when [`navigate`](Self::navigate) has not yet been called,
    /// or if the network event subscription failed.
    ///
    /// # Errors
    ///
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

    /// lightweight [`NodeHandle`]s backed by CDP `RemoteObjectId`s.
    ///
    /// No HTML serialisation occurs — the browser's in-memory DOM is queried
    /// directly over the CDP connection, eliminating the `page.content()` +
    /// `scraper::Html::parse_document` round-trip.
    ///
    ///
    /// # Errors
    ///
    /// [`BrowserError::Timeout`] if it exceeds `cdp_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    /// # let nodes = page.query_selector_all("div[data-ux]").await?;
    /// # for node in &nodes {
    ///     let ux_type = node.attr("data-ux").await?;
    ///     let text    = node.text_content().await?;
    ///     println!("{ux_type:?}: {text}");
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_selector_all(&self, selector: &str) -> Result<Vec<NodeHandle>> {
        let elements = timeout(self.cdp_timeout, self.page.find_elements(selector))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "PageHandle::query_selector_all".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| BrowserError::CdpError {
                operation: "PageHandle::query_selector_all".to_string(),
                message: e.to_string(),
            })?;

        let selector_arc: Arc<str> = Arc::from(selector);
        Ok(elements
            .into_iter()
            .map(|el| NodeHandle {
                element: el,
                selector: selector_arc.clone(),
                cdp_timeout: self.cdp_timeout,
                page: self.page.clone(),
            })
            .collect())
    }

    /// Evaluate arbitrary JavaScript and return the result as `T`.
    ///
    /// # Errors
    ///
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

    ///
    /// # Errors
    ///
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

    ///
    /// [`SessionSnapshot`][crate::session::SessionSnapshot] and without
    /// requiring a direct `chromiumoxide` dependency in calling code.
    ///
    /// Individual cookie failures are logged as warnings and do not abort the
    /// remaining cookies.
    ///
    /// # Errors
    ///
    /// call exceeds `cdp_timeout`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::session::SessionCookie;
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let page = handle.browser().expect("valid browser").new_page().await?;
    /// let cookies = vec![SessionCookie {
    ///     name: "session".to_string(),
    ///     value: "abc123".to_string(),
    ///     domain: ".example.com".to_string(),
    ///     path: "/".to_string(),
    ///     expires: -1.0,
    ///     http_only: true,
    ///     secure: true,
    ///     same_site: "Lax".to_string(),
    /// }];
    /// page.inject_cookies(&cookies).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn inject_cookies(&self, cookies: &[crate::session::SessionCookie]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetCookieParams;

        for cookie in cookies {
            let params = match SetCookieParams::builder()
                .name(cookie.name.clone())
                .value(cookie.value.clone())
                .domain(cookie.domain.clone())
                .path(cookie.path.clone())
                .http_only(cookie.http_only)
                .secure(cookie.secure)
                .build()
            {
                Ok(p) => p,
                Err(e) => {
                    warn!(cookie = %cookie.name, error = %e, "Failed to build cookie params");
                    continue;
                }
            };

            match timeout(self.cdp_timeout, self.page.execute(params)).await {
                Err(_) => {
                    warn!(
                        cookie = %cookie.name,
                        timeout_ms = self.cdp_timeout.as_millis(),
                        "Timed out injecting cookie"
                    );
                }
                Ok(Err(e)) => {
                    warn!(cookie = %cookie.name, error = %e, "Failed to inject cookie");
                }
                Ok(Ok(_)) => {}
            }
        }

        debug!(count = cookies.len(), "Cookies injected");
        Ok(())
    }

    /// Capture a screenshot of the current page as PNG bytes.
    ///
    /// them in-memory.
    ///
    /// # Errors
    ///
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

// ─── Stealth diagnostics ──────────────────────────────────────────────────────

#[cfg(feature = "stealth")]
impl PageHandle {
    /// Run all built-in stealth detection checks against the current page.
    ///
    /// Iterates [`crate::diagnostic::all_checks`], evaluates each check's
    /// JavaScript via CDP `Runtime.evaluate`, and returns an aggregate
    /// [`crate::diagnostic::DiagnosticReport`].
    ///
    /// recorded as failing checks and do **not** abort the whole run.
    ///
    /// # Errors
    ///
    /// Individual check failures are captured in the report.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::page::WaitUntil;
    /// use std::time::Duration;
    ///
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let browser = handle.browser().expect("valid browser");
    /// let mut page = browser.new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(10)).await?;
    ///
    /// let report = page.verify_stealth().await?;
    /// println!("Stealth: {}/{} checks passed", report.passed_count, report.checks.len());
    /// # for failure in report.failures() {
    ///     eprintln!("  FAIL  {}: {}", failure.description, failure.details);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn verify_stealth(&self) -> Result<crate::diagnostic::DiagnosticReport> {
        use crate::diagnostic::{CheckResult, DiagnosticReport, all_checks};

        let mut results: Vec<CheckResult> = Vec::new();

        for check in all_checks() {
            let result = match self.eval::<String>(check.script).await {
                Ok(json) => check.parse_output(&json),
                Err(e) => {
                    tracing::warn!(
                        check = ?check.id,
                        error = %e,
                        "stealth check script failed during evaluation"
                    );
                    CheckResult {
                        id: check.id,
                        description: check.description.to_string(),
                        passed: false,
                        details: format!("script error: {e}"),
                    }
                }
            };
            tracing::debug!(
                check = ?result.id,
                passed = result.passed,
                details = %result.details,
                "stealth check result"
            );
            results.push(result);
        }

        Ok(DiagnosticReport::new(results))
    }

    /// Run stealth checks and attach transport diagnostics (JA3/JA4/HTTP3).
    ///
    pub async fn verify_stealth_with_transport(
        &self,
        observed: Option<crate::diagnostic::TransportObservations>,
    ) -> Result<crate::diagnostic::DiagnosticReport> {
        let report = self.verify_stealth().await?;

        let user_agent = match self.eval::<String>("navigator.userAgent").await {
            Ok(ua) => ua,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read navigator.userAgent for transport diagnostics");
                String::new()
            }
        };

        let transport = crate::diagnostic::TransportDiagnostic::from_user_agent_and_observations(
            &user_agent,
            observed.as_ref(),
        );

        Ok(report.with_transport(transport))
    }
}

// ─── extract feature ─────────────────────────────────────────────────────────

#[cfg(feature = "extract")]
impl PageHandle {
    ///
    ///
    /// All per-node extractions are driven concurrently via
    /// [`futures::future::try_join_all`].
    ///
    /// # Errors
    ///
    /// fails, or [`BrowserError::ExtractionFailed`] if any field extraction
    /// fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use stygian_browser::extract::Extract;
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use std::time::Duration;
    ///
    /// #[derive(Extract)]
    /// struct Link {
    ///     href: Option<String>,
    /// }
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate(
    ///     "https://example.com",
    ///     WaitUntil::DomContentLoaded,
    ///     Duration::from_secs(30),
    /// ).await?;
    /// let links: Vec<Link> = page.extract_all::<Link>("nav li").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_all<T>(&self, selector: &str) -> Result<Vec<T>>
    where
        T: crate::extract::Extractable,
    {
        use futures::future::try_join_all;

        let nodes = self.query_selector_all(selector).await?;
        try_join_all(nodes.iter().map(|n| T::extract_from(n)))
            .await
            .map_err(BrowserError::ExtractionFailed)
    }
}

// ─── similarity feature ──────────────────────────────────────────────────────

#[cfg(feature = "similarity")]
impl NodeHandle {
    /// node.
    ///
    /// Issues a single `Runtime.callFunctionOn` JS eval that extracts the tag,
    /// class list, attribute names, and body-depth in one round-trip.
    ///
    /// # Errors
    ///
    /// invalidated, or [`BrowserError::ScriptExecutionFailed`] if the script
    /// produces unexpected output.
    pub async fn fingerprint(&self) -> Result<crate::similarity::ElementFingerprint> {
        const JS: &str = r"function() {
    var el = this;
    var tag = el.tagName.toLowerCase();
    var classes = Array.prototype.slice.call(el.classList).sort();
    var attrNames = Array.prototype.slice.call(el.attributes)
        .map(function(a) { return a.name; })
        .filter(function(n) { return n !== 'class' && n !== 'id'; })
        .sort();
    var depth = 0;
    var n = el.parentElement;
    while (n && n.tagName.toLowerCase() !== 'body') { depth++; n = n.parentElement; }
    return JSON.stringify({ tag: tag, classes: classes, attrNames: attrNames, depth: depth });
}";

        let returns = tokio::time::timeout(self.cdp_timeout, self.element.call_js_fn(JS, true))
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::fingerprint".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "fingerprint"))?;

        let json_str = returns
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrowserError::ScriptExecutionFailed {
                script: "NodeHandle::fingerprint".to_string(),
                reason: "CDP returned no string value from fingerprint script".to_string(),
            })?;

        serde_json::from_str::<crate::similarity::ElementFingerprint>(json_str).map_err(|e| {
            BrowserError::ScriptExecutionFailed {
                script: "NodeHandle::fingerprint".to_string(),
                reason: format!("failed to deserialise fingerprint JSON: {e}"),
            }
        })
    }
}

#[cfg(feature = "similarity")]
impl PageHandle {
    /// `reference`, scored by [`crate::similarity::SimilarityConfig`].
    ///
    /// [`NodeHandle::fingerprint`]), then fingerprints every candidate returned
    /// [`crate::similarity::jaccard_weighted`] score exceeds
    /// `config.threshold`.  Results are ordered by score descending.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
    /// use stygian_browser::similarity::SimilarityConfig;
    /// use std::time::Duration;
    ///
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate("https://example.com", WaitUntil::DomContentLoaded, Duration::from_secs(30)).await?;
    ///
    /// # let nodes = page.query_selector_all("h1").await?;
    /// # let reference = nodes.into_iter().next().ok_or(stygian_browser::error::BrowserError::StaleNode { selector: "h1".to_string() })?;
    ///     let similar = page.find_similar(&reference, SimilarityConfig::default()).await?;
    /// # for m in &similar {
    ///         println!("score={:.2}", m.score);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`BrowserError::ScriptExecutionFailed`] if a scoring script fails.
    pub async fn find_similar(
        &self,
        reference: &NodeHandle,
        config: crate::similarity::SimilarityConfig,
    ) -> Result<Vec<crate::similarity::SimilarMatch>> {
        use crate::similarity::{SimilarMatch, jaccard_weighted};

        let ref_fp = reference.fingerprint().await?;
        let candidates = self.query_selector_all("*").await?;

        let mut matches: Vec<SimilarMatch> = Vec::new();
        for node in candidates {
            if let Ok(cand_fp) = node.fingerprint().await {
                let score = jaccard_weighted(&ref_fp, &cand_fp);
                if score >= config.threshold {
                    matches.push(SimilarMatch { node, score });
                }
            }
            // Stale / detached nodes are silently skipped.
        }

        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if config.max_results > 0 {
            matches.truncate(config.max_results);
        }

        Ok(matches)
    }
}

impl Drop for PageHandle {
    fn drop(&mut self) {
        warn!("PageHandle dropped without explicit close(); spawning cleanup task");
        // chromiumoxide Page does not implement close on Drop, so we spawn
        // swap it out. We clone the Page handle (it's Arc-backed internally).
        let page = self.page.clone();
        tokio::spawn(async move {
            let _ = page.close().await;
        });
    }
}

// ─── Session warmup & refresh ─────────────────────────────────────────────────

/// Simplified, JSON-serializable wait strategy used in [`WarmupOptions`] and
/// [`RefreshOptions`].
///
/// This is a serialization-friendly analogue of [`WaitUntil`].  Use
/// [`WarmupWait::into_wait_until`] to convert before calling
/// [`PageHandle::navigate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WarmupWait {
    /// Wait until the HTML is fully parsed (`DOMContentLoaded`).  This is the
    /// default and works for most pages.
    #[default]
    DomContentLoaded,
    /// Wait until there are no more than two in-flight network requests for at
    /// least 500 ms after navigation.
    NetworkIdle,
}

impl WarmupWait {
    /// Convert into the lower-level [`WaitUntil`] enum.
    #[must_use]
    pub fn into_wait_until(self) -> WaitUntil {
        match self {
            Self::DomContentLoaded => WaitUntil::DomContentLoaded,
            Self::NetworkIdle => WaitUntil::NetworkIdle,
        }
    }
}

/// Options for [`PageHandle::warmup`].
///
/// # Example
///
/// ```
/// use stygian_browser::page::{WarmupOptions, WarmupWait};
///
/// let opts = WarmupOptions {
///     url: "https://example.com".to_string(),
///     wait: WarmupWait::DomContentLoaded,
///     timeout_ms: 30_000,
///     stabilize_ms: 500,
/// };
/// assert_eq!(opts.timeout_ms, 30_000);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupOptions {
    /// The URL to navigate to during warmup.
    pub url: String,
    /// Wait strategy applied after the navigation commit (default:
    /// `DomContentLoaded`).
    #[serde(default)]
    pub wait: WarmupWait,
    /// Navigation timeout in milliseconds.  Default: `30 000`.
    #[serde(default = "WarmupOptions::default_timeout_ms")]
    pub timeout_ms: u64,
    /// Additional pause after navigation to let dynamic resources (XHR,
    /// lazy-loaded images) settle, in milliseconds.  `0` disables the
    /// stabilization step (default).
    #[serde(default)]
    pub stabilize_ms: u64,
}

impl WarmupOptions {
    /// Returns the default navigation timeout (30 000 ms).
    #[must_use]
    pub const fn default_timeout_ms() -> u64 {
        30_000
    }
}

impl Default for WarmupOptions {
    fn default() -> Self {
        Self {
            url: String::new(),
            wait: WarmupWait::DomContentLoaded,
            timeout_ms: Self::default_timeout_ms(),
            stabilize_ms: 0,
        }
    }
}

/// Diagnostic report produced by [`PageHandle::warmup`].
///
/// # Example
///
/// ```
/// use stygian_browser::page::WarmupReport;
/// let report = WarmupReport {
///     url: "https://example.com".to_string(),
///     elapsed_ms: 250,
///     status_code: Some(200),
///     title: "Example Domain".to_string(),
///     stabilized: false,
/// };
/// assert_eq!(report.status_code, Some(200));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupReport {
    /// The URL that was warmed.
    pub url: String,
    /// Elapsed wall-time in milliseconds.
    pub elapsed_ms: u64,
    /// HTTP status code of the warmup navigation, if captured by the
    /// `Network.responseReceived` listener.
    pub status_code: Option<u16>,
    /// Page title after warmup navigation.
    pub title: String,
    /// Whether a stabilization pause (`stabilize_ms > 0`) was applied after
    /// navigation.
    pub stabilized: bool,
}

/// Options for [`PageHandle::refresh`].
///
/// # Example
///
/// ```
/// use stygian_browser::page::{RefreshOptions, WarmupWait};
///
/// let opts = RefreshOptions {
///     wait: WarmupWait::DomContentLoaded,
///     timeout_ms: 15_000,
///     reset_connection: true,
/// };
/// assert!(opts.reset_connection);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshOptions {
    /// Wait strategy applied after the reload (default: `DomContentLoaded`).
    #[serde(default)]
    pub wait: WarmupWait,
    /// Reload timeout in milliseconds.  Default: `30 000`.
    #[serde(default = "RefreshOptions::default_timeout_ms")]
    pub timeout_ms: u64,
    /// When `true`, re-navigates to the current URL rather than issuing a
    /// browser-level reload.  This signals to the calling code that a new TCP
    /// connection is desired while cookies and storage are retained in the
    /// browser process.  Default: `false`.
    #[serde(default)]
    pub reset_connection: bool,
}

impl RefreshOptions {
    /// Returns the default reload timeout (30 000 ms).
    #[must_use]
    pub const fn default_timeout_ms() -> u64 {
        30_000
    }
}

impl Default for RefreshOptions {
    fn default() -> Self {
        Self {
            wait: WarmupWait::DomContentLoaded,
            timeout_ms: Self::default_timeout_ms(),
            reset_connection: false,
        }
    }
}

/// Diagnostic report produced by [`PageHandle::refresh`].
///
/// # Example
///
/// ```
/// use stygian_browser::page::RefreshReport;
/// let report = RefreshReport {
///     url: "https://example.com".to_string(),
///     elapsed_ms: 180,
///     status_code: Some(200),
/// };
/// assert_eq!(report.elapsed_ms, 180);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshReport {
    /// URL of the page after the refresh navigation.
    pub url: String,
    /// Elapsed wall-time in milliseconds.
    pub elapsed_ms: u64,
    /// HTTP status code of the refresh navigation, if captured.
    pub status_code: Option<u16>,
}

// ─── PageHandle warmup / refresh ──────────────────────────────────────────────

impl PageHandle {
    /// Warm up a browser session by navigating to `options.url` and
    /// optionally waiting for dynamic resources to settle.
    ///
    /// Warmup is **idempotent**: calling it repeatedly re-navigates and
    /// re-warms the same session without adverse side effects.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NavigationFailed`] if the navigation times out
    /// or the underlying CDP call fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::page::{WarmupOptions, WarmupWait};
    ///
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    ///
    /// let report = page.warmup(WarmupOptions {
    ///     url: "https://example.com".to_string(),
    ///     wait: WarmupWait::DomContentLoaded,
    ///     timeout_ms: 30_000,
    ///     stabilize_ms: 500,
    /// }).await?;
    /// println!("warmed in {}ms: {}", report.elapsed_ms, report.title);
    /// handle.release().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn warmup(&mut self, options: WarmupOptions) -> Result<WarmupReport> {
        let start = std::time::Instant::now();
        let nav_timeout = Duration::from_millis(options.timeout_ms);
        self.navigate(&options.url, options.wait.clone().into_wait_until(), nav_timeout)
            .await?;
        let status_code = self.status_code()?;
        let title = self.title().await.unwrap_or_default();
        let stabilized = options.stabilize_ms > 0;
        if stabilized {
            tokio::time::sleep(Duration::from_millis(options.stabilize_ms)).await;
        }
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(WarmupReport {
            url: options.url,
            elapsed_ms,
            status_code,
            title,
            stabilized,
        })
    }

    /// Refresh the current page, retaining all in-browser session state
    /// (cookies, `localStorage`, `sessionStorage`).
    ///
    /// When `options.reset_connection` is `false` (default) a standard
    /// CDP reload is issued.  When `true`, the current URL is re-navigated,
    /// which expresses the caller's intent to force a new underlying TCP/TLS
    /// connection while keeping all browser-side state intact.
    ///
    /// Refresh is **idempotent**: repeated calls simply reload the page again.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::NavigationFailed`] if the current URL cannot be
    /// determined or the reload times out.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> stygian_browser::error::Result<()> {
    /// use stygian_browser::{BrowserPool, BrowserConfig};
    /// use stygian_browser::page::{RefreshOptions, WaitUntil};
    ///
    /// let pool = BrowserPool::new(BrowserConfig::default()).await?;
    /// let handle = pool.acquire().await?;
    /// let mut page = handle.browser().expect("valid browser").new_page().await?;
    /// page.navigate(
    ///     "https://example.com",
    ///     WaitUntil::DomContentLoaded,
    ///     std::time::Duration::from_secs(30),
    /// ).await?;
    ///
    /// let report = page.refresh(RefreshOptions::default()).await?;
    /// println!("refreshed in {}ms", report.elapsed_ms);
    /// handle.release().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn refresh(&mut self, options: RefreshOptions) -> Result<RefreshReport> {
        let start = std::time::Instant::now();
        let nav_timeout = Duration::from_millis(options.timeout_ms);
        let wait = options.wait.clone().into_wait_until();
        // Resolve the current URL before any navigation changes it.
        let current_url = self.url().await?;
        if current_url.is_empty() || current_url == "about:blank" {
            return Err(BrowserError::NavigationFailed {
                url: current_url,
                reason: "page has not been navigated yet; call warmup() or navigate() first"
                    .to_string(),
            });
        }
        // Both code paths navigate to the same URL.  `reset_connection: true`
        // expresses the *intent* to use a new TCP connection; the browser is free
        // to reuse or create a new connection as its connection pool dictates.
        self.navigate(&current_url, wait, nav_timeout).await?;
        let status_code = self.status_code()?;
        let url = self.url().await?;
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        Ok(RefreshReport {
            url,
            elapsed_ms,
            status_code,
        })
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

    #[test]
    fn page_handle_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<PageHandle>();
        assert_sync::<PageHandle>();
    }

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

    // ── NodeHandle pure-logic tests ───────────────────────────────────────────

    /// `attr_map` relies on `chunks_exact(2)` — verify the pairing logic is
    /// correct without a live browser by exercising it directly.
    #[test]
    fn attr_map_chunking_pairs_correctly() {
        let flat = [
            "id".to_string(),
            "main".to_string(),
            "data-ux".to_string(),
            "Section".to_string(),
            "class".to_string(),
            "container".to_string(),
        ];
        let mut map = std::collections::HashMap::with_capacity(flat.len() / 2);
        for pair in flat.chunks_exact(2) {
            if let [name, value] = pair {
                map.insert(name.clone(), value.clone());
            }
        }
        assert_eq!(map.get("id").map(String::as_str), Some("main"));
        assert_eq!(map.get("data-ux").map(String::as_str), Some("Section"));
        assert_eq!(map.get("class").map(String::as_str), Some("container"));
        assert_eq!(map.len(), 3);
    }

    /// gracefully — the trailing element is silently ignored.
    #[test]
    fn attr_map_chunking_ignores_odd_trailing() {
        let flat = ["orphan".to_string()]; // no value
        let mut map = std::collections::HashMap::new();
        for pair in flat.chunks_exact(2) {
            if let [name, value] = pair {
                map.insert(name.clone(), value.clone());
            }
        }
        assert!(map.is_empty());
    }

    /// Empty flat list → empty map.
    #[test]
    fn attr_map_chunking_empty_input() {
        let flat: Vec<String> = vec![];
        let map: std::collections::HashMap<String, String> = flat
            .chunks_exact(2)
            .filter_map(|pair| {
                if let [name, value] = pair {
                    Some((name.clone(), value.clone()))
                } else {
                    None
                }
            })
            .collect();
        assert!(map.is_empty());
    }

    #[test]
    fn ancestors_json_parse_round_trip() -> std::result::Result<(), serde_json::Error> {
        let json = r#"["p","article","body","html"]"#;
        let result: Vec<String> = serde_json::from_str(json)?;
        assert_eq!(result, ["p", "article", "body", "html"]);
        Ok(())
    }

    #[test]
    fn ancestors_json_parse_empty() -> std::result::Result<(), serde_json::Error> {
        let json = "[]";
        let result: Vec<String> = serde_json::from_str(json)?;
        assert!(result.is_empty());
        Ok(())
    }

    /// `"div::parent"`) must surface that suffix in its `Display` output so
    /// callers can locate the failed traversal in logs.
    #[test]
    fn traversal_selector_suffix_in_stale_error() {
        let e = crate::error::BrowserError::StaleNode {
            selector: "div::parent".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("div::parent"),
            "StaleNode display must include the full selector; got: {msg}"
        );
    }

    #[test]
    fn traversal_next_suffix_in_stale_error() {
        let e = crate::error::BrowserError::StaleNode {
            selector: "li.price::next".to_string(),
        };
        assert!(e.to_string().contains("li.price::next"));
    }

    #[test]
    fn traversal_prev_suffix_in_stale_error() {
        let e = crate::error::BrowserError::StaleNode {
            selector: "td.label::prev".to_string(),
        };
        assert!(e.to_string().contains("td.label::prev"));
    }

    // ── Warmup / Refresh type tests ───────────────────────────────────────────

    #[test]
    fn warmup_options_defaults() {
        let opts = WarmupOptions::default();
        assert_eq!(opts.wait, WarmupWait::DomContentLoaded);
        assert_eq!(opts.timeout_ms, WarmupOptions::default_timeout_ms());
        assert_eq!(opts.stabilize_ms, 0);
    }

    #[test]
    fn warmup_options_serialize_round_trip()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let opts = WarmupOptions {
            url: "https://example.com".to_string(),
            wait: WarmupWait::NetworkIdle,
            timeout_ms: 15_000,
            stabilize_ms: 250,
        };
        let json = serde_json::to_string(&opts)?;
        let restored: WarmupOptions = serde_json::from_str(&json)?;
        assert_eq!(restored.url, "https://example.com");
        assert_eq!(restored.wait, WarmupWait::NetworkIdle);
        assert_eq!(restored.timeout_ms, 15_000);
        assert_eq!(restored.stabilize_ms, 250);
        Ok(())
    }

    #[test]
    fn warmup_wait_default_is_dom_content_loaded() {
        assert_eq!(WarmupWait::default(), WarmupWait::DomContentLoaded);
    }

    #[test]
    fn warmup_wait_into_wait_until_variants() {
        assert!(matches!(
            WarmupWait::DomContentLoaded.into_wait_until(),
            WaitUntil::DomContentLoaded
        ));
        assert!(matches!(
            WarmupWait::NetworkIdle.into_wait_until(),
            WaitUntil::NetworkIdle
        ));
    }

    #[test]
    fn refresh_options_defaults() {
        let opts = RefreshOptions::default();
        assert_eq!(opts.wait, WarmupWait::DomContentLoaded);
        assert_eq!(opts.timeout_ms, RefreshOptions::default_timeout_ms());
        assert!(!opts.reset_connection);
    }

    #[test]
    fn refresh_options_serialize_round_trip()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let opts = RefreshOptions {
            wait: WarmupWait::NetworkIdle,
            timeout_ms: 10_000,
            reset_connection: true,
        };
        let json = serde_json::to_string(&opts)?;
        let restored: RefreshOptions = serde_json::from_str(&json)?;
        assert_eq!(restored.wait, WarmupWait::NetworkIdle);
        assert_eq!(restored.timeout_ms, 10_000);
        assert!(restored.reset_connection);
        Ok(())
    }

    #[test]
    fn warmup_report_serialize_round_trip()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let report = WarmupReport {
            url: "https://example.com".to_string(),
            elapsed_ms: 320,
            status_code: Some(200),
            title: "Example Domain".to_string(),
            stabilized: true,
        };
        let json = serde_json::to_string(&report)?;
        let restored: WarmupReport = serde_json::from_str(&json)?;
        assert_eq!(restored.url, "https://example.com");
        assert_eq!(restored.elapsed_ms, 320);
        assert_eq!(restored.status_code, Some(200));
        assert_eq!(restored.title, "Example Domain");
        assert!(restored.stabilized);
        Ok(())
    }

    #[test]
    fn refresh_report_serialize_round_trip()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let report = RefreshReport {
            url: "https://example.com/".to_string(),
            elapsed_ms: 180,
            status_code: Some(304),
        };
        let json = serde_json::to_string(&report)?;
        let restored: RefreshReport = serde_json::from_str(&json)?;
        assert_eq!(restored.url, "https://example.com/");
        assert_eq!(restored.elapsed_ms, 180);
        assert_eq!(restored.status_code, Some(304));
        Ok(())
    }

    #[test]
    fn warmup_options_missing_stabilize_ms_defaults_to_zero()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        // stabilize_ms has `#[serde(default)]`; omitting it from JSON should
        // deserialize to 0 rather than erroring.
        let json = r#"{"url":"https://example.com","timeout_ms":30000}"#;
        let opts: WarmupOptions = serde_json::from_str(json)?;
        assert_eq!(opts.stabilize_ms, 0);
        Ok(())
    }

    // ── Integration tests (require live Chrome — skipped in CI) ──────────────

    /// Warm up a page then immediately extract content from the same origin.
    #[test]
    #[ignore = "requires live Chrome"]
    fn integration_warmup_then_extraction() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            use crate::{BrowserConfig, BrowserPool};
            let pool = BrowserPool::new(BrowserConfig::default())
                .await
                .expect("pool");
            let handle = pool.acquire().await.expect("handle");
            let mut page = handle
                .browser()
                .expect("browser")
                .new_page()
                .await
                .expect("page");

            let report = page
                .warmup(WarmupOptions {
                    url: "https://example.com".to_string(),
                    wait: WarmupWait::DomContentLoaded,
                    timeout_ms: 30_000,
                    stabilize_ms: 0,
                })
                .await
                .expect("warmup");

            assert!(!report.title.is_empty(), "title populated after warmup");
            assert!(report.elapsed_ms > 0);

            // Confirm the page is still usable for further queries.
            let html = page.content().await.expect("content");
            assert!(
                html.contains("example"),
                "page content available after warmup"
            );

            page.close().await.expect("close");
            handle.release().await;
        });
    }

    /// Refresh a page and verify session continuity (URL unchanged, page
    /// still navigable).
    #[test]
    #[ignore = "requires live Chrome"]
    fn integration_refresh_keeps_session_state() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            use crate::{BrowserConfig, BrowserPool};
            let pool = BrowserPool::new(BrowserConfig::default())
                .await
                .expect("pool");
            let handle = pool.acquire().await.expect("handle");
            let mut page = handle
                .browser()
                .expect("browser")
                .new_page()
                .await
                .expect("page");

            page.navigate(
                "https://example.com",
                WaitUntil::DomContentLoaded,
                Duration::from_secs(30),
            )
            .await
            .expect("initial navigate");

            let report = page.refresh(RefreshOptions::default()).await.expect("refresh");

            assert!(
                report.url.contains("example.com"),
                "URL retained after refresh; got: {}",
                report.url
            );
            assert!(report.elapsed_ms > 0);

            page.close().await.expect("close");
            handle.release().await;
        });
    }
}
