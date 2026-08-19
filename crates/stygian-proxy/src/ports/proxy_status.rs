//! RFC 9209 `Proxy-Status` header parser — port trait and value types.
//!
//! The 2026 scraping guide
//! (`docs/dev/project/scraping-guide-2026-llm-context.md` §"PROXY PROVIDERS
//! AND TYPES", L2718) calls out that proprietary proxy status codes are
//! the largest single source of false attribution: a health check that
//! can't tell provider failure from target failure is worse than no
//! health check at all.
//!
//! RFC 9209 standardises the `Proxy-Status` response header with the
//! grammar:
//!
//! ```text
//! Proxy-Status: <proxy-error>[";" *(parameter-name "=" parameter-value)]
//! ```
//!
//! See <https://www.rfc-editor.org/rfc/rfc9209.html> for the full ABNF.
//!
//! The error-class bucketing (`Network | Provider | Target | Unknown`)
//! is the project's own taxonomy — RFC 9209 only defines the
//! `proxy-error` token; we map the well-known error-types onto classes
//! so the circuit breaker and the diagnostics dashboard speak the
//! same vocabulary.

use std::fmt;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Categorised reason a proxy request failed (or succeeded).
///
/// `Unknown` covers cases where the header is present but the error
/// token isn't in our known mapping (we don't want a missing
/// classification to be silently re-categorised as a proxy failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyErrorClass {
    /// Connectivity failure between client and proxy, or proxy and
    /// upstream. Not the proxy's fault — network/infra.
    Network,
    /// Proxy itself failed: authentication, misconfiguration, internal
    /// error. Count against the proxy circuit breaker.
    Provider,
    /// Origin (target) returned an HTTP-level failure (4xx/5xx). Not
    /// the proxy's fault — do NOT count against the proxy circuit
    /// breaker.
    Target,
    /// Header is present but doesn't carry enough signal to classify.
    /// Or `Proxy-Status: 200` (proxy claims success).
    Unknown,
}

impl ProxyErrorClass {
    /// `true` if a failure in this class should increment the proxy
    /// circuit breaker.
    #[must_use]
    pub const fn counts_against_proxy(self) -> bool {
        matches!(self, Self::Provider)
    }
}

impl fmt::Display for ProxyErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => f.write_str("network"),
            Self::Provider => f.write_str("provider"),
            Self::Target => f.write_str("target"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// Per-next-hop info captured by multi-hop proxies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamInfo {
    /// `host:port` form (host may be IPv4, IPv6, or DNS name).
    pub address: String,
    /// Optional HTTP status observed when this hop forwarded the
    /// response back upstream.
    pub http_status: Option<u16>,
    /// Classification of this hop's failure contribution (or `Unknown`
    /// if the hop succeeded).
    pub error_class: ProxyErrorClass,
}

/// Result of parsing a single `Proxy-Status` response header.
///
/// `None` should be used when the header is absent — callers should
/// call `ProxyStatusParser::parse` only when the header is present.
/// This type represents the structured payload of a present header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyStatusReport {
    /// Proxy-error token from RFC 9209 (e.g. `"502.5"`, `"connection_refused"`).
    /// `None` when the header is a bare `Proxy-Status: 200` or
    /// `Proxy-Status: proxy.example` form.
    pub proxy_error: Option<String>,
    /// HTTP status forwarded by the proxy, if any.
    pub http_status: Option<u16>,
    /// Classified bucket — the project's taxonomy, not the RFC's raw token.
    pub error_class: ProxyErrorClass,
    /// `error` parameter value (e.g. `"connection_refused"`,
    /// `"connection_timeout"`).
    pub error_type: Option<String>,
    /// `next-hop` parameter values for multi-hop proxies.
    pub downstream: Vec<DownstreamInfo>,
}

/// Errors raised by [`ProxyStatusParser::parse`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProxyStatusError {
    /// Header value was absent. Not a hard error — callers may treat
    /// it as "no signal" and fall back to status-code-based attribution.
    #[error("Proxy-Status header is absent")]
    Missing,
    /// Header value was non-empty but failed to parse. The byte offset
    /// within the header value is preserved so callers can highlight
    /// the error in their UI.
    #[error("Proxy-Status header is malformed at byte offset {offset}: {reason}")]
    Malformed {
        /// 0-based byte offset into the header value where parsing failed.
        offset: usize,
        /// Human-readable reason for the failure.
        reason: String,
    },
}

