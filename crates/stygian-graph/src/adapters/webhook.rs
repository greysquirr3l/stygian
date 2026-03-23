//! Webhook trigger adapter — axum-based HTTP listener.
//!
//! Implements [`WebhookTrigger`](crate::ports::webhook::WebhookTrigger) with an embedded axum server that accepts
//! inbound webhooks, verifies HMAC-SHA256 signatures, enforces body-size limits,
//! and emits [`WebhookEvent`](crate::ports::webhook::WebhookEvent)s via a channel.
//!
//! Also implements [`ScrapingService`](crate::ports::ScrapingService) so a pipeline node can start a webhook
//! listener and wait for the next event as input.
//!
//! # Feature gate
//!
//! Requires `feature = "api"`.

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::webhook::{WebhookConfig, WebhookEvent, WebhookListenerHandle, WebhookTrigger};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};
use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

// ─── Shared state ─────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    config: WebhookConfig,
    tx: broadcast::Sender<WebhookEvent>,
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// Axum-based webhook trigger adapter.
pub struct AxumWebhookTrigger {
    tx: broadcast::Sender<WebhookEvent>,
    rx: Mutex<broadcast::Receiver<WebhookEvent>>,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl AxumWebhookTrigger {
    /// Create a new [`AxumWebhookTrigger`].
    pub fn new() -> Self {
        let (tx, rx) = broadcast::channel(256);
        Self {
            tx,
            rx: Mutex::new(rx),
            shutdown: Mutex::new(None),
        }
    }

    /// Verify an HMAC-SHA256 signature.
    ///
    /// The `signature` should be in the form `sha256=<hex>`.
    fn verify_hmac(secret: &str, signature: &str, body: &[u8]) -> bool {
        let Some(hex_sig) = signature.strip_prefix("sha256=") else {
            return false;
        };

        let Ok(expected_bytes) = hex_decode(hex_sig) else {
            return false;
        };

        let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
            return false;
        };

        mac.update(body);
        mac.verify_slice(&expected_bytes).is_ok()
    }
}

impl Default for AxumWebhookTrigger {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a hex string to bytes.
fn hex_decode(hex: &str) -> std::result::Result<Vec<u8>, ()> {
    if !hex.len().is_multiple_of(2) {
        return Err(());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

// ─── Routes ───────────────────────────────────────────────────────────────────

async fn trigger_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Enforce body size
    if body.len() > state.config.max_body_size {
        warn!(
            size = body.len(),
            max = state.config.max_body_size,
            "webhook body too large"
        );
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    let body_str = String::from_utf8_lossy(&body).to_string();

    // Extract signature header
    let signature = headers
        .get("x-hub-signature-256")
        .or_else(|| headers.get("x-signature-256"))
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Verify signature if secret is configured
    if let Some(ref secret) = state.config.secret {
        match &signature {
            Some(sig) => {
                if !AxumWebhookTrigger::verify_hmac(secret, sig, &body) {
                    warn!("webhook signature verification failed");
                    return StatusCode::UNAUTHORIZED;
                }
                debug!("webhook signature verified");
            }
            None => {
                warn!("webhook missing signature header, secret is configured");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    // Build filtered headers map
    let filtered_headers: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let key = k.as_str().to_lowercase();
            // Filter to relevant headers
            if key.starts_with("x-")
                || key == "content-type"
                || key == "user-agent"
                || key == "accept"
            {
                v.to_str().ok().map(|val| (key, val.to_string()))
            } else {
                None
            }
        })
        .collect();

    let source_ip = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let received_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let event = WebhookEvent {
        method: "POST".into(),
        path: state.config.path_prefix.clone(),
        headers: filtered_headers,
        body: body_str,
        received_at_ms,
        signature,
        source_ip,
    };

    info!(path = %event.path, "webhook event received");

    if state.tx.send(event).is_err() {
        warn!("no webhook subscribers connected");
    }

    StatusCode::OK
}

async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}

// ─── WebhookTrigger ───────────────────────────────────────────────────────────

#[async_trait]
impl WebhookTrigger for AxumWebhookTrigger {
    async fn start_listener(&self, config: WebhookConfig) -> Result<WebhookListenerHandle> {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let state = AppState {
            config: config.clone(),
            tx: self.tx.clone(),
        };

        let trigger_path = format!("{}/trigger", config.path_prefix);
        let health_path = format!("{}/health", config.path_prefix);

        let app = Router::new()
            .route(&trigger_path, post(trigger_handler))
            .route(&health_path, get(health_handler))
            .with_state(state);

        let listener = TcpListener::bind(&config.bind_address).await.map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "failed to bind webhook listener on {}: {e}",
                config.bind_address
            )))
        })?;

        let handle_id = format!("webhook-{}", config.bind_address);

        info!(bind = %config.bind_address, prefix = %config.path_prefix, "webhook listener started");

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .ok();
        });

        *self.shutdown.lock().await = Some(shutdown_tx);

        Ok(WebhookListenerHandle { id: handle_id })
    }

    async fn stop_listener(&self, handle: WebhookListenerHandle) -> Result<()> {
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(());
            info!(id = %handle.id, "webhook listener stopped");
        }
        Ok(())
    }

    async fn recv_event(&self) -> Result<Option<WebhookEvent>> {
        match self.rx.lock().await.recv().await {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::RecvError::Closed) => Ok(None),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "webhook receiver lagged, events dropped");
                // Try again after lag
                match self.rx.lock().await.recv().await {
                    Ok(event) => Ok(Some(event)),
                    _ => Ok(None),
                }
            }
        }
    }

    fn verify_signature(&self, secret: &str, signature: &str, body: &[u8]) -> bool {
        Self::verify_hmac(secret, signature, body)
    }
}

