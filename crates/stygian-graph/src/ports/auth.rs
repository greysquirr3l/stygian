//! `AuthPort` — runtime token loading, expiry checking, and refresh.
//!
//! Implement this trait to inject live credentials into pipeline execution
//! without pre-loading a static token. Designed to integrate with
//! `stygian-browser`'s OAuth2 PKCE token store.

use std::future::Future;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::domain::error::{ServiceError, StygianError};

// ─────────────────────────────────────────────────────────────────────────────
// Token
// ─────────────────────────────────────────────────────────────────────────────

/// A resolved `OAuth2` / API bearer token with optional expiry metadata.
///
/// `TokenSet` deliberately does **not** implement `Display` — only `Debug` —
/// to prevent accidental log or format-string leakage of access tokens.
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::auth::TokenSet;
/// use std::time::{SystemTime, Duration};
///
/// let ts = TokenSet {
///     access_token: "tok_abc123".to_string(),
///     refresh_token: Some("ref_xyz".to_string()),
///     expires_at: SystemTime::now().checked_add(Duration::from_secs(3600)),
///     scopes: vec!["read:user".to_string()],
/// };
/// assert!(!ts.is_expired());
/// ```
#[derive(Debug, Clone)]
pub struct TokenSet {
    /// Bearer token to inject into requests
    pub access_token: String,
    /// Refresh token (may be absent for non-OAuth2 API keys)
    pub refresh_token: Option<String>,
    /// Absolute expiry time; `None` means the token does not expire
    pub expires_at: Option<SystemTime>,
    /// `OAuth2` scopes granted to this token
    pub scopes: Vec<String>,
}

