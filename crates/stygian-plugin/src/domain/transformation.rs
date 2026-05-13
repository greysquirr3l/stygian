//! Transformation pipeline for extracted data

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A transformation to apply to extracted values
///
/// Transformations are chained in order to clean, normalize, and validate extracted data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum Transformation {
    /// Trim whitespace from start and end
    Trim,

    /// Convert to lowercase
    Lowercase,

    /// Convert to uppercase
    Uppercase,

    /// Remove all whitespace
    RemoveWhitespace,

    /// Replace using a regex pattern
    Regex {
        pattern: String,
        replacement: String,
    },

    /// Extract using a regex capture group
    RegexExtract { pattern: String, group: usize },

    /// Coerce to a specific type
    Coerce {
        target_type: String, // "string", "number", "boolean", "date"
    },

    /// Keep only if matches a regex pattern
    Filter { pattern: String },

    /// Replace multiple consecutive whitespace with single space
    NormalizeWhitespace,

    /// Remove HTML tags
    StripHtml,

    /// Decode HTML entities
    DecodeHtml,

    /// Parse JSON string into object
    ParseJson,

    /// Custom JavaScript transformation (extension point)
    #[cfg(feature = "javascript")]
    JavaScript { code: String },
}

impl Transformation {
    fn apply_regex(value: &str, pattern: &str, replacement: &str) -> crate::Result<String> {
        let re = regex::Regex::new(pattern).map_err(|e| {
            crate::error::PluginError::InvalidTransformation(format!("Invalid regex: {e}"))
        })?;
        Ok(re.replace_all(value, replacement).to_string())
    }

    fn apply_regex_extract(value: &str, pattern: &str, group: usize) -> crate::Result<String> {
        let re = regex::Regex::new(pattern).map_err(|e| {
            crate::error::PluginError::InvalidTransformation(format!("Invalid regex: {e}"))
        })?;
        let caps = re.captures(value).ok_or_else(|| {
            crate::error::PluginError::ExtractionError(format!("No match for pattern: {pattern}"))
        })?;
        caps.get(group)
            .ok_or_else(|| {
                crate::error::PluginError::ExtractionError(format!(
                    "Capture group {group} not found"
                ))
            })
            .map(|m| m.as_str().to_string())
    }

    fn apply_coerce(value: &str, target_type: &str) -> crate::Result<String> {
        match target_type {
            "string" => Ok(value.to_string()),
            "number" => {
                value.parse::<f64>().map_err(|_| {
                    crate::error::PluginError::InvalidTransformation(format!(
                        "Cannot coerce '{value}' to number"
                    ))
                })?;
                Ok(value.to_string())
            }
            "boolean" => {
                let b = matches!(value.to_lowercase().as_str(), "true" | "yes" | "1");
                Ok(b.to_string())
            }
            "date" => {
                chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
                    crate::error::PluginError::InvalidTransformation(format!(
                        "Invalid date: {value}"
                    ))
                })?;
                Ok(value.to_string())
            }
            _ => Err(crate::error::PluginError::InvalidTransformation(format!(
                "Unknown type: {target_type}"
            ))),
        }
    }

    fn apply_filter(value: &str, pattern: &str) -> crate::Result<String> {
        let re = regex::Regex::new(pattern).map_err(|e| {
            crate::error::PluginError::InvalidTransformation(format!("Invalid regex: {e}"))
        })?;
        if re.is_match(value) {
            Ok(value.to_string())
        } else {
            Err(crate::error::PluginError::ExtractionError(
                "Value did not match filter pattern".to_string(),
            ))
        }
    }

    /// Apply this transformation to a value
    pub fn apply(&self, value: &str) -> crate::Result<String> {
        match self {
            Self::Trim => Ok(value.trim().to_string()),
            Self::Lowercase => Ok(value.to_lowercase()),
            Self::Uppercase => Ok(value.to_uppercase()),
            Self::RemoveWhitespace => Ok(value.chars().filter(|c| !c.is_whitespace()).collect()),
            Self::Regex {
                pattern,
                replacement,
            } => Self::apply_regex(value, pattern, replacement),
            Self::RegexExtract { pattern, group } => {
                Self::apply_regex_extract(value, pattern, *group)
            }
            Self::Coerce { target_type } => Self::apply_coerce(value, target_type),
            Self::Filter { pattern } => Self::apply_filter(value, pattern),
            Self::NormalizeWhitespace => Ok(value.split_whitespace().collect::<Vec<_>>().join(" ")),
            Self::StripHtml => {
                static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                    #[expect(clippy::expect_used, reason = "hardcoded regex pattern is valid")]
                    regex::Regex::new(r"<[^>]+>").expect("valid hardcoded HTML tag pattern")
                });
                Ok(RE.replace_all(value, "").to_string())
            }
            Self::DecodeHtml => Ok(value
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")),
            Self::ParseJson => serde_json::from_str::<Value>(value)
                .map(|v| v.to_string())
                .map_err(|e| crate::error::PluginError::InvalidTransformation(e.to_string())),
            #[cfg(feature = "javascript")]
            Self::JavaScript { code: _ } => Err(crate::error::PluginError::InvalidTransformation(
                "JavaScript transformations not yet implemented".to_string(),
            )),
        }
    }

    /// Apply a chain of transformations to a value
    pub fn apply_chain(transformations: &[Self], mut value: String) -> crate::Result<String> {
        for transformation in transformations {
            value = transformation.apply(&value)?;
        }
        Ok(value)
    }
}