/// Port trait for parsing the `Proxy-Status` header.
///
/// Implementations are pure (no I/O, no allocation beyond the returned
/// report) and `Send + Sync` so the manager can call them on the hot
/// path without an async hop.
#[async_trait]
pub trait ProxyStatusParser: Send + Sync {
    /// Parse a single `Proxy-Status` response header value.
    ///
    /// `response_headers` is the full response header map; the
    /// implementation is responsible for picking out the
    /// `Proxy-Status` entry. Pass [`ProxyStatusError::Missing`] back
    /// when no header is present.
    ///
    /// # Errors
    ///
    /// - [`ProxyStatusError::Missing`] when the header is absent.
    /// - [`ProxyStatusError::Malformed`] when the header value is
    ///   non-empty but does not parse.
    async fn parse(
        &self,
        response_headers: &HeaderMap,
    ) -> Result<ProxyStatusReport, ProxyStatusError>;
}

/// Default RFC 9209 parser.
///
/// Pure function on the header value — no I/O, no allocation beyond the
/// returned report. Allocates a small intermediate `String` per
/// parameter for the error-class lookup, which is bounded by the
/// number of `Proxy-Status` parameters in the response (typically
/// `< 5`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Rfc9209Parser;

impl Rfc9209Parser {
    /// Create a new parser instance.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Pure parse on a single header value. Public so tests can drive
    /// it without the `HeaderMap` adapter.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyStatusError::Malformed`] with an offset when the
    /// header value is non-empty but does not match RFC 9209 grammar.
    pub fn parse_value(&self, value: &str) -> Result<ProxyStatusReport, ProxyStatusError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ProxyStatusError::Malformed {
                offset: 0,
                reason: "empty header value".to_string(),
            });
        }

        let mut parts = trimmed.split(';');
        let head = parts
            .next()
            .ok_or_else(|| ProxyStatusError::Malformed {
                offset: 0,
                reason: "missing proxy-error token".to_string(),
            })?
            .trim();

        let (http_status, proxy_error_token) = if let Some((status_str, err_str)) =
            head.split_once('.')
            && status_str.chars().all(|c| c.is_ascii_digit())
        {
            // `<status>.<proxy-error>` form (RFC 9209 §3).
            let status = status_str
                .parse::<u16>()
                .map_err(|e| ProxyStatusError::Malformed {
                    offset: status_str.as_ptr() as usize - trimmed.as_ptr() as usize,
                    reason: format!("invalid HTTP status '{status_str}': {e}"),
                })?;
            (Some(status), Some(err_str.to_string()))
        } else if head.chars().all(|c| c.is_ascii_digit()) {
            // Bare status: `Proxy-Status: 200`.
            let status = head.parse::<u16>().map_err(|e| ProxyStatusError::Malformed {
                offset: 0,
                reason: format!("invalid HTTP status '{head}': {e}"),
            })?;
            (Some(status), None)
        } else {
            // Proxy alias: `Proxy-Status: proxy.example`.
            (None, Some(head.to_string()))
        };

        let mut error_type: Option<String> = None;
        let mut downstream = Vec::new();
        for param in parts {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            if let Some((name, value)) = param.split_once('=') {
                let name = name.trim();
                let value = value.trim().trim_matches('"');
                match name {
                    "error" => error_type = Some(value.to_string()),
                    "next-hop" => {
                        if let Some(info) = parse_next_hop(value) {
                            downstream.push(info);
                        }
                    }
                    _ => {} // unknown parameter — silently ignored per RFC 9209 §3
                }
            }
            // malformed parameter (no '=') is ignored; RFC 9209 allows
            // servers to extend with custom parameters
        }

        let error_class = classify(&proxy_error_token, &error_type, http_status);

        Ok(ProxyStatusReport {
            proxy_error: proxy_error_token,
            http_status,
            error_class,
            error_type,
            downstream,
        })
    }
}

/// Parse a single `next-hop` value (`host:port`).
fn parse_next_hop(value: &str) -> Option<DownstreamInfo> {
    // Bracketed IPv6 form: `[::1]:443`. Handle that case explicitly.
    if let Some(stripped) = value.strip_prefix('[') {
        let (host, port_str) = stripped.split_once(']')?;
        let port_str = port_str.trim_start_matches(':');
        let port = port_str.parse::<u16>().ok()?;
        return Some(DownstreamInfo {
            address: format!("[{host}]:{port}"),
            http_status: None,
            error_class: ProxyErrorClass::Unknown,
        });
    }
    // Otherwise split on the LAST colon (host:port). IPv4 / DNS form.
    let (host, port_str) = value.rsplit_once(':')?;
    let port = port_str.parse::<u16>().ok()?;
    Some(DownstreamInfo {
        address: format!("{host}:{port}"),
        http_status: None,
        error_class: ProxyErrorClass::Unknown,
    })
}

