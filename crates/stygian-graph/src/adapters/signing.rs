//! Request signing adapters.
//!
//! Provides concrete [`crate::ports::signing::SigningPort`] implementations:
//!
//! | Adapter | Use case |
//! |---|---|
//! | [`crate::adapters::signing::NoopSigningAdapter`] | Testing / no-op passthrough |
//! | [`crate::adapters::signing::HttpSigningAdapter`] | Delegate to any external signing sidecar over HTTP |
//!
//! # Frida RPC bridge example
//!
//! Run a Frida sidecar that exposes a POST /sign endpoint, then wire it in:
//!
//! ```no_run
//! use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};
//!
//! let signer = HttpSigningAdapter::new(HttpSigningConfig {
//!     endpoint: "http://localhost:27042/sign".to_string(),
//!     ..Default::default()
//! });
//! ```
//!
//! # AWS Signature V4 / custom HMAC
//!
//! Implement [`crate::ports::signing::SigningPort`] directly, or point [`crate::adapters::signing::HttpSigningAdapter`] at a
//! lightweight signing sidecar that handles key material and algorithm details.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::ports::signing::{SigningError, SigningInput, SigningOutput, SigningPort};

#[cfg(test)]
use crate::ports::signing::ErasedSigningPort;

// ─────────────────────────────────────────────────────────────────────────────
// NoopSigningAdapter
// ─────────────────────────────────────────────────────────────────────────────

/// A no-op [`SigningPort`] that passes requests through unsigned.
///
/// Useful as a default when an adapter accepts an optional signer, and as a
/// stand-in during testing.
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::signing::NoopSigningAdapter;
/// use stygian_graph::ports::signing::{SigningPort, SigningInput};
/// use serde_json::json;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let signer = NoopSigningAdapter;
/// let output = signer.sign(SigningInput {
///     method: "GET".to_string(),
///     url: "https://example.com".to_string(),
///     headers: Default::default(),
///     body: None,
///     context: json!({}),
/// }).await.unwrap();
/// assert!(output.headers.is_empty());
/// # });
/// ```
pub struct NoopSigningAdapter;

impl SigningPort for NoopSigningAdapter {
    async fn sign(&self, _input: SigningInput) -> Result<SigningOutput, SigningError> {
        Ok(SigningOutput::default())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HttpSigningAdapter
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`HttpSigningAdapter`].
///
/// # Example
///
/// ```rust
/// use stygian_graph::adapters::signing::HttpSigningConfig;
/// use std::time::Duration;
///
/// let config = HttpSigningConfig {
///     endpoint: "http://localhost:27042/sign".to_string(),
///     timeout: Duration::from_secs(5),
///     bearer_token: Some("my-sidecar-auth-token".to_string()),
///     extra_headers: Default::default(),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct HttpSigningConfig {
    /// Full URL of the signing sidecar endpoint (e.g. `http://localhost:27042/sign`)
    pub endpoint: String,
    /// Request timeout to the signing sidecar (default: 10 seconds)
    pub timeout: Duration,
    /// Optional bearer token to authenticate with the sidecar itself
    pub bearer_token: Option<String>,
    /// Additional static headers to send to the sidecar
    pub extra_headers: HashMap<String, String>,
}

impl Default for HttpSigningConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:27042/sign".to_string(),
            timeout: Duration::from_secs(10),
            bearer_token: None,
            extra_headers: HashMap::new(),
        }
    }
}

/// Wire format for the signing request sent to the sidecar.
#[derive(Debug, Serialize)]
struct SignRequest {
    method: String,
    url: String,
    headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_b64: Option<String>,
    context: serde_json::Value,
}

/// Wire format for the signing response received from the sidecar.
#[derive(Debug, Deserialize)]
struct SignResponse {
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query_params: Vec<(String, String)>,
    #[serde(default)]
    body_b64: Option<String>,
}

