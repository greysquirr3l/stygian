//! Typed DOM extraction via [`Extract`] derive macro.
//!
//! # Example
//!
//! ```no_run
//! use stygian_browser::extract::Extract;
//! use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use std::time::Duration;
//!
//! #[derive(Extract)]
//! struct Headline {
//!     #[selector("h2")]
//!     title: String,
//!     #[selector("a", attr = "href")]
//!     link: Option<String>,
//! }
//!
//! # async fn run() -> stygian_browser::error::Result<()> {
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let handle = pool.acquire().await?;
//! let mut page = handle.browser().expect("valid browser").new_page().await?;
//! page.navigate(
//!     "https://example.com",
//!     WaitUntil::DomContentLoaded,
//!     Duration::from_secs(30),
//! ).await?;
//! let items: Vec<Headline> = page.extract_all::<Headline>("article").await?;
//! # Ok(())
//! # }
//! ```

pub use stygian_extract_derive::Extract;

// ─── ExtractionError ─────────────────────────────────────────────────────────

/// An error produced during `#[derive(Extract)]`-driven extraction.
///
/// The [`CdpFailed`][Self::CdpFailed] variant boxes its [`crate::error::BrowserError`]
/// source to break the otherwise-infinite recursive type size between
/// `BrowserError::ExtractionFailed` and this enum.
#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    /// A required field's selector matched no element.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::extract::ExtractionError;
    /// let e = ExtractionError::Missing { field: "title", selector: "h2" };
    /// assert!(e.to_string().contains("title"));
    /// assert!(e.to_string().contains("h2"));
    /// ```
    #[error("required field `{field}` had no match for selector `{selector}`")]
    Missing {
        /// Name of the Rust struct field that required a match.
        field: &'static str,
        /// CSS selector that produced no results.
        selector: &'static str,
    },

    /// A CDP call inside extraction failed.
    ///
    /// The [`crate::error::BrowserError`] source is boxed to prevent an
    /// infinitely-sized recursive type (since `BrowserError` may itself contain
    /// an `ExtractionError` via its `ExtractionFailed` variant).
    #[error("CDP error extracting field `{field}`: {source}")]
    CdpFailed {
        /// Name of the struct field whose CDP call failed.
        field: &'static str,
        /// Underlying browser / CDP error.
        #[source]
        source: Box<crate::error::BrowserError>,
    },

    /// A `#[selector(..., nested)]` field's inner extraction failed.
    ///
    /// # Example
    ///
    /// ```
    /// use stygian_browser::extract::ExtractionError;
    /// let inner = ExtractionError::Missing { field: "href", selector: "a" };
    /// let e = ExtractionError::Nested {
    ///     field: "link",
    ///     source: Box::new(inner),
    /// };
    /// assert!(e.to_string().contains("link"));
    /// ```
    #[error("nested extraction for field `{field}` failed: {source}")]
    Nested {
        /// Name of the outer struct field that triggered nested extraction.
        field: &'static str,
        /// Inner extraction failure.
        #[source]
        source: Box<Self>,
    },
}

// ─── Extractable trait ───────────────────────────────────────────────────────

/// Types that can be extracted from a live DOM [`crate::page::NodeHandle`].
///
/// Implement this manually or derive it with `#[derive(Extract)]`.
///
/// # Example
///
/// ```no_run
/// use stygian_browser::extract::{Extractable, ExtractionError, Extract};
/// use stygian_browser::page::NodeHandle;
///
/// #[derive(Extract)]
/// struct Title {
///     #[selector("h1")]
///     text: String,
/// }
///
/// // `Title` now implements `Extractable` and can be passed to
/// // `PageHandle::extract_all::<Title>`.
/// ```
pub trait Extractable: Sized {
    /// Extract an instance of `Self` from the given DOM node.
    ///
    /// The node is the **root** element — selectors in the derive macro are
    /// evaluated relative to it via `children_matching`.
    ///
    /// # Errors
    ///
    /// Returns [`ExtractionError`] when a required field selector matches no
    /// element, when a CDP call fails, or when nested extraction fails.
    fn extract_from(
        node: &crate::page::NodeHandle,
    ) -> impl std::future::Future<Output = Result<Self, ExtractionError>> + Send;
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_error_missing_display() {
        let e = ExtractionError::Missing {
            field: "foo",
            selector: ".bar",
        };
        let msg = e.to_string();
        assert!(
            msg.contains("foo"),
            "display must contain field name 'foo'; got: {msg}"
        );
        assert!(
            msg.contains(".bar"),
            "display must contain selector '.bar'; got: {msg}"
        );
    }

    #[test]
    fn extraction_error_nested_display() {
        let inner = ExtractionError::Missing {
            field: "href",
            selector: "a",
        };
        let e = ExtractionError::Nested {
            field: "link",
            source: Box::new(inner),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("link"),
            "display must contain outer field name 'link'; got: {msg}",
        );
    }
}
