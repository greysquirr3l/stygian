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
    #[must_use]
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
    #[must_use]
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

    #[must_use]
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

    #[must_use]
    pub fn should_block(&self, cdp_type: &str) -> bool {
        self.blocked
            .iter()
            .any(|r| r.as_cdp_str().eq_ignore_ascii_case(cdp_type))
    }

    #[must_use]
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

// ─── OuterHtmlStrategy / OuterHtmlResult ──────────────────────────────────────

/// Selector for [`NodeHandle::outer_html_with_strategy`].
///
/// The default [`OuterHtmlStrategy::Current`] preserves the historical call
/// path used by [`NodeHandle::outer_html`]: a Chromium element-level
/// `outer_html()` call (which evaluates `this.outerHTML` via JS) followed
/// by a direct `XMLSerializer` fallback when the primary call returns an
/// empty payload.
///
/// [`OuterHtmlStrategy::Recursive`] uses the dedicated Chromium `DevTools`
/// Protocol command `DOM.getOuterHTML` (a single round-trip, browser-side
/// serialisation that already includes shadow-DOM roots) with a Rust-side
/// fallback that calls `DOM.describeNode` with `depth = -1` and walks the
/// resulting CDP `Node` tree to produce HTML locally.
///
/// Both strategies are **generic** — neither relies on Wix, SPA, or vendor
/// attributes, classes, or heuristics. `Recursive` simply selects a different
/// CDP backend that already handles deeply nested subtrees, large SPAs, and
/// shadow-DOM trees correctly in a single browser-side pass.
///
/// # Example
///
/// ```
/// use stygian_browser::page::OuterHtmlStrategy;
/// assert_eq!(OuterHtmlStrategy::default(), OuterHtmlStrategy::Current);
/// assert_eq!(OuterHtmlStrategy::Current.as_str(), "Current");
/// assert_eq!(OuterHtmlStrategy::Recursive.as_str(), "Recursive");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum OuterHtmlStrategy {
    /// Legacy behaviour: element-level JS eval + `XMLSerializer` fallback.
    #[default]
    Current,
    /// CDP `DOM.getOuterHTML` (single round-trip) + Rust-side
    /// `DOM.describeNode` walk fallback.
    Recursive,
}

impl OuterHtmlStrategy {
    /// Stable identifier suitable for logs, metrics, and serialization.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Recursive => "Recursive",
        }
    }

    /// All known variants in declaration order. Useful for exhaustive
    /// iteration in tests and diagnostics.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Current, Self::Recursive]
    }
}

impl std::fmt::Display for OuterHtmlStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of [`NodeHandle::outer_html_with_strategy`].
///
/// The default `String`-returning [`NodeHandle::outer_html`] flattens this
/// into a `Result<String>` where `Empty` and `Failed` both surface as the
/// empty string — preserving the historical contract.
///
/// Derives [`Serialize`] so callers can include the outcome in structured
/// logs, metrics, or per-request reports. `Deserialize` is intentionally not
/// derived because the `Failed::backends` field holds `&'static str`
/// backend names — a deserialised value would need owned `String`s and
/// would lose the typed backend taxonomy this enum encodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum OuterHtmlResult {
    /// The chosen strategy's backends all returned an empty payload. This
    /// typically means the page is still rendering or the node has been
    /// detached since the handle was created.
    Empty,
    /// Successfully serialised outer markup for the target node.
    Content(String),
    /// Every backend the strategy tried returned an error. The list names
    /// the backends in the order they were attempted so callers can build
    /// retry strategies or surface diagnostics.
    Failed {
        /// Names of the backends that returned an error.
        backends: Vec<&'static str>,
    },
}

impl OuterHtmlResult {
    /// Return the serialized markup, or `None` if the result is `Empty` or
    /// `Failed`.
    #[must_use]
    pub const fn content(&self) -> Option<&str> {
        match self {
            Self::Content(s) => Some(s.as_str()),
            Self::Empty | Self::Failed { .. } => None,
        }
    }

    /// `true` when the result carries no usable markup — either `Empty` or
    /// `Failed`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Content(s) => s.is_empty(),
            Self::Empty | Self::Failed { .. } => true,
        }
    }
}

