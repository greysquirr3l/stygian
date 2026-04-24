use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::types::TransactionView;

/// Errors returned while parsing HAR data.
#[derive(Debug, Error)]
pub enum HarError {
    /// HAR payload is not valid JSON.
    #[error("invalid HAR json: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// Expected HAR structure is missing required fields.
    #[error("invalid HAR structure: {0}")]
    InvalidStructure(&'static str),
}

/// Internal parsed HAR representation.
#[derive(Debug, Clone)]
pub struct ParsedHar {
    /// Page title from the HAR pages section when available.
    pub page_title: Option<String>,
    /// Parsed request transactions.
    pub requests: Vec<TransactionViewWithType>,
}

/// Transaction plus optional resource type.
#[derive(Debug, Clone)]
pub struct TransactionViewWithType {
    /// Transaction used by the classifier.
    pub transaction: TransactionView,
    /// Resource type (document/script/xhr/etc.) if present in HAR.
    pub resource_type: Option<String>,
}

impl TransactionViewWithType {
    /// Convenience accessor for URL.
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn url(&self) -> &str {
        &self.transaction.url
    }

    /// Convenience accessor for status.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.transaction.status
    }
}

impl From<TransactionViewWithType> for TransactionView {
    fn from(value: TransactionViewWithType) -> Self {
        value.transaction
    }
}

/// Parse a HAR JSON string into transactions usable by the classifier.
///
/// # Errors
///
/// Returns [`HarError::InvalidJson`] when `har_json` is not valid JSON,
/// or [`HarError::InvalidStructure`] when required HAR fields are missing.
pub fn parse_har_transactions(har_json: &str) -> Result<ParsedHar, HarError> {
    let root: Value = serde_json::from_str(har_json)?;

    let log = root
        .get("log")
        .ok_or(HarError::InvalidStructure("missing log object"))?;

    let page_title = log
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| pages.first())
        .and_then(|page| page.get("title"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let entries = log
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(HarError::InvalidStructure("missing entries array"))?;

    let mut requests: Vec<TransactionViewWithType> = Vec::new();

    for entry in entries {
        let request = entry
            .get("request")
            .ok_or(HarError::InvalidStructure("entry missing request"))?;
        let response = entry
            .get("response")
            .ok_or(HarError::InvalidStructure("entry missing response"))?;

        let url = request
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(HarError::InvalidStructure("entry request missing url"))?;

        let status = response
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|x| u16::try_from(x).ok())
            .ok_or(HarError::InvalidStructure("entry response missing status"))?;

        let headers = response
            .get("headers")
            .and_then(Value::as_array)
            .map(|headers| extract_headers(headers))
            .unwrap_or_default();

        let body_snippet = response
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .map(|text| text.chars().take(2_048).collect::<String>());

        let tx = TransactionView {
            url,
            status,
            response_headers: headers,
            response_body_snippet: body_snippet,
        };

        requests.push(TransactionViewWithType {
            transaction: tx,
            resource_type: entry
                .get("_resourceType")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }

    Ok(ParsedHar {
        page_title,
        requests,
    })
}

fn extract_headers(headers: &[Value]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for header in headers {
        let name = header
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let value = header
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned);

        if let (Some(k), Some(v)) = (name, value) {
            let _prev = out.insert(k, v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_har() {
        let json = r#"{
            "log": {
                "pages": [{"title": "https://example.com"}],
                "entries": [
                    {
                        "_resourceType": "document",
                        "request": {"url": "https://example.com"},
                        "response": {
                            "status": 403,
                            "headers": [{"name": "server", "value": "cloudflare"}],
                            "content": {"text": "Attention Required! | Cloudflare"}
                        }
                    }
                ]
            }
        }"#;

        let parsed_result = parse_har_transactions(json);
        assert!(parsed_result.is_ok(), "parse should succeed");

        let Ok(parsed) = parsed_result else {
            return;
        };

        assert_eq!(parsed.page_title.as_deref(), Some("https://example.com"));
        assert_eq!(parsed.requests.len(), 1);

        let first = parsed.requests.first();
        assert!(first.is_some(), "parsed requests unexpectedly empty");
        if let Some(first) = first {
            assert_eq!(first.status(), 403);
            assert_eq!(first.url(), "https://example.com");
        }
    }
}