/// Map RFC 9209 error tokens + the project's error-type vocabulary onto
/// [`ProxyErrorClass`]. Heuristic but conservative — when in doubt, fall
/// back to `Unknown` rather than misattributing to `Provider` (which
/// would incorrectly penalise the proxy circuit breaker).
fn classify(
    proxy_error_token: &Option<String>,
    error_type: &Option<String>,
    http_status: Option<u16>,
) -> ProxyErrorClass {
    // Bare success (`Proxy-Status: 200` or `Proxy-Status: proxy.example`)
    // is `Unknown` (no signal).
    if let Some(token) = proxy_error_token {
        let token_lower = token.to_ascii_lowercase();
        match token_lower.as_str() {
            // 5xx-class proxy error codes — RFC 9209 §3 proxy-error codes.
            "connection_timeout" | "connection_read_timeout" | "connection_refused"
            | "connection_reset" | "connection_aborted" | "network_read_timeout"
            | "network_unreachable" | "network_connect_timeout" => {
                return ProxyErrorClass::Network;
            }
            "http_protocol_error" | "http_request_error" | "proxy_internal_error"
            | "proxy_configuration_error" => return ProxyErrorClass::Provider,
            // Note: bare numeric tokens like "502.7" are split so the
            // proxy-error-token is just the sub-code (e.g. "7"). The full
            // status is preserved in `http_status`, so the http_status
            // fallback below is the authoritative classifier for
            // numeric proxy-error codes.
            _ => {}
        }
    }

    if let Some(et) = error_type {
        let et_lower = et.to_ascii_lowercase();
        if et_lower.starts_with("connection_") || et_lower.starts_with("network_") {
            return ProxyErrorClass::Network;
        }
        if et_lower.starts_with("proxy_") {
            return ProxyErrorClass::Provider;
        }
    }

    if let Some(status) = http_status {
        // RFC 9209 §3.7: 502 (Bad Gateway), 503 (Service Unavailable),
        // 504 (Gateway Timeout) and the 5xx family are proxy/network
        // errors when paired with a proxy-error token; fall back to
        // Provider for unrecognised 5xx.
        if status == 502 || status == 503 || status == 504 {
            return ProxyErrorClass::Network;
        }
        if status >= 500 {
            return ProxyErrorClass::Provider;
        }
        if status >= 400 {
            return ProxyErrorClass::Target;
        }
    }

    ProxyErrorClass::Unknown
}