impl std::fmt::Display for OuterHtmlResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Content(s) => write!(f, "Content({} bytes)", s.len()),
            Self::Failed { backends } => write!(f, "Failed({})", backends.join(", ")),
        }
    }
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
    /// Backwards-compatible thin wrapper around
    /// [`outer_html_with_strategy`][Self::outer_html_with_strategy] using the
    /// default [`OuterHtmlStrategy::Current`] strategy. Preserves the
    /// historical return contract: `Ok(String)` where the string may be
    /// empty when both the primary and fallback backends return empty
    /// payloads.
    ///
    /// Callers that need to distinguish an empty payload from a hard failure
    /// — or that want the deeper `DOM.getOuterHTML` + Rust-side walk path —
    /// should call [`outer_html_with_strategy`][Self::outer_html_with_strategy]
    /// directly.
    ///
    /// # Errors
    ///
    /// Returns an error when any CDP call the chosen strategy actually
    /// invokes fails — that includes both the primary call and any fallback
    /// call (the `XMLSerializer` JS fallback for [`OuterHtmlStrategy::Current`],
    /// the `DOM.describeNode` walk for [`OuterHtmlStrategy::Recursive`]).
    /// Errors surface as [`BrowserError::Timeout`] (CDP call exceeded
    /// `cdp_timeout`), [`BrowserError::StaleNode`] (the handle was
    /// invalidated mid-call), or [`BrowserError::CdpError`] (transport-level
    /// failure).
    ///
    /// Empty or partially-empty payloads from any individual backend do
    /// **not** error — they are flattened to an empty `String` so the
    /// historical `Ok(String)` contract is preserved. Callers that need to
    /// distinguish an empty payload from a hard failure should call
    /// [`outer_html_with_strategy`][Self::outer_html_with_strategy]
    /// directly and inspect the [`OuterHtmlResult`] variant.
    pub async fn outer_html(&self) -> Result<String> {
        match self
            .outer_html_with_strategy(OuterHtmlStrategy::Current)
            .await?
        {
            OuterHtmlResult::Content(s) => Ok(s),
            OuterHtmlResult::Empty | OuterHtmlResult::Failed { .. } => Ok(String::new()),
        }
    }

    /// Return the element's `outerHTML` using an explicit resolution strategy.
    ///
    /// The [`OuterHtmlStrategy::Current`] strategy matches the historical
    /// [`outer_html`][Self::outer_html] path: a Chromium element-level JS
    /// evaluation of `this.outerHTML`, followed by a JS
    /// `new XMLSerializer().serializeToString(this)` fallback when the
    /// primary call returns an empty payload.
    ///
    /// The [`OuterHtmlStrategy::Recursive`] strategy resolves [#66] for
    /// sites where the JS-side `outerHTML` accessor intermittently returns
    /// a truncated or empty payload — most notably Wix Studio / Editor X
    /// pages and large SPAs with deeply nested shadow-DOM subtrees. It
    /// prefers the dedicated Chromium `DevTools` Protocol command
    /// `DOM.getOuterHTML` (a single round-trip that performs the
    /// serialisation inside the browser, with shadow-DOM roots included by
    /// default) and falls back to a Rust-side walk that calls
    /// `DOM.describeNode` with `depth = -1` and serialises the resulting
    /// `Node` tree to HTML locally. Neither path relies on Wix-specific
    /// selectors, attributes, or heuristics — the resolution is entirely
    /// driven by CDP commands Chromium already exposes.
    ///
    /// Both strategies return [`OuterHtmlResult::Empty`] (rather than
    /// `Failed`) when every backend returns an empty payload — this is
    /// indistinguishable from "node legitimately empty" at the CDP layer.
    ///
    /// [#66]: https://github.com/greysquirr3l/stygian/issues/66
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Timeout`] if the primary CDP call exceeds
    /// `cdp_timeout`, [`BrowserError::StaleNode`] if the handle was
    /// invalidated, or [`BrowserError::CdpError`] on transport-level
    /// failure.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_browser::page::OuterHtmlStrategy;
    /// # use stygian_browser::error::Result;
    /// # async fn run(handle: stygian_browser::NodeHandle) -> Result<()> {
    /// // Use the deep-resolution path for SPA / Wix Studio / shadow-DOM pages.
    /// let html = handle
    ///     .outer_html_with_strategy(OuterHtmlStrategy::Recursive)
    ///     .await?;
    /// # let _ = html;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn outer_html_with_strategy(
        &self,
        strategy: OuterHtmlStrategy,
    ) -> Result<OuterHtmlResult> {
        match strategy {
            OuterHtmlStrategy::Current => self.outer_html_current().await,
            OuterHtmlStrategy::Recursive => self.outer_html_recursive().await,
        }
    }

    /// Strategy body for [`OuterHtmlStrategy::Current`].
    async fn outer_html_current(&self) -> Result<OuterHtmlResult> {
        let primary = timeout(self.cdp_timeout, self.element.outer_html())
            .await
            .map_err(|_| BrowserError::Timeout {
                operation: "NodeHandle::outer_html_with_strategy(Current)".to_string(),
                duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
            })?
            .map_err(|e| self.cdp_err_or_stale(&e, "outer_html_current"))?;

        if let Some(html) = primary
            && !html.trim().is_empty()
        {
            return Ok(OuterHtmlResult::Content(html));
        }

        let fallback_html = self.outer_html_via_js().await?;
        if !fallback_html.trim().is_empty() {
            return Ok(OuterHtmlResult::Content(fallback_html));
        }

        Ok(OuterHtmlResult::Empty)
    }

    /// Strategy body for [`OuterHtmlStrategy::Recursive`].
    ///
    /// Primary: `DOM.getOuterHTML` (single round-trip, browser-side
    /// serialisation via stable `objectId`). Fallback: `DOM.describeNode`
    /// with `objectId` + `depth=-1`, Rust-side `Node` → HTML serializer.
    async fn outer_html_recursive(&self) -> Result<OuterHtmlResult> {
        use chromiumoxide::cdp::browser_protocol::dom::{GetOuterHtmlParams, GetOuterHtmlReturns};
        use chromiumoxide::types::CommandResponse;

        let mut failed_backends: Vec<&'static str> = Vec::new();

        let primary = timeout(
            self.cdp_timeout,
            self.page.execute(
                GetOuterHtmlParams::builder()
                    // Use the stable V8 RemoteObjectId instead of the
                    // ephemeral CDP NodeId. NodeIds are invalidated whenever
                    // the page's JavaScript mutates the DOM (e.g. React
                    // re-renders on SPAs like Wix), causing DOM.getOuterHTML
                    // to silently return an empty string for a valid node.
                    // RemoteObjectId is tied to the V8 heap object reference
                    // and survives DOM mutations.
                    .object_id(self.element.remote_object_id.clone())
                    .build(),
            ),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "NodeHandle::outer_html_with_strategy(Recursive)".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| self.cdp_err_or_stale(&e, "outer_html_recursive::DOM.getOuterHTML"));

        match primary {
            Ok(CommandResponse {
                result: GetOuterHtmlReturns { outer_html },
                ..
            }) if !outer_html.trim().is_empty() => {
                return Ok(OuterHtmlResult::Content(outer_html));
            }
            Ok(CommandResponse {
                result: GetOuterHtmlReturns { outer_html },
                ..
            }) => {
                debug!(
                    selector = %self.selector,
                    bytes = outer_html.len(),
                    "DOM.getOuterHTML returned empty payload; falling back to DOM.describeNode walk"
                );
            }
            Err(e) => {
                failed_backends.push("DOM.getOuterHTML");
                debug!(
                    selector = %self.selector,
                    error = %e,
                    "DOM.getOuterHTML failed; falling back to DOM.describeNode walk"
                );
            }
        }

        match self.outer_html_via_rust_walk().await {
            Ok(html) if !html.trim().is_empty() => Ok(OuterHtmlResult::Content(html)),
            Ok(_) => {
                if failed_backends.is_empty() {
                    // Every backend returned an empty payload (no errors
                    // raised). Surface this as `Empty` rather than `Failed`.
                    Ok(OuterHtmlResult::Empty)
                } else {
                    // At least one backend errored and the other returned
                    // empty — surface as `Failed` so callers can
                    // distinguish "nothing to serialize" from "backends
                    // broke".
                    Ok(OuterHtmlResult::Failed {
                        backends: failed_backends,
                    })
                }
            }
            Err(e) => {
                failed_backends.push("DOM.describeNode-walk");
                debug!(
                    selector = %self.selector,
                    error = %e,
                    "Rust-side DOM.describeNode walk failed"
                );
                Ok(OuterHtmlResult::Failed {
                    backends: failed_backends,
                })
            }
        }
    }

    /// Rust-side fallback: `DOM.describeNode` with `depth = -1` and
    /// `objectId` returns the entire subtree rooted at the target node;
    /// we walk it locally and emit HTML using [`serialize_node_tree`].
    async fn outer_html_via_rust_walk(&self) -> Result<String> {
        use chromiumoxide::cdp::browser_protocol::dom::DescribeNodeParams;
        use chromiumoxide::types::CommandResponse;

        let described: CommandResponse<
            chromiumoxide::cdp::browser_protocol::dom::DescribeNodeReturns,
        > = timeout(
            self.cdp_timeout,
            self.page.execute(
                DescribeNodeParams::builder()
                    // Use stable RemoteObjectId rather than ephemeral NodeId
                    // for the same reason as outer_html_recursive — NodeIds
                    // become stale after SPA DOM mutations.
                    .object_id(self.element.remote_object_id.clone())
                    .depth(-1)
                    .build(),
            ),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "NodeHandle::outer_html_via_rust_walk".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| self.cdp_err_or_stale(&e, "outer_html_via_rust_walk"))?;

        Ok(serialize_node_tree(&described.node))
    }

    async fn outer_html_via_js(&self) -> Result<String> {
        let returns = timeout(
            self.cdp_timeout,
            self.element.call_js_fn(
                r"function() {
                    if (typeof this.outerHTML === 'string' && this.outerHTML.length > 0) {
                        return this.outerHTML;
                    }
                    try {
                        return new XMLSerializer().serializeToString(this);
                    } catch (_) {
                        return '';
                    }
                }",
                true,
            ),
        )
        .await
        .map_err(|_| BrowserError::Timeout {
            operation: "NodeHandle::outer_html_via_js".to_string(),
            duration_ms: u64::try_from(self.cdp_timeout.as_millis()).unwrap_or(u64::MAX),
        })?
        .map_err(|e| self.cdp_err_or_stale(&e, "outer_html_via_js"))?;

        Ok(returns
            .result
            .value
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string())
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
    #[must_use]
    pub const fn inner(&self) -> &Page {
        &self.page
    }

    /// Close this page (tab).
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Timeout`] when the close call does not
    /// complete within the 5-second timeout, and
    /// [`BrowserError::CdpError`] for underlying chromiumoxide failures
    /// while issuing the `Page.close` CDP command.
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
        use crate::diagnostic::{CheckResult, DiagnosticReport, all_checks, all_limitation_probes};

        let mut results: Vec<CheckResult> = Vec::new();
        let mut known_limitations = Vec::new();

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

        for probe in all_limitation_probes() {
            let limitation = match self.eval::<String>(probe.script).await {
                Ok(json) => probe.parse_output(&json),
                Err(error) => Some(crate::diagnostic::KnownLimitation {
                    id: probe.id,
                    description: probe.description.to_string(),
                    details: format!("script error: {error}"),
                }),
            };
            if let Some(limitation) = limitation {
                tracing::debug!(
                    limitation = ?limitation.id,
                    details = %limitation.details,
                    "stealth limitation observed"
                );
                known_limitations.push(limitation);
            }
        }

        Ok(DiagnosticReport::new(results).with_known_limitations(known_limitations))
    }

    /// Run stealth checks and attach transport diagnostics (JA3/JA4/HTTP3).
    ///
    /// # Errors
    ///
    /// Propagates any [`BrowserError`] returned by the inner
    /// [`Self::verify_stealth`] call (which surfaces CDP / selector /
    /// evaluation failures from the underlying stealth probe). The
    /// `navigator.userAgent` read uses `eval` and is best-effort — its
    /// failure is logged and downgraded to an empty string so the
    /// transport-diagnostic block can still be attached.
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

    /// Try each selector in `selectors` in order and return the extracted
    /// results from the **first** selector that matches at least one node.
    ///
    /// This is useful when a page may use different markup across versions or
    /// A/B variants — supply the preferred selector first and progressively
    /// wider fallbacks afterwards.
    ///
    /// Returns an empty `Vec` only when *all* selectors match zero nodes
    /// (i.e. the element is genuinely absent from the page).  A non-empty
    /// intermediate selector result that then fails during extraction **will**
    /// return an error.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if the selector query fails, or
    /// [`BrowserError::ExtractionFailed`] if a matched node fails extraction.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use stygian_browser::extract::Extract;
    ///
    /// #[derive(Extract)]
    /// struct Headline { title: String }
    ///
    /// # async fn run(page: &stygian_browser::PageHandle) -> stygian_browser::error::Result<()> {
    /// // Try modern selector first, fall back to legacy markup.
    /// let items = page
    ///     .extract_all_with_fallback::<Headline>(&["h2.headline", "h2.title", "h2"])
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_all_with_fallback<T>(&self, selectors: &[&str]) -> Result<Vec<T>>
    where
        T: crate::extract::Extractable,
    {
        use futures::future::try_join_all;

        for &selector in selectors {
            let nodes = self.query_selector_all(selector).await?;
            if nodes.is_empty() {
                continue;
            }
            return try_join_all(nodes.iter().map(|n| T::extract_from(n)))
                .await
                .map_err(BrowserError::ExtractionFailed);
        }

        Ok(vec![])
    }

    /// Extract from every node matching `selector`, **skipping** nodes where
    /// a required field is absent (i.e. [`ExtractionError::Missing`]).
    ///
    /// Unlike [`extract_all`], this method is lenient about structural
    /// mismatches: nodes that fail with [`ExtractionError::Missing`] are
    /// silently dropped from the result set.  All other extraction errors
    /// (CDP failures, stale nodes, nested errors) still propagate as hard
    /// failures.
    ///
    /// This is useful when scraping heterogeneous lists where some items
    /// lack an optional field that your struct treats as required.
    ///
    /// [`extract_all`]: Self::extract_all
    /// [`ExtractionError::Missing`]: crate::extract::ExtractionError::Missing
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::CdpError`] if the selector query fails, or
    /// [`BrowserError::ExtractionFailed`] for non-`Missing` extraction errors.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use stygian_browser::extract::Extract;
    ///
    /// #[derive(Extract)]
    /// struct Price { amount: String }
    ///
    /// # async fn run(page: &stygian_browser::PageHandle) -> stygian_browser::error::Result<()> {
    /// // Products without a price tag are silently skipped.
    /// let prices = page.extract_resilient::<Price>(".product").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_resilient<T>(&self, selector: &str) -> Result<Vec<T>>
    where
        T: crate::extract::Extractable,
    {
        use crate::extract::ExtractionError;

        let nodes = self.query_selector_all(selector).await?;
        let mut results = Vec::with_capacity(nodes.len());

        for node in &nodes {
            match T::extract_from(node).await {
                Ok(item) => results.push(item),
                Err(ExtractionError::Missing { .. }) => {
                    tracing::debug!(
                        selector,
                        "extract_resilient: skipping node with missing required field"
                    );
                }
                Err(e) => return Err(BrowserError::ExtractionFailed(e)),
            }
        }

        Ok(results)
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
    pub const fn into_wait_until(self) -> WaitUntil {
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
        self.navigate(
            &options.url,
            options.wait.clone().into_wait_until(),
            nav_timeout,
        )
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

// ─── Rust-side CDP Node → HTML serializer (Recursive fallback) ───────────────

/// CDP `DOM.Node.nodeType` constants (matches the WHATWG DOM spec).
mod node_type {
    /// `Element` node.
    pub const ELEMENT: i64 = 1;
    /// Text node (`Text`).
    pub const TEXT: i64 = 3;
    /// `CDATASection` node.
    pub const CDATA_SECTION: i64 = 4;
    /// `ProcessingInstruction` node.
    pub const PROCESSING_INSTRUCTION: i64 = 7;
    /// `Comment` node.
    pub const COMMENT: i64 = 8;
    /// `Document` node.
    pub const DOCUMENT: i64 = 9;
    /// `DocumentType` node.
    pub const DOCUMENT_TYPE: i64 = 10;
    /// `DocumentFragment` node.
    pub const DOCUMENT_FRAGMENT: i64 = 11;
}

/// HTML elements that have no closing tag (per the WHATWG spec).
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "meta", "param",
    "source", "track", "wbr",
];

/// Serialise a CDP `Node` subtree (rooted at `node`) to an HTML string.
///
/// Used by [`NodeHandle::outer_html_via_rust_walk`] as the
/// [`OuterHtmlStrategy::Recursive`] fallback when `DOM.getOuterHTML`
/// returns an empty payload or errors out. The implementation is a
/// straightforward depth-first walk that mirrors what Chromium's own
/// `Element.outerHTML` accessor produces for the same tree:
/// - element nodes emit `<tag attrs>children</tag>`. [`VOID_ELEMENTS`]
///   emit `<tag attrs>` with no closing slash and no children, matching
///   Chromium's `outerHTML` byte-for-byte (which uses HTML5 syntax, not
///   XHTML self-closing).
/// - text nodes are HTML-escaped
/// - comment nodes emit `<!--value-->`
/// - `<!DOCTYPE …>` declarations are emitted for `DocumentType` roots
/// - `Document` / `DocumentFragment` roots emit only their children
///   (no outer wrapper), matching how `XMLSerializer` treats them
/// - `template` content (`template_content`) is inlined as additional
///   children of the `<template>` element, mirroring browser behaviour
/// - shadow roots are inlined as additional children of their host
///   (no `<shadowroot>` wrapper, since shadow content is what
///   `outerHTML` is expected to surface)
///
/// This serializer is not intended to be a perfect drop-in for
/// `Element.outerHTML` on every edge case (`CDATA`, `ProcessingInstruction`,
/// and namespace prefixes are simplified) — it is the second-line fallback
/// for the `Recursive` strategy and only fires when `DOM.getOuterHTML`
/// itself fails.
fn serialize_node_tree(node: &chromiumoxide::cdp::browser_protocol::dom::Node) -> String {
    let mut out = String::new();
    serialize_node_into(&mut out, node);
    out
}

fn serialize_node_into(out: &mut String, node: &chromiumoxide::cdp::browser_protocol::dom::Node) {
    match node.node_type {
        node_type::ELEMENT => {
            let tag = node.local_name.as_str();
            out.push('<');
            out.push_str(tag);
            if let Some(attrs) = &node.attributes {
                for pair in attrs.chunks_exact(2) {
                    if let [name, value] = pair {
                        out.push(' ');
                        escape_attr_name(out, name);
                        out.push_str("=\"");
                        escape_attr_value(out, value);
                        out.push('"');
                    }
                }
            }
            if VOID_ELEMENTS.contains(&tag) {
                out.push('>');
                return;
            }
            out.push('>');
            serialize_inline_children(out, node);
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        node_type::TEXT => {
            escape_text(out, &node.node_value);
        }
        node_type::COMMENT => {
            out.push_str("<!--");
            out.push_str(&node.node_value);
            out.push_str("-->");
        }
        node_type::DOCUMENT | node_type::DOCUMENT_FRAGMENT => {
            serialize_inline_children(out, node);
        }
        node_type::DOCUMENT_TYPE => {
            out.push_str("<!DOCTYPE ");
            out.push_str(&node.node_name);
            if let Some(public_id) = &node.public_id {
                out.push(' ');
                out.push_str(public_id);
            }
            if let Some(system_id) = &node.system_id {
                out.push(' ');
                out.push_str(system_id);
            }
            out.push('>');
        }
        node_type::CDATA_SECTION => {
            out.push_str("<![CDATA[");
            out.push_str(&node.node_value);
            out.push_str("]]>");
        }
        node_type::PROCESSING_INSTRUCTION => {
            out.push_str("<?");
            out.push_str(&node.node_name);
            if !node.node_value.is_empty() {
                out.push(' ');
                out.push_str(&node.node_value);
            }
            out.push_str("?>");
        }
        _ => {
            if !node.node_value.is_empty() {
                escape_text(out, &node.node_value);
            }
        }
    }
}

/// Emit the inline children of a node (regular `children`, plus
/// `template_content`, `shadow_roots`, and `content_document`) in the order
/// Chromium's own `Element.outerHTML` accessor surfaces them.
fn serialize_inline_children(
    out: &mut String,
    node: &chromiumoxide::cdp::browser_protocol::dom::Node,
) {
    if let Some(children) = &node.children {
        for child in children {
            serialize_node_into(out, child);
        }
    }
    if let Some(template_content) = &node.template_content {
        serialize_node_into(out, template_content);
    }
    if let Some(shadow_roots) = &node.shadow_roots {
        for shadow in shadow_roots {
            serialize_node_into(out, shadow);
        }
    }
    if let Some(content_document) = &node.content_document {
        serialize_node_into(out, content_document);
    }
}

/// Escape a text node payload for safe inclusion in HTML element content.
fn escape_text(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// Escape an attribute name (same rules as text — `&` and `<` cannot appear
/// in well-formed attribute names but are escaped defensively).
fn escape_attr_name(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Escape an attribute value for inclusion inside `"…"` quoted form.
fn escape_attr_value(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
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

    /// Verify the resilient extractor correctly classifies `ExtractionError`
    /// variants — `Missing` must be treated as "skip", others as hard errors.
    #[cfg(feature = "extract")]
    #[test]
    fn extraction_error_missing_is_skippable() {
        use crate::extract::ExtractionError;

        let missing = ExtractionError::Missing {
            field: "title",
            selector: "h1",
        };
        assert!(
            matches!(missing, ExtractionError::Missing { .. }),
            "ExtractionError::Missing should be the skip variant"
        );

        // Non-Missing variants should NOT match the skip pattern
        let nested = ExtractionError::Nested {
            field: "link",
            source: Box::new(ExtractionError::Missing {
                field: "href",
                selector: "a",
            }),
        };
        assert!(
            !matches!(nested, ExtractionError::Missing { .. }),
            "ExtractionError::Nested must not match Missing"
        );
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

    // ── OuterHtmlStrategy / OuterHtmlResult type tests (T101) ─────────────────

    #[test]
    fn outer_html_strategy_default_is_current() {
        assert_eq!(OuterHtmlStrategy::default(), OuterHtmlStrategy::Current);
    }

    #[test]
    fn outer_html_strategy_as_str_matches_variant() {
        assert_eq!(OuterHtmlStrategy::Current.as_str(), "Current");
        assert_eq!(OuterHtmlStrategy::Recursive.as_str(), "Recursive");
    }

    #[test]
    fn outer_html_strategy_display_matches_as_str() {
        assert_eq!(
            format!("{}", OuterHtmlStrategy::Current),
            OuterHtmlStrategy::Current.as_str()
        );
        assert_eq!(
            format!("{}", OuterHtmlStrategy::Recursive),
            OuterHtmlStrategy::Recursive.as_str()
        );
    }

    #[test]
    fn outer_html_strategy_is_copy_and_eq() {
        let s = OuterHtmlStrategy::Recursive;
        let copy = s;
        assert_eq!(s, copy);
        assert_eq!(s, OuterHtmlStrategy::Recursive);
        assert_ne!(s, OuterHtmlStrategy::Current);
    }

    #[test]
    fn outer_html_strategy_all_iterates_both_variants() {
        let all = OuterHtmlStrategy::all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], OuterHtmlStrategy::Current);
        assert_eq!(all[1], OuterHtmlStrategy::Recursive);
    }

    #[test]
    fn outer_html_strategy_serialize_round_trip()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for variant in OuterHtmlStrategy::all() {
            let json = serde_json::to_string(&variant)?;
            let restored: OuterHtmlStrategy = serde_json::from_str(&json)?;
            assert_eq!(restored, variant);
        }
        Ok(())
    }

    #[test]
    fn outer_html_result_content_returns_some_for_content() {
        let r = OuterHtmlResult::Content("<div/>".to_string());
        assert_eq!(r.content(), Some("<div/>"));
    }

    #[test]
    fn outer_html_result_content_returns_none_for_empty() {
        assert_eq!(OuterHtmlResult::Empty.content(), None);
    }

    #[test]
    fn outer_html_result_content_returns_none_for_failed() {
        let r = OuterHtmlResult::Failed {
            backends: vec!["DOM.getOuterHTML"],
        };
        assert_eq!(r.content(), None);
    }

    #[test]
    fn outer_html_result_is_empty_variants() {
        assert!(OuterHtmlResult::Empty.is_empty());
        assert!(
            OuterHtmlResult::Failed {
                backends: vec!["a"]
            }
            .is_empty()
        );
        assert!(!OuterHtmlResult::Content("<x/>".to_string()).is_empty());
        assert!(OuterHtmlResult::Content(String::new()).is_empty());
    }

    #[test]
    fn outer_html_result_display_includes_state() {
        assert_eq!(format!("{}", OuterHtmlResult::Empty), "Empty");
        assert_eq!(
            format!("{}", OuterHtmlResult::Content("<div/>".to_string())),
            "Content(6 bytes)"
        );
        let failed = OuterHtmlResult::Failed {
            backends: vec!["DOM.getOuterHTML", "DOM.describeNode-walk"],
        };
        let s = format!("{failed}");
        assert!(s.contains("DOM.getOuterHTML"));
        assert!(s.contains("DOM.describeNode-walk"));
    }

    #[test]
    fn outer_html_result_serializes_each_variant()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let empty_json = serde_json::to_string(&OuterHtmlResult::Empty)?;
        assert_eq!(empty_json, "\"Empty\"");

        let content_json =
            serde_json::to_string(&OuterHtmlResult::Content("<p>x</p>".to_string()))?;
        assert_eq!(content_json, r#"{"Content":"<p>x</p>"}"#);

        let failed_json = serde_json::to_string(&OuterHtmlResult::Failed {
            backends: vec!["DOM.getOuterHTML", "DOM.describeNode-walk"],
        })?;
        assert_eq!(
            failed_json,
            r#"{"Failed":{"backends":["DOM.getOuterHTML","DOM.describeNode-walk"]}}"#
        );
        Ok(())
    }

    // ── Rust-side CDP Node → HTML serializer tests (T101) ─────────────────────

    use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, Node, NodeId};

    fn mk_node(
        node_type: i64,
        local_name: &str,
        node_name: &str,
        node_value: &str,
        attributes: Option<Vec<String>>,
        children: Option<Vec<Node>>,
    ) -> Node {
        Node {
            node_id: NodeId::default(),
            parent_id: None,
            backend_node_id: BackendNodeId::default(),
            node_type,
            node_name: node_name.to_string(),
            local_name: local_name.to_string(),
            node_value: node_value.to_string(),
            child_node_count: None,
            children,
            attributes,
            document_url: None,
            base_url: None,
            public_id: None,
            system_id: None,
            internal_subset: None,
            xml_version: None,
            name: None,
            value: None,
            pseudo_type: None,
            pseudo_identifier: None,
            shadow_root_type: None,
            frame_id: None,
            content_document: None,
            shadow_roots: None,
            template_content: None,
            pseudo_elements: None,
            distributed_nodes: None,
            is_svg: None,
            compatibility_mode: None,
            assigned_slot: None,
            is_scrollable: None,
            affected_by_starting_styles: None,
            adopted_style_sheets: None,
        }
    }

    #[test]
    fn serialize_element_with_text_child() {
        let text = mk_node(node_type::TEXT, "", "", "hello", None, None);
        let div = mk_node(node_type::ELEMENT, "div", "DIV", "", None, Some(vec![text]));
        assert_eq!(serialize_node_tree(&div), "<div>hello</div>");
    }

    #[test]
    fn serialize_element_with_attributes() {
        let div = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            Some(vec![
                "id".into(),
                "main".into(),
                "class".into(),
                "container wide".into(),
            ]),
            None,
        );
        assert_eq!(
            serialize_node_tree(&div),
            r#"<div id="main" class="container wide"></div>"#
        );
    }

    #[test]
    fn serialize_void_element_emits_self_closing() {
        let img = mk_node(
            node_type::ELEMENT,
            "img",
            "IMG",
            "",
            Some(vec!["src".into(), "/a.png".into()]),
            None,
        );
        assert_eq!(serialize_node_tree(&img), r#"<img src="/a.png">"#);
        let br = mk_node(node_type::ELEMENT, "br", "BR", "", None, None);
        assert_eq!(serialize_node_tree(&br), "<br>");
    }

    #[test]
    fn serialize_nested_elements() {
        let p = mk_node(
            node_type::ELEMENT,
            "p",
            "P",
            "",
            None,
            Some(vec![mk_node(
                node_type::TEXT,
                "",
                "",
                "Mesh content here",
                None,
                None,
            )]),
        );
        let section = mk_node(
            node_type::ELEMENT,
            "section",
            "SECTION",
            "",
            None,
            Some(vec![p]),
        );
        let html = serialize_node_tree(&section);
        assert_eq!(html, "<section><p>Mesh content here</p></section>");
    }

    #[test]
    fn serialize_text_escapes_special_chars() {
        let n = mk_node(node_type::TEXT, "", "", "a < b && c > d", None, None);
        assert_eq!(serialize_node_tree(&n), "a &lt; b &amp;&amp; c &gt; d");
    }

    #[test]
    fn serialize_attribute_value_escapes_quotes_and_amp() {
        let div = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            Some(vec!["title".into(), "a & b \"c\"".into()]),
            None,
        );
        assert_eq!(
            serialize_node_tree(&div),
            r#"<div title="a &amp; b &quot;c&quot;"></div>"#
        );
    }

    #[test]
    fn serialize_attribute_name_escapes_special_chars() {
        let div = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            Some(vec!["weird<\"&".into(), "v".into()]),
            None,
        );
        assert_eq!(
            serialize_node_tree(&div),
            r#"<div weird&lt;&quot;&amp;="v"></div>"#
        );
    }

    #[test]
    fn serialize_comment_node() {
        let n = mk_node(node_type::COMMENT, "", "", " a comment ", None, None);
        assert_eq!(serialize_node_tree(&n), "<!-- a comment -->");
    }

    #[test]
    fn serialize_document_root_flattens_children() {
        let html = mk_node(
            node_type::ELEMENT,
            "html",
            "HTML",
            "",
            None,
            Some(vec![mk_node(
                node_type::ELEMENT,
                "body",
                "BODY",
                "",
                None,
                None,
            )]),
        );
        let doc = mk_node(
            node_type::DOCUMENT,
            "",
            "#document",
            "",
            None,
            Some(vec![html]),
        );
        assert_eq!(serialize_node_tree(&doc), "<html><body></body></html>");
    }

    #[test]
    fn serialize_document_fragment_root_flattens_children() {
        let span = mk_node(
            node_type::ELEMENT,
            "span",
            "SPAN",
            "",
            None,
            Some(vec![mk_node(node_type::TEXT, "", "", "x", None, None)]),
        );
        let frag = mk_node(
            node_type::DOCUMENT_FRAGMENT,
            "",
            "#document-fragment",
            "",
            None,
            Some(vec![span]),
        );
        assert_eq!(serialize_node_tree(&frag), "<span>x</span>");
    }

    #[test]
    fn serialize_doctype_node() {
        let dt = Node {
            public_id: Some("-//W3C//DTD HTML 4.01//EN".to_string()),
            system_id: Some("http://www.w3.org/TR/html4/strict.dtd".to_string()),
            ..mk_node(node_type::DOCUMENT_TYPE, "", "html", "", None, None)
        };
        assert_eq!(
            serialize_node_tree(&dt),
            "<!DOCTYPE html -//W3C//DTD HTML 4.01//EN http://www.w3.org/TR/html4/strict.dtd>"
        );
    }

    #[test]
    fn serialize_doctype_node_no_ids() {
        let dt = mk_node(node_type::DOCUMENT_TYPE, "", "html", "", None, None);
        assert_eq!(serialize_node_tree(&dt), "<!DOCTYPE html>");
    }

    #[test]
    fn serialize_cdata_section() {
        let n = mk_node(node_type::CDATA_SECTION, "", "", "raw & <data>", None, None);
        assert_eq!(serialize_node_tree(&n), "<![CDATA[raw & <data>]]>");
    }

    #[test]
    fn serialize_processing_instruction() {
        let n = mk_node(
            node_type::PROCESSING_INSTRUCTION,
            "",
            "xml-stylesheet",
            "href=\"style.css\"",
            None,
            None,
        );
        assert_eq!(
            serialize_node_tree(&n),
            "<?xml-stylesheet href=\"style.css\"?>"
        );
    }

    #[test]
    fn serialize_template_inlines_template_content() {
        let inner = mk_node(
            node_type::ELEMENT,
            "span",
            "SPAN",
            "",
            None,
            Some(vec![mk_node(node_type::TEXT, "", "", "tmpl", None, None)]),
        );
        let mut tmpl = mk_node(node_type::ELEMENT, "template", "TEMPLATE", "", None, None);
        tmpl.template_content = Some(Box::new(inner));
        assert_eq!(
            serialize_node_tree(&tmpl),
            "<template><span>tmpl</span></template>"
        );
    }

    #[test]
    fn serialize_shadow_roots_inlined_into_host() {
        let shadow_text = mk_node(node_type::TEXT, "", "", "shadow-text", None, None);
        let shadow = Node {
            shadow_root_type: Some(chromiumoxide::cdp::browser_protocol::dom::ShadowRootType::Open),
            ..mk_node(
                node_type::DOCUMENT_FRAGMENT,
                "",
                "#document-fragment",
                "",
                None,
                Some(vec![mk_node(
                    node_type::ELEMENT,
                    "span",
                    "SPAN",
                    "",
                    None,
                    Some(vec![shadow_text]),
                )]),
            )
        };
        let mut host = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            None,
            Some(vec![mk_node(node_type::TEXT, "", "", "light", None, None)]),
        );
        host.shadow_roots = Some(vec![shadow]);
        assert_eq!(
            serialize_node_tree(&host),
            "<div>light<span>shadow-text</span></div>"
        );
    }

    #[test]
    fn serialize_deeply_nested_subtree() {
        // Build a 5-level deep subtree: <a><b><c><d><e>deep</e></d></c></b></a>
        let tag_e = mk_node(
            node_type::ELEMENT,
            "e",
            "E",
            "",
            None,
            Some(vec![mk_node(node_type::TEXT, "", "", "deep", None, None)]),
        );
        let tag_d = mk_node(node_type::ELEMENT, "d", "D", "", None, Some(vec![tag_e]));
        let tag_c = mk_node(node_type::ELEMENT, "c", "C", "", None, Some(vec![tag_d]));
        let tag_b = mk_node(node_type::ELEMENT, "b", "B", "", None, Some(vec![tag_c]));
        let tag_a = mk_node(node_type::ELEMENT, "a", "A", "", None, Some(vec![tag_b]));
        assert_eq!(
            serialize_node_tree(&tag_a),
            "<a><b><c><d><e>deep</e></d></c></b></a>"
        );
    }

    #[test]
    fn serialize_element_with_text_and_element_children() {
        let span = mk_node(
            node_type::ELEMENT,
            "span",
            "SPAN",
            "",
            None,
            Some(vec![mk_node(node_type::TEXT, "", "", "inline", None, None)]),
        );
        let div = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            None,
            Some(vec![
                mk_node(node_type::TEXT, "", "", "before", None, None),
                span,
                mk_node(node_type::TEXT, "", "", "after", None, None),
            ]),
        );
        assert_eq!(
            serialize_node_tree(&div),
            "<div>before<span>inline</span>after</div>"
        );
    }

    #[test]
    fn serialize_attribute_pairs_drop_orphans() {
        // An odd-length attribute list (one name with no value) must not crash.
        let div = mk_node(
            node_type::ELEMENT,
            "div",
            "DIV",
            "",
            Some(vec!["orphan".into()]),
            None,
        );
        // The orphan name has no value so it is silently skipped (pairs of 2).
        assert_eq!(serialize_node_tree(&div), "<div></div>");
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
    fn warmup_options_serialize_round_trip() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
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
    fn refresh_options_serialize_round_trip() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
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
    fn warmup_report_serialize_round_trip() -> std::result::Result<(), Box<dyn std::error::Error>> {
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
    fn refresh_report_serialize_round_trip() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
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
    #[allow(clippy::expect_used)]
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
    #[allow(clippy::expect_used)]
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

            let report = page
                .refresh(RefreshOptions::default())
                .await
                .expect("refresh");

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