impl std::fmt::Display for Transformation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trim => write!(f, "Trim"),
            Self::Lowercase => write!(f, "Lowercase"),
            Self::Uppercase => write!(f, "Uppercase"),
            Self::RemoveWhitespace => write!(f, "RemoveWhitespace"),
            Self::Regex { pattern, .. } => write!(f, "Regex({pattern})"),
            Self::RegexExtract { pattern, group } => {
                write!(f, "RegexExtract({pattern}, group {group})")
            }
            Self::Coerce { target_type } => write!(f, "Coerce({target_type})"),
            Self::Filter { pattern } => write!(f, "Filter({pattern})"),
            Self::NormalizeWhitespace => write!(f, "NormalizeWhitespace"),
            Self::StripHtml => write!(f, "StripHtml"),
            Self::DecodeHtml => write!(f, "DecodeHtml"),
            Self::ParseJson => write!(f, "ParseJson"),
            #[cfg(feature = "javascript")]
            Self::JavaScript { .. } => write!(f, "JavaScript"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::Trim;
        assert_eq!(t.apply("  hello  ")?, "hello");
        Ok(())
    }

    #[test]
    fn test_lowercase() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::Lowercase;
        assert_eq!(t.apply("HELLO")?, "hello");
        Ok(())
    }

    #[test]
    fn test_regex_replace() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::Regex {
            pattern: r"(\d{3})-(\d{4})".to_string(),
            replacement: "($1) $2".to_string(),
        };
        assert_eq!(t.apply("123-4567")?, "(123) 4567");
        Ok(())
    }

    #[test]
    fn test_regex_extract() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::RegexExtract {
            pattern: r"Price: \$(\d+\.\d{2})".to_string(),
            group: 1,
        };
        assert_eq!(t.apply("Price: $19.99")?, "19.99");
        Ok(())
    }

    #[test]
    fn test_coerce_number() {
        let t = Transformation::Coerce {
            target_type: "number".to_string(),
        };
        assert!(t.apply("123.45").is_ok());
        assert!(t.apply("not a number").is_err());
    }

    #[test]
    fn test_normalize_whitespace() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::NormalizeWhitespace;
        assert_eq!(t.apply("hello   world   foo")?, "hello world foo");
        Ok(())
    }

    #[test]
    fn test_strip_html() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let t = Transformation::StripHtml;
        assert_eq!(t.apply("<p>Hello <b>world</b></p>")?, "Hello world");
        Ok(())
    }

    #[test]
    fn test_transformation_chain() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let transforms = vec![
            Transformation::StripHtml,
            Transformation::Trim,
            Transformation::NormalizeWhitespace,
        ];
        let result =
            Transformation::apply_chain(&transforms, "  <p>hello   world</p>  ".to_string())?;
        assert_eq!(result, "hello world");
        Ok(())
    }
}