/// A [`SigningPort`] that delegates to an external HTTP signing sidecar.
///
/// The sidecar receives a JSON payload describing the outbound request and
/// returns the headers / query params / body override to apply. This pattern
/// works for:
///
/// - **Frida RPC bridges** — a Python/Node sidecar attached to a running mobile
///   app that calls the native `.so` signing function and exposes the result
/// - **AWS Signature V4** — a lightweight server that knows your AWS credentials
/// - **OAuth 1.0a** — sign Twitter/X API v1 requests via a sidecar that holds
///   the consumer secret
/// - **Any custom HMAC scheme** — keep key material out of the main process
///
/// # Example
///
/// ```no_run
/// use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};
/// use stygian_graph::ports::signing::{SigningPort, SigningInput};
/// use serde_json::json;
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let signer = HttpSigningAdapter::new(HttpSigningConfig {
///     endpoint: "http://localhost:27042/sign".to_string(),
///     ..Default::default()
/// });
///
/// let output = signer.sign(SigningInput {
///     method: "GET".to_string(),
///     url: "https://api.tinder.com/v2/profile".to_string(),
///     headers: Default::default(),
///     body: None,
///     context: json!({}),
/// }).await.unwrap();
///
/// for (k, v) in &output.headers {
///     println!("{k}: {v}");
/// }
/// # });
/// ```
pub struct HttpSigningAdapter {
    config: HttpSigningConfig,
    client: Client,
}

impl HttpSigningAdapter {
    /// Create a new `HttpSigningAdapter` with the given configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use stygian_graph::adapters::signing::{HttpSigningAdapter, HttpSigningConfig};
    ///
    /// let signer = HttpSigningAdapter::new(HttpSigningConfig::default());
    /// ```
    pub fn new(config: HttpSigningConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_default();
        Self { config, client }
    }
}

impl SigningPort for HttpSigningAdapter {
    async fn sign(&self, input: SigningInput) -> Result<SigningOutput, SigningError> {
        let body_b64 = input.body.as_deref().map(base64_encode);

        let req_body = SignRequest {
            method: input.method,
            url: input.url,
            headers: input.headers,
            body_b64,
            context: input.context,
        };

        let mut req = self.client.post(&self.config.endpoint).json(&req_body);

        if let Some(token) = &self.config.bearer_token {
            req = req.bearer_auth(token);
        }
        for (k, v) in &self.config.extra_headers {
            req = req.header(k, v);
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                SigningError::Timeout(
                    self.config
                        .timeout
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                )
            } else {
                SigningError::BackendUnavailable(e.to_string())
            }
        })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(SigningError::InvalidResponse(format!(
                "sidecar returned HTTP {status}: {body}"
            )));
        }

        let sign_resp: SignResponse = response
            .json()
            .await
            .map_err(|e| SigningError::InvalidResponse(e.to_string()))?;

        let body_override = sign_resp
            .body_b64
            .map(|b64| base64_decode(&b64))
            .transpose()
            .map_err(|e| SigningError::InvalidResponse(format!("base64 decode failed: {e}")))?;

        Ok(SigningOutput {
            headers: sign_resp.headers,
            query_params: sign_resp.query_params,
            body_override,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Base64 helpers (std-only, no extra deps)
// ─────────────────────────────────────────────────────────────────────────────

fn base64_encode(input: &[u8]) -> String {
    use std::fmt::Write;
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 {
            chunk[1] as usize
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            chunk[2] as usize
        } else {
            0
        };
        let _ = write!(out, "{}", TABLE[b0 >> 2] as char);
        let _ = write!(out, "{}", TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            let _ = write!(out, "{}", TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            let _ = write!(out, "{}", TABLE[b2 & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len() * 3 / 4 + 1);
    let decode_char = |c: u8| -> Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 char: {c}")),
        }
    };
    let bytes = input.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let v0 = decode_char(bytes[i])?;
        let v1 = decode_char(bytes[i + 1])?;
        out.push((v0 << 2) | (v1 >> 4));
        if i + 2 < bytes.len() {
            let v2 = decode_char(bytes[i + 2])?;
            out.push(((v1 & 0xf) << 4) | (v2 >> 2));
            if i + 3 < bytes.len() {
                let v3 = decode_char(bytes[i + 3])?;
                out.push(((v2 & 3) << 6) | v3);
            }
        }
        i += 4;
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn noop_returns_empty_output() {
        let signer = NoopSigningAdapter;
        let output = signer
            .sign(SigningInput {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: HashMap::new(),
                body: None,
                context: json!({}),
            })
            .await
            .unwrap();
        assert!(output.headers.is_empty());
        assert!(output.query_params.is_empty());
        assert!(output.body_override.is_none());
    }

    #[tokio::test]
    async fn noop_is_erased_signing_port() {
        let signer: std::sync::Arc<dyn ErasedSigningPort> = std::sync::Arc::new(NoopSigningAdapter);
        let output = signer
            .erased_sign(SigningInput {
                method: "POST".to_string(),
                url: "https://api.example.com/data".to_string(),
                headers: HashMap::new(),
                body: Some(b"{\"key\":\"val\"}".to_vec()),
                context: json!({"session": "abc"}),
            })
            .await
            .unwrap();
        assert!(output.headers.is_empty());
    }

    #[test]
    fn base64_roundtrip() {
        let input = b"Hello, Stygian signing!";
        let encoded = base64_encode(input);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn base64_encode_known_value() {
        // RFC 4648 test vector
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
    }
}