// ─── ScrapingService ──────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for AxumWebhookTrigger {
    /// Start a webhook listener and wait for the next event.
    ///
    /// The `input.url` is used as the bind address. Params:
    /// - `"path_prefix"`: URL path prefix (default: `"/webhooks"`)
    /// - `"secret"`: Optional HMAC secret
    /// - `"timeout_secs"`: Max seconds to wait for an event (default: 60)
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let path_prefix = input
            .params
            .get("path_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("/webhooks")
            .to_string();

        let secret = input
            .params
            .get("secret")
            .and_then(|v| v.as_str())
            .map(String::from);

        let timeout_secs = input
            .params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let config = WebhookConfig {
            bind_address: input.url.clone(),
            path_prefix,
            secret,
            max_body_size: 1_048_576,
        };

        let handle = self.start_listener(config).await?;

        let event = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.recv_event(),
        )
        .await;

        // Stop listener regardless of outcome
        let _ = self.stop_listener(handle).await;

        match event {
            Ok(Ok(Some(evt))) => Ok(ServiceOutput {
                data: evt.body.clone(),
                metadata: json!({
                    "source": "webhook",
                    "method": evt.method,
                    "path": evt.path,
                    "received_at_ms": evt.received_at_ms,
                    "source_ip": evt.source_ip,
                }),
            }),
            Ok(Ok(None)) => Err(StygianError::Service(ServiceError::Unavailable(
                "webhook listener closed without receiving event".into(),
            ))),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(StygianError::Service(ServiceError::Timeout(
                timeout_secs * 1000,
            ))),
        }
    }

    fn name(&self) -> &'static str {
        "webhook-trigger"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_decode_valid() {
        let result = hex_decode("48656c6c6f").unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn test_hex_decode_empty() {
        let result = hex_decode("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn test_hex_decode_invalid_chars() {
        assert!(hex_decode("zzzz").is_err());
    }

    #[test]
    fn test_verify_hmac_valid() {
        let secret = "test-secret";
        let body = b"test body";

        // Compute expected signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let result = mac.finalize();
        let hex: String = result
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let signature = format!("sha256={hex}");

        assert!(AxumWebhookTrigger::verify_hmac(secret, &signature, body));
    }

    #[test]
    fn test_verify_hmac_invalid_signature() {
        assert!(!AxumWebhookTrigger::verify_hmac(
            "secret",
            "sha256=invalidhex",
            b"body"
        ));
    }

    #[test]
    fn test_verify_hmac_wrong_prefix() {
        assert!(!AxumWebhookTrigger::verify_hmac(
            "secret",
            "md5=abc123",
            b"body"
        ));
    }

    #[test]
    fn test_verify_hmac_wrong_secret() {
        let body = b"test body";
        let mut mac = HmacSha256::new_from_slice(b"correct-secret").unwrap();
        mac.update(body);
        let result = mac.finalize();
        let hex: String = result
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let signature = format!("sha256={hex}");

        assert!(!AxumWebhookTrigger::verify_hmac(
            "wrong-secret",
            &signature,
            body
        ));
    }

    #[test]
    fn test_default_trigger() {
        let trigger = AxumWebhookTrigger::default();
        assert_eq!(trigger.name(), "webhook-trigger");
    }

    #[tokio::test]
    async fn test_start_and_stop_listener() {
        let trigger = AxumWebhookTrigger::new();
        let config = WebhookConfig {
            bind_address: "127.0.0.1:0".into(), // OS-assigned port
            ..Default::default()
        };

        let handle = trigger.start_listener(config).await.unwrap();
        assert!(handle.id.starts_with("webhook-"));

        trigger.stop_listener(handle).await.unwrap();
    }
}