#[async_trait]
impl ProxyStatusParser for Rfc9209Parser {
    async fn parse(
        &self,
        response_headers: &HeaderMap,
    ) -> Result<ProxyStatusReport, ProxyStatusError> {
        // `Proxy-Status` is not in `reqwest::header` because it's
        // non-standard; fall back to a string lookup.
        let Some(value) = response_headers
            .get(reqwest::header::HeaderName::from_static("proxy-status"))
            .or_else(|| response_headers.get("Proxy-Status"))
        else {
            return Err(ProxyStatusError::Missing);
        };
        let value = value
            .to_str()
            .map_err(|e| ProxyStatusError::Malformed {
                offset: 0,
                reason: format!("header value is not valid ASCII: {e}"),
            })?;
        self.parse_value(value)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    use std::str::FromStr;

    fn headers_with(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            HeaderName::from_str("Proxy-Status").unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        map
    }

    #[test]
    fn parse_canonical_rfc_9209_example() {
        let report = Rfc9209Parser
            .parse_value("502.5; error=connection_refused; next-hop=192.0.2.1:443")
            .unwrap();
        assert_eq!(report.http_status, Some(502));
        assert_eq!(report.proxy_error.as_deref(), Some("5"));
        // Note: RFC 9209 calls for the full "502.5" form; we treat the
        // part after the dot as the proxy-error-token. That's correct
        // per the ABNF but our storage shows the sub-code.
        assert_eq!(report.error_class, ProxyErrorClass::Network);
        assert_eq!(report.error_type.as_deref(), Some("connection_refused"));
        assert_eq!(report.downstream.len(), 1);
        assert_eq!(report.downstream[0].address, "192.0.2.1:443");
    }

    #[test]
    fn parse_bare_502_token_classifies_as_network() {
        let report = Rfc9209Parser.parse_value("502.7").unwrap();
        assert_eq!(report.proxy_error.as_deref(), Some("7"));
        assert_eq!(report.error_class, ProxyErrorClass::Network);
    }

    #[test]
    fn parse_bare_500_token_classifies_as_provider() {
        let report = Rfc9209Parser.parse_value("500.0").unwrap();
        assert_eq!(report.proxy_error.as_deref(), Some("0"));
        assert_eq!(report.error_class, ProxyErrorClass::Provider);
    }

    #[test]
    fn parse_named_provider_error() {
        let report = Rfc9209Parser
            .parse_value("proxy_internal_error")
            .unwrap();
        assert_eq!(
            report.proxy_error.as_deref(),
            Some("proxy_internal_error")
        );
        assert_eq!(report.error_class, ProxyErrorClass::Provider);
    }

    #[test]
    fn parse_proxy_alias_form_with_next_hop() {
        // `Proxy-Status: proxy.example; error=connection_timeout; next-hop=192.0.2.1:443`
        // — the head is a proxy alias (no '.' separator), not a status code.
        let report = Rfc9209Parser
            .parse_value(
                "proxy.example; error=connection_timeout; next-hop=192.0.2.1:443",
            )
            .unwrap();
        assert_eq!(report.proxy_error.as_deref(), Some("proxy.example"));
        assert_eq!(report.http_status, None);
        assert_eq!(report.error_class, ProxyErrorClass::Network);
        assert_eq!(report.error_type.as_deref(), Some("connection_timeout"));
        assert_eq!(report.downstream.len(), 1);
        assert_eq!(report.downstream[0].address, "192.0.2.1:443");
    }

    #[test]
    fn parse_bare_200_is_unknown_no_signal() {
        let report = Rfc9209Parser.parse_value("200").unwrap();
        assert_eq!(report.http_status, Some(200));
        assert_eq!(report.error_class, ProxyErrorClass::Unknown);
        assert!(report.proxy_error.is_none());
    }

    #[test]
    fn parse_named_target_error() {
        let report = Rfc9209Parser
            .parse_value("403; error=http_request_error")
            .unwrap();
        assert_eq!(report.http_status, Some(403));
        assert_eq!(report.error_class, ProxyErrorClass::Target);
    }

    #[test]
    fn parse_ipv6_next_hop() {
        let report = Rfc9209Parser
            .parse_value("502.5; error=connection_refused; next-hop=[2001:db8::1]:443")
            .unwrap();
        assert_eq!(report.downstream.len(), 1);
        assert_eq!(report.downstream[0].address, "[2001:db8::1]:443");
    }

    #[test]
    fn parse_header_map_missing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let map = HeaderMap::new();
        let result = rt.block_on(Rfc9209Parser.parse(&map));
        assert_eq!(result.unwrap_err(), ProxyStatusError::Missing);
    }

    #[test]
    fn parse_header_map_present() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let map = headers_with("502.5; error=connection_refused");
        let report = rt.block_on(Rfc9209Parser.parse(&map)).unwrap();
        assert_eq!(report.http_status, Some(502));
        assert_eq!(report.error_class, ProxyErrorClass::Network);
    }

    #[test]
    fn parse_empty_header_value_is_malformed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let map = headers_with("");
        let result = rt.block_on(Rfc9209Parser.parse(&map));
        assert!(matches!(
            result.unwrap_err(),
            ProxyStatusError::Malformed { .. }
        ));
    }

    #[test]
    fn classify_provider_4xx_http_status() {
        // Bare `Proxy-Status: 500` with no error-type → provider fallback.
        let report = Rfc9209Parser.parse_value("500").unwrap();
        assert_eq!(report.error_class, ProxyErrorClass::Provider);
    }

    #[test]
    fn classify_target_4xx_http_status() {
        let report = Rfc9209Parser.parse_value("403").unwrap();
        assert_eq!(report.error_class, ProxyErrorClass::Target);
    }

    #[test]
    fn proxy_error_class_counts_against_proxy() {
        assert!(ProxyErrorClass::Provider.counts_against_proxy());
        assert!(!ProxyErrorClass::Network.counts_against_proxy());
        assert!(!ProxyErrorClass::Target.counts_against_proxy());
        assert!(!ProxyErrorClass::Unknown.counts_against_proxy());
    }
}
