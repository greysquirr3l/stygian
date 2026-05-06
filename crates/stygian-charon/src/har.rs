use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::types::TransactionView;

const MAX_HAR_BYTES: usize = 10 * 1024 * 1024;
const MAX_HAR_ENTRIES: usize = 10_000;
const MAX_HEADERS_PER_ENTRY: usize = 256;
const MAX_URL_BYTES: usize = 8 * 1024;

/// Errors returned while parsing HAR data.
#[derive(Debug, Error)]
pub enum HarError {
    /// HAR payload is not valid JSON.
    #[error("invalid HAR json: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// Expected HAR structure is missing required fields.
    #[error("invalid HAR structure: {0}")]
    InvalidStructure(&'static str),
    /// HAR input exceeded a configured safety limit.
    #[error("har input exceeds safety limit: {0}")]
    LimitExceeded(&'static str),
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
/// [`HarError::InvalidStructure`] when required HAR fields are missing, or
/// [`HarError::LimitExceeded`] when input safety limits are exceeded.
pub fn parse_har_transactions(har_json: &str) -> Result<ParsedHar, HarError> {
    if har_json.len() > MAX_HAR_BYTES {
        return Err(HarError::LimitExceeded("har payload too large"));
    }

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

    if entries.len() > MAX_HAR_ENTRIES {
        return Err(HarError::LimitExceeded("too many HAR entries"));
    }

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

        if url.len() > MAX_URL_BYTES {
            return Err(HarError::LimitExceeded("request url too large"));
        }

        let status = response
            .get("status")
            .and_then(Value::as_u64)
            .and_then(|x| u16::try_from(x).ok())
            .ok_or(HarError::InvalidStructure("entry response missing status"))?;

        let headers = match response.get("headers").and_then(Value::as_array) {
            Some(headers) => {
                if headers.len() > MAX_HEADERS_PER_ENTRY {
                    return Err(HarError::LimitExceeded("too many response headers"));
                }
                extract_headers(headers)
            }
            None => BTreeMap::new(),
        };

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

    #[test]
    fn rejects_oversized_har_payload() {
        let oversized = " ".repeat(MAX_HAR_BYTES + 1);

        let result = parse_har_transactions(&oversized);

        assert!(matches!(
            result,
            Err(HarError::LimitExceeded("har payload too large"))
        ));
    }

    #[test]
    fn rejects_too_many_entries() {
        let entries = std::iter::repeat_n(
            r#"{"request":{"url":"https://example.com"},"response":{"status":200}}"#,
            MAX_HAR_ENTRIES + 1,
        )
        .collect::<Vec<_>>()
        .join(",");
        let json = format!(r#"{{"log":{{"entries":[{entries}]}}}}"#);

        let result = parse_har_transactions(&json);

        assert!(matches!(
            result,
            Err(HarError::LimitExceeded("too many HAR entries"))
        ));
    }

    #[test]
    fn rejects_too_many_response_headers() {
        let headers = std::iter::repeat_n(
            r#"{"name":"server","value":"cloudflare"}"#,
            MAX_HEADERS_PER_ENTRY + 1,
        )
        .collect::<Vec<_>>()
        .join(",");
        let json = format!(
            r#"{{"log":{{"entries":[{{"request":{{"url":"https://example.com"}},"response":{{"status":403,"headers":[{headers}]}}}}]}}}}"#
        );

        let result = parse_har_transactions(&json);

        assert!(matches!(
            result,
            Err(HarError::LimitExceeded("too many response headers"))
        ));
    }
}