impl TokenSet {
    /// Returns `true` if the token has expired (with a 60-second safety margin).
    ///
    /// A token without an `expires_at` is considered perpetually valid.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::ports::auth::TokenSet;
    /// use std::time::{SystemTime, Duration};
    ///
    /// let expired = TokenSet {
    ///     access_token: "tok".to_string(),
    ///     refresh_token: None,
    ///     expires_at: SystemTime::now().checked_sub(Duration::from_secs(300)),
    ///     scopes: vec![],
    /// };
    /// assert!(expired.is_expired());
    /// ```
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let Some(exp) = self.expires_at else {
            return false;
        };
        let threshold = SystemTime::now()
            .checked_add(Duration::from_mins(1))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        exp <= threshold
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by [`AuthPort`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No token is stored; the user must complete the auth flow first.
    #[error("no token found — please run the auth flow")]
    TokenNotFound,

    /// The stored token has expired and could not be refreshed.
    #[error("token expired")]
    TokenExpired,

    /// The refresh request failed.
    #[error("token refresh failed: {0}")]
    RefreshFailed(String),

    /// The token store could not be read or written.
    #[error("token storage failed: {0}")]
    StorageFailed(String),

    /// The PKCE / interactive auth flow failed.
    #[error("auth flow failed: {0}")]
    AuthFlowFailed(String),

    /// The token was present but malformed.
    #[error("invalid token: {0}")]
    InvalidToken(String),
}

impl From<AuthError> for StygianError {
    fn from(e: AuthError) -> Self {
        Self::Service(ServiceError::AuthenticationFailed(e.to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait
// ─────────────────────────────────────────────────────────────────────────────

/// Port for runtime credential management.
///
/// Implement this trait to supply live tokens to pipeline execution.
/// `stygian-browser`'s encrypted disk token store is the primary reference
/// implementation, but in-memory and environment-variable backed variants
/// are also common.
///
/// The trait uses native `async fn` in traits (Rust 2024 edition) so it is
/// *not* object-safe. Use `Arc<impl AuthPort>` or generics rather than
/// `Arc<dyn AuthPort>`.
///
/// # Example implementation (in-memory)
///
/// ```rust
/// use stygian_graph::ports::auth::{AuthPort, AuthError, TokenSet};
///
/// struct StaticTokenAuth { token: String }
///
/// impl AuthPort for StaticTokenAuth {
///     async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
///         Ok(Some(TokenSet {
///             access_token: self.token.clone(),
///             refresh_token: None,
///             expires_at: None,
///             scopes: vec![],
///         }))
///     }
///     async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> {
///         Err(AuthError::TokenNotFound)
///     }
/// }
/// ```
pub trait AuthPort: Send + Sync {
    /// Load the current token from the backing store.
    ///
    /// Returns `Ok(None)` if no token has been stored yet.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::StorageFailed`] if the backing store is unavailable.
    fn load_token(
        &self,
    ) -> impl Future<Output = std::result::Result<Option<TokenSet>, AuthError>> + Send;

    /// Obtain a fresh token by exchanging the stored refresh token with the
    /// authorization server, then persist it.
    ///
    /// Implementations should persist the refreshed token before returning so
    /// that concurrent callers get a consistent view.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::RefreshFailed`] when the token endpoint rejects the
    /// request, or [`AuthError::TokenNotFound`] when no refresh token is
    /// available.
    fn refresh_token(
        &self,
    ) -> impl Future<Output = std::result::Result<TokenSet, AuthError>> + Send;
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a live access-token string from an `AuthPort`.
///
/// 1. Calls `load_token()`.
/// 2. If the token is expired, calls `refresh_token()`.
/// 3. Returns the raw access token string, ready to be injected into a request.
///
/// # Errors
///
/// Returns `Err` if no token exists, storage is unavailable, or refresh fails.
///
/// # Example
///
/// ```rust
/// # use stygian_graph::ports::auth::{AuthPort, AuthError, TokenSet, resolve_token};
/// # struct Env;
/// # impl AuthPort for Env {
/// #   async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
/// #     Ok(Some(TokenSet { access_token: "abc".to_string(), refresh_token: None, expires_at: None, scopes: vec![] }))
/// #   }
/// #   async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> { Err(AuthError::TokenNotFound) }
/// # }
/// # async fn run() -> std::result::Result<String, AuthError> {
/// let auth = Env;
/// let token = resolve_token(&auth).await?;
/// println!("Bearer {token}");
/// # Ok(token)
/// # }
/// ```
pub async fn resolve_token(port: &impl AuthPort) -> std::result::Result<String, AuthError> {
    let ts = port.load_token().await?.ok_or(AuthError::TokenNotFound)?;

    if ts.is_expired() {
        let refreshed = port.refresh_token().await?;
        return Ok(refreshed.access_token);
    }

    Ok(ts.access_token)
}

// ─────────────────────────────────────────────────────────────────────────────
// ErasedAuthPort — object-safe wrapper for use with Arc<dyn ...>
// ─────────────────────────────────────────────────────────────────────────────

/// Object-safe version of [`AuthPort`] for runtime dispatch.
///
/// [`AuthPort`] uses native `async fn in trait` (Rust 2024) and is NOT
/// object-safe.  `ErasedAuthPort` wraps the same logic via `async_trait`,
/// producing `Pin<Box<dyn Future>>` return types that `Arc<dyn ...>` requires.
///
/// A blanket `impl<T: AuthPort> ErasedAuthPort for T` is provided — you never
/// need to implement this trait directly.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use stygian_graph::ports::auth::{ErasedAuthPort, EnvAuthPort};
///
/// let port: Arc<dyn ErasedAuthPort> = Arc::new(EnvAuthPort::new("GITHUB_TOKEN"));
/// // Pass `port` to GraphQlService::with_auth_port(port)
/// ```
#[async_trait]
pub trait ErasedAuthPort: Send + Sync {
    /// Resolve a live access-token string — load, check expiry, refresh if needed.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no token exists, storage is unavailable, or refresh fails.
    async fn erased_resolve_token(&self) -> std::result::Result<String, AuthError>;
}

#[async_trait]
impl<T: AuthPort> ErasedAuthPort for T {
    async fn erased_resolve_token(&self) -> std::result::Result<String, AuthError> {
        resolve_token(self).await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EnvAuthPort — convenience impl backed by an environment variable
// ─────────────────────────────────────────────────────────────────────────────

/// An [`AuthPort`] that reads a static token from an environment variable.
///
/// Tokens from environment variables never expire; `refresh_token` always
/// returns [`AuthError::TokenNotFound`].
///
/// # Example
///
/// ```rust
/// use stygian_graph::ports::auth::EnvAuthPort;
///
/// let auth = EnvAuthPort::new("GITHUB_TOKEN");
/// // At pipeline execution time, `load_token()` will read $GITHUB_TOKEN.
/// ```
pub struct EnvAuthPort {
    var_name: String,
}

impl EnvAuthPort {
    /// Create an `EnvAuthPort` that will read `var_name` from the environment
    /// at token-load time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use stygian_graph::ports::auth::EnvAuthPort;
    ///
    /// let auth = EnvAuthPort::new("GITHUB_TOKEN");
    /// ```
    #[must_use]
    pub fn new(var_name: impl Into<String>) -> Self {
        Self {
            var_name: var_name.into(),
        }
    }
}

impl AuthPort for EnvAuthPort {
    async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
        match std::env::var(&self.var_name) {
            Ok(token) if !token.is_empty() => Ok(Some(TokenSet {
                access_token: token,
                refresh_token: None,
                expires_at: None,
                scopes: vec![],
            })),
            Ok(_) | Err(_) => Ok(None),
        }
    }

    async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> {
        // Static env-var tokens don't support refresh.
        Err(AuthError::TokenNotFound)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, unsafe_code)] // env::set_var / remove_var are unsafe in Rust ≥1.79
    use super::*;

    struct FixedToken(String);

    impl AuthPort for FixedToken {
        async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
            Ok(Some(TokenSet {
                access_token: self.0.clone(),
                refresh_token: None,
                expires_at: None,
                scopes: vec![],
            }))
        }

        async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> {
            Err(AuthError::RefreshFailed("no refresh token".to_string()))
        }
    }

    struct NoToken;

    impl AuthPort for NoToken {
        async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
            Ok(None)
        }

        async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> {
            Err(AuthError::TokenNotFound)
        }
    }

    struct ExpiredToken {
        new_token: String,
    }

    impl AuthPort for ExpiredToken {
        async fn load_token(&self) -> std::result::Result<Option<TokenSet>, AuthError> {
            Ok(Some(TokenSet {
                access_token: "old_token".to_string(),
                refresh_token: Some("ref".to_string()),
                expires_at: SystemTime::now().checked_sub(Duration::from_hours(1)),
                scopes: vec![],
            }))
        }

        async fn refresh_token(&self) -> std::result::Result<TokenSet, AuthError> {
            Ok(TokenSet {
                access_token: self.new_token.clone(),
                refresh_token: None,
                expires_at: None,
                scopes: vec![],
            })
        }
    }

    #[test]
    fn not_expired_when_no_expiry() {
        let ts = TokenSet {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
        };
        assert!(!ts.is_expired());
    }

    #[test]
    fn expired_when_past_expiry() {
        let ts = TokenSet {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: SystemTime::now().checked_sub(Duration::from_mins(5)),
            scopes: vec![],
        };
        assert!(ts.is_expired());
    }

    #[test]
    fn not_expired_within_60s_margin() {
        // Expires in 30s — within the 60s safety margin, so treated as expired.
        let ts = TokenSet {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: SystemTime::now().checked_add(Duration::from_secs(30)),
            scopes: vec![],
        };
        assert!(ts.is_expired());
    }

    #[test]
    fn not_expired_outside_60s_margin() {
        let ts = TokenSet {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: SystemTime::now().checked_add(Duration::from_mins(2)),
            scopes: vec![],
        };
        assert!(!ts.is_expired());
    }

    #[tokio::test]
    async fn resolve_token_returns_access_token() {
        let auth = FixedToken("tok_abc".to_string());
        let token = resolve_token(&auth).await.unwrap();
        assert_eq!(token, "tok_abc");
    }

    #[tokio::test]
    async fn resolve_token_returns_err_when_no_token() {
        let auth = NoToken;
        assert!(resolve_token(&auth).await.is_err());
    }

    #[tokio::test]
    async fn resolve_token_refreshes_when_expired() {
        let auth = ExpiredToken {
            new_token: "fresh_tok".to_string(),
        };
        let token = resolve_token(&auth).await.unwrap();
        assert_eq!(token, "fresh_tok");
    }

    #[tokio::test]
    async fn env_auth_port_loads_from_env() {
        // Safety: test-only env mutation under #[tokio::test]
        unsafe { std::env::set_var("_STYGIAN_TEST_TOKEN_1", "env_tok_xyz") };
        let auth = EnvAuthPort::new("_STYGIAN_TEST_TOKEN_1");
        let token = resolve_token(&auth).await.unwrap();
        assert_eq!(token, "env_tok_xyz");
        unsafe { std::env::remove_var("_STYGIAN_TEST_TOKEN_1") };
    }

    #[tokio::test]
    async fn env_auth_port_returns_none_when_unset() {
        let auth = EnvAuthPort::new("_STYGIAN_TEST_MISSING_VAR_9999");
        let ts = auth.load_token().await.unwrap();
        assert!(ts.is_none());
    }
}
