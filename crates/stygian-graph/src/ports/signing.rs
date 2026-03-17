//! `SigningPort` — request signing abstraction.
//!
//! Implement this trait to attach signatures, HMAC tokens, timestamps, or any
//! other authentication material to outbound requests without coupling the
//! calling adapter to the specific signing scheme.
//!
//! # Common use cases
//!
//! - **Frida RPC bridge**: delegate to a sidecar that calls native `.so`
//!   signing functions inside a real mobile app (Tinder, Snapchat, etc.)
//! - **AWS Signature V4**: sign S3 or API Gateway requests
//! - **OAuth 1.0a**: generate per-request `oauth_signature`
//! - **Custom HMAC**: add `X-Request-Signature` / `X-Signed-At` headers
//! - **Device attestation**: attach Play Integrity / Apple DeviceCheck tokens
//! - **Timestamp + nonce**: anti-replay headers for trading or payment APIs

use std::collections::HashMap;
use std::future::Future;

use async_trait::async_trait;

use crate::domain::error::{ServiceError, StygianError};

// ─────────────────────────────────────────────────────────────────────────────
// Input / Output types
// ─────────────────────────────────────────────────────────────────────────────

/// The request material passed to a [`SigningPort`] for signing.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::signing::SigningInput;
/// use serde_json::json;
///
/// let input = SigningInput {
///     method: "GET".to_string(),
///     url: "https://api.example.com/v2/profile".to_string(),
///     headers: Default::default(),
///     body: None,
///     context: json!({"nonce_seed": 42}),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct SigningInput {
    /// HTTP method of the outbound request (e.g. `"GET"`, `"POST"`)
    pub method: String,
    /// Fully-qualified URL of the outbound request
    pub url: String,
    /// Request headers already present before signing
    pub headers: HashMap<String, String>,
    /// Request body bytes; `None` for bodyless methods
    pub body: Option<Vec<u8>>,
    /// Arbitrary signing context supplied by the caller (nonce seed, session
    /// ID, timestamp override, etc.)
    pub context: serde_json::Value,
}

/// The signing material to merge into the outbound request.
///
/// All fields are additive — they are merged on top of the existing request.
/// A `SigningOutput` with all-default values is a valid no-op.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::signing::SigningOutput;
/// use std::collections::HashMap;
///
/// let mut headers = HashMap::new();
/// headers.insert("Authorization".to_string(), "HMAC-SHA256 sig=abc123".to_string());
/// headers.insert("X-Signed-At".to_string(), "1710676800000".to_string());
///
/// let output = SigningOutput {
///     headers,
///     query_params: vec![],
///     body_override: None,
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct SigningOutput {
    /// Headers to add or override on the outbound request
    pub headers: HashMap<String, String>,
    /// Query parameters to append to the URL
    pub query_params: Vec<(String, String)>,
    /// If `Some`, replace the request body with this value (for signing
    /// schemes that embed a digest into the body)
    pub body_override: Option<Vec<u8>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Error
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by [`SigningPort`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    /// The signing sidecar or backend was unreachable.
    #[error("signing backend unavailable: {0}")]
    BackendUnavailable(String),

    /// The sidecar returned an unexpected or malformed response.
    #[error("signing response invalid: {0}")]
    InvalidResponse(String),

    /// The signing key or secret was absent.
    #[error("signing credentials missing: {0}")]
    CredentialsMissing(String),

    /// The signing request timed out.
    #[error("signing timed out after {0}ms")]
    Timeout(u64),

    /// Any other signing failure.
    #[error("signing failed: {0}")]
    Other(String),
}

impl From<SigningError> for StygianError {
    fn from(e: SigningError) -> Self {
        Self::Service(ServiceError::AuthenticationFailed(e.to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait
// ─────────────────────────────────────────────────────────────────────────────

/// Port for request signing.
///
/// Implement this trait to attach signatures, HMAC tokens, timestamps, or any
/// other authentication material to outbound requests. The calling adapter
/// merges the returned [`SigningOutput`] into the request before sending.
///
/// The trait uses native `async fn` in traits (Rust 2024 edition) so it is
/// *not* object-safe. Use [`ErasedSigningPort`] with `Arc<dyn ...>` when
/// runtime dispatch is required.
///
/// # Example implementation (passthrough — no signing)
///
/// ```rust
/// use stygian_graph::ports::signing::{SigningPort, SigningInput, SigningOutput, SigningError};
///
/// struct NoSigning;
///
/// impl SigningPort for NoSigning {
///     async fn sign(&self, _input: SigningInput) -> Result<SigningOutput, SigningError> {
///         Ok(SigningOutput::default())
///     }
/// }
/// ```
pub trait SigningPort: Send + Sync {
    /// Sign an outbound request, returning the authentication material to merge.
    ///
    /// Implementations must be idempotent — the same `input` must always
    /// produce a valid (if not byte-for-byte identical) `output`.
    ///
    /// # Errors
    ///
    /// - [`SigningError::BackendUnavailable`] — sidecar / key store unreachable
    /// - [`SigningError::InvalidResponse`] — sidecar returned malformed data
    /// - [`SigningError::CredentialsMissing`] — signing key absent at call time
    /// - [`SigningError::Timeout`] — operation exceeded the configured deadline
    fn sign(
        &self,
        input: SigningInput,
    ) -> impl Future<Output = Result<SigningOutput, SigningError>> + Send;
}

// ─────────────────────────────────────────────────────────────────────────────
// ErasedSigningPort — object-safe wrapper for Arc<dyn ...>
// ─────────────────────────────────────────────────────────────────────────────

/// Object-safe version of [`SigningPort`] for runtime dispatch.
///
/// [`SigningPort`] uses native `async fn in trait` (Rust 2024) and is NOT
/// object-safe. `ErasedSigningPort` wraps it via `async_trait`, producing
/// `Pin<Box<dyn Future>>` return types required by `Arc<dyn ...>`.
///
/// A blanket `impl<T: SigningPort> ErasedSigningPort for T` is provided — you
/// never need to implement this trait directly.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use stygian_graph::ports::signing::ErasedSigningPort;
/// use stygian_graph::adapters::signing::NoopSigningAdapter;
///
/// let signer: Arc<dyn ErasedSigningPort> = Arc::new(NoopSigningAdapter);
/// ```
#[async_trait]
pub trait ErasedSigningPort: Send + Sync {
    /// Sign an outbound request, returning the authentication material to merge.
    ///
    /// # Errors
    ///
    /// Returns [`SigningError`] if signing fails for any reason.
    async fn erased_sign(&self, input: SigningInput) -> Result<SigningOutput, SigningError>;
}

#[async_trait]
impl<T: SigningPort> ErasedSigningPort for T {
    async fn erased_sign(&self, input: SigningInput) -> Result<SigningOutput, SigningError> {
        self.sign(input).await
    }
}
