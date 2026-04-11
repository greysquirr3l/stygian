//! Webhook trigger port — accept inbound HTTP requests that start pipelines.
//!
//! Defines the [`WebhookTrigger`](crate::ports::webhook::WebhookTrigger) trait and associated types.  The port contains
//! **zero** infrastructure dependencies: adapters (e.g. axum, actix) implement
//! the trait with real HTTP servers.
//!
//! # Architecture
//!
//! ```text
//! External service ──POST──▶ WebhookTrigger adapter
//!                                │
//!                                ▼
//!                          WebhookEvent
//!                                │
//!                      Application layer decides
//!                      which pipeline to execute
//! ```

use crate::domain::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─────────────────────────────────────────────────────────────────────────────
// Domain types
// ─────────────────────────────────────────────────────────────────────────────

/// An inbound webhook event received by the trigger listener.
///
/// Contains enough context for the application layer to decide which pipeline
/// to execute and with what input.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::webhook::WebhookEvent;
///
/// let event = WebhookEvent {
///     method: "POST".into(),
///     path: "/hooks/github".into(),
///     headers: [("content-type".into(), "application/json".into())].into(),
///     body: r#"{"action":"push"}"#.into(),
///     received_at_ms: 1_700_000_000_000,
///     signature: Some("sha256=abc123".into()),
///     source_ip: Some("203.0.113.1".into()),
/// };
/// assert_eq!(event.method, "POST");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    /// HTTP method (e.g. `POST`, `PUT`).
    pub method: String,
    /// Request path (e.g. `/hooks/github`).
    pub path: String,
    /// Filtered HTTP headers (lowercase keys).
    pub headers: HashMap<String, String>,
    /// Request body as a UTF-8 string.
    pub body: String,
    /// Unix timestamp (milliseconds) when the event was received.
    pub received_at_ms: u64,
    /// Optional webhook signature header value (e.g. `sha256=...`).
    pub signature: Option<String>,
    /// Optional source IP address.
    pub source_ip: Option<String>,
}

/// Configuration for a webhook trigger listener.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::webhook::WebhookConfig;
///
/// let config = WebhookConfig {
///     bind_address: "0.0.0.0:9090".into(),
///     path_prefix: "/webhooks".into(),
///     secret: Some("my-hmac-secret".into()),
///     max_body_size: 1_048_576,
/// };
/// assert_eq!(config.max_body_size, 1_048_576);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Socket address to bind the listener to (e.g. `"0.0.0.0:9090"`).
    pub bind_address: String,
    /// URL path prefix for webhook routes (e.g. `"/webhooks"`).
    pub path_prefix: String,
    /// Optional shared secret for HMAC-SHA256 signature verification.
    /// When set, requests without a valid signature are rejected.
    pub secret: Option<String>,
    /// Maximum request body size in bytes (default 1 MiB).
    pub max_body_size: usize,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:9090".into(),
            path_prefix: "/webhooks".into(),
            secret: None,
            max_body_size: 1_048_576, // 1 MiB
        }
    }
}

/// Handle returned by [`WebhookTrigger::start_listener`] for managing the
/// listener lifecycle.
///
/// Dropping the handle does **not** stop the listener — call
/// [`WebhookTrigger::stop_listener`] explicitly for graceful shutdown.
pub struct WebhookListenerHandle {
    /// Opaque identifier for the running listener.
    pub id: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Port trait
// ─────────────────────────────────────────────────────────────────────────────

/// Port: accept inbound webhooks and emit [`WebhookEvent`]s.
///
/// Implementations bind an HTTP listener, verify signatures, enforce body-size
/// limits, and forward valid events to registered callbacks.  The application
/// layer maps events to pipeline executions.
///
/// All methods are `async` and implementations must be `Send + Sync + 'static`.
#[async_trait]
pub trait WebhookTrigger: Send + Sync + 'static {
    /// Start the HTTP listener with the given configuration.
    ///
    /// Returns a [`WebhookListenerHandle`] that can be passed to
    /// [`stop_listener`](Self::stop_listener) for graceful shutdown.
    async fn start_listener(&self, config: WebhookConfig) -> Result<WebhookListenerHandle>;

    /// Gracefully stop the listener identified by `handle`.
    ///
    /// In-flight requests should be drained before the listener shuts down.
    async fn stop_listener(&self, handle: WebhookListenerHandle) -> Result<()>;

    /// Wait for the next webhook event.
    ///
    /// Blocks until an event is received or the listener is stopped (returns
    /// `Ok(None)` in the latter case).
    async fn recv_event(&self) -> Result<Option<WebhookEvent>>;

    /// Verify the HMAC-SHA256 signature for a webhook payload.
    ///
    /// Returns `true` if the signature is valid, `false` if it is invalid,
    /// and `Ok(true)` if no secret is configured (verification is skipped).
    ///
    /// # Arguments
    ///
    /// * `secret` — The shared HMAC secret.
    /// * `signature` — The signature header value (e.g. `sha256=<hex>`).
    /// * `body` — The raw request body bytes.
    fn verify_signature(&self, secret: &str, signature: &str, body: &[u8]) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_event_creation() {
        let event = WebhookEvent {
            method: "POST".into(),
            path: "/hooks/github".into(),
            headers: HashMap::new(),
            body: r#"{"ref":"refs/heads/main"}"#.into(),
            received_at_ms: 1_700_000_000_000,
            signature: Some("sha256=abc".into()),
            source_ip: None,
        };
        assert_eq!(event.method, "POST");
        assert_eq!(event.path, "/hooks/github");
        assert!(event.signature.is_some());
    }

    #[test]
    fn test_webhook_config_default() {
        let cfg = WebhookConfig::default();
        assert_eq!(cfg.bind_address, "0.0.0.0:9090");
        assert_eq!(cfg.path_prefix, "/webhooks");
        assert!(cfg.secret.is_none());
        assert_eq!(cfg.max_body_size, 1_048_576);
    }

    #[test]
    fn test_webhook_event_serialisation() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let event = WebhookEvent {
            method: "POST".into(),
            path: "/trigger".into(),
            headers: [("x-hub-signature-256".into(), "sha256=abc".into())].into(),
            body: "{}".into(),
            received_at_ms: 0,
            signature: None,
            source_ip: Some("127.0.0.1".into()),
        };
        let json = serde_json::to_string(&event)?;
        let back: WebhookEvent = serde_json::from_str(&json)?;
        assert_eq!(back.method, "POST");
        assert_eq!(back.source_ip.as_deref(), Some("127.0.0.1"));
        Ok(())
    }

    #[test]
    fn test_webhook_config_with_secret() {
        let cfg = WebhookConfig {
            secret: Some("my-secret".into()),
            ..Default::default()
        };
        assert_eq!(cfg.secret.as_deref(), Some("my-secret"));
    }
}
