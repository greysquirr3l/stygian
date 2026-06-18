//! Selector types for DOM element selection

use serde::{Deserialize, Serialize};

/// A selector for finding DOM elements
///
/// Supports both CSS selectors (fast, reliable) and `XPath` (more powerful, fragile).
/// The plugin generates both and tries CSS first, falling back to `XPath`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Selector {
    /// CSS selector (preferred, tried first)
    Css(String),

    /// `XPath` expression (fallback)
    XPath(String),

    /// Both CSS and `XPath`; try CSS first
    Both { css: String, xpath: String },
}

impl Selector {
    /// Create a CSS selector
    pub fn css(selector: impl Into<String>) -> Self {
        Self::Css(selector.into())
    }

    /// Create an `XPath` selector
    pub fn xpath(xpath: impl Into<String>) -> Self {
        Self::XPath(xpath.into())
    }

    /// Create a dual selector (prefer CSS)
    pub fn dual(css: impl Into<String>, xpath: impl Into<String>) -> Self {
        Self::Both {
            css: css.into(),
            xpath: xpath.into(),
        }
    }

    /// Get the primary selector (CSS if available, otherwise `XPath`)
    #[must_use]
    pub fn primary(&self) -> &str {
        match self {
            Self::Css(s) | Self::XPath(s) => s,
            Self::Both { css, .. } => css,
        }
    }

    /// Get the fallback selector (`XPath` if available)
    #[must_use]
    pub fn fallback(&self) -> Option<&str> {
        match self {
            Self::Both { xpath, .. } => Some(xpath),
            _ => None,
        }
    }

    /// Validate selector syntax (basic checks)
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::PluginError::SelectorError`] when the CSS
    /// selector is empty, references an unknown pseudo-element, the `XPath`
    /// expression is empty, or the `XPath` brackets are unbalanced.
    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Css(s) => {
                if s.is_empty() {
                    return Err(crate::error::PluginError::SelectorError {
                        selector: s.clone(),
                        reason: "CSS selector cannot be empty".to_string(),
                    });
                }
                // Basic CSS validation: check for common syntax errors
                if s.contains("::") && !is_valid_css_pseudo_element(s) {
                    return Err(crate::error::PluginError::SelectorError {
                        selector: s.clone(),
                        reason: "Invalid CSS pseudo-element".to_string(),
                    });
                }
                Ok(())
            }
            Self::XPath(s) => {
                if s.is_empty() {
                    return Err(crate::error::PluginError::SelectorError {
                        selector: s.clone(),
                        reason: "XPath expression cannot be empty".to_string(),
                    });
                }
                // Basic XPath validation: check for balanced brackets
                if !is_balanced_xpath(s) {
                    return Err(crate::error::PluginError::SelectorError {
                        selector: s.clone(),
                        reason: "XPath expression has unbalanced brackets".to_string(),
                    });
                }
                Ok(())
            }
            Self::Both { css, xpath } => {
                Self::Css(css.clone()).validate()?;
                Self::XPath(xpath.clone()).validate()?;
                Ok(())
            }
        }
    }
}

/// Check if a string is a valid CSS pseudo-element
fn is_valid_css_pseudo_element(s: &str) -> bool {
    matches!(
        s,
        _ if s.contains("::before")
            || s.contains("::after")
            || s.contains("::first-line")
            || s.contains("::first-letter")
            || s.contains("::selection")
            || s.contains("::placeholder")
    )
}

/// Check if an `XPath` has balanced brackets
fn is_balanced_xpath(s: &str) -> bool {
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Css(s) => write!(f, "CSS({s})"),
            Self::XPath(s) => write!(f, "XPath({s})"),
            Self::Both { css, xpath } => write!(f, "Both(CSS: {css}, XPath: {xpath})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_css_selector_creation() {
        let sel = Selector::css(".my-class");
        assert_eq!(sel.primary(), ".my-class");
        assert_eq!(sel.fallback(), None);
    }

    #[test]
    fn test_xpath_selector_creation() {
        let sel = Selector::xpath("//div[@id='main']");
        assert_eq!(sel.primary(), "//div[@id='main']");
        assert_eq!(sel.fallback(), None);
    }

    #[test]
    fn test_dual_selector() {
        let sel = Selector::dual(".product", "//div[@class='product']");
        assert_eq!(sel.primary(), ".product");
        assert_eq!(sel.fallback(), Some("//div[@class='product']"));
    }

    #[test]
    fn test_empty_selector_validation() {
        let sel = Selector::css("");
        assert!(sel.validate().is_err());
    }

    #[test]
    fn test_unbalanced_xpath_validation() {
        let sel = Selector::xpath("//div[@id='main'");
        assert!(sel.validate().is_err());
    }
}
