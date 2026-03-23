//! WebSocket stream source adapter.
//!
//! Implements [`StreamSourcePort`] and [`ScrapingService`] for consuming
//! WebSocket feeds.  Uses `tokio-tungstenite` for the underlying connection.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::websocket::WebSocketSource;
//! use stygian_graph::ports::stream_source::StreamSourcePort;
//!
//! # async fn example() {
//! let source = WebSocketSource::default();
//! let events = source.subscribe("wss://api.example.com/ws", Some(10)).await.unwrap();
//! println!("received {} events", events.len());
//! # }
//! ```

use async_trait::async_trait;
use futures::stream::StreamExt;
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::stream_source::{StreamEvent, StreamSourcePort};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─── Configuration ────────────────────────────────────────────────────────────

/// Configuration for a WebSocket connection.
///
/// # Example
///
/// ```
/// use stygian_graph::adapters::websocket::WebSocketConfig;
///
/// let config = WebSocketConfig {
///     subscribe_message: Some(r#"{"type":"subscribe","channel":"prices"}"#.into()),
///     bearer_token: None,
///     timeout_secs: 30,
///     max_reconnect_attempts: 3,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// Optional message to send immediately after connecting (e.g. subscribe).
    pub subscribe_message: Option<String>,
    /// Optional Bearer token for Authorization header on the upgrade request.
    pub bearer_token: Option<String>,
    /// Connection timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum reconnection attempts on connection drop.
    pub max_reconnect_attempts: u32,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            subscribe_message: None,
            bearer_token: None,
            timeout_secs: 30,
            max_reconnect_attempts: 3,
        }
    }
}

// ─── Adapter ──────────────────────────────────────────────────────────────────

/// WebSocket stream source adapter.
///
/// Connects to a WebSocket endpoint and collects messages until `max_events`
/// is reached, the stream closes, or a connection timeout occurs.
#[derive(Default)]
pub struct WebSocketSource {
    config: WebSocketConfig,
}

impl WebSocketSource {
    /// Create a new WebSocket source with custom configuration.
    pub fn new(config: WebSocketConfig) -> Self {
        Self { config }
    }

    /// Extract configuration from `ServiceInput.params` overrides.
    fn config_from_params(&self, params: &serde_json::Value) -> WebSocketConfig {
        let mut cfg = self.config.clone();
        if let Some(msg) = params.get("subscribe_message").and_then(|v| v.as_str()) {
            cfg.subscribe_message = Some(msg.to_string());
        }
        if let Some(token) = params.get("bearer_token").and_then(|v| v.as_str()) {
            cfg.bearer_token = Some(token.to_string());
        }
        if let Some(t) = params.get("timeout_secs").and_then(|v| v.as_u64()) {
            cfg.timeout_secs = t;
        }
        if let Some(r) = params
            .get("max_reconnect_attempts")
            .and_then(|v| v.as_u64())
        {
            cfg.max_reconnect_attempts = r as u32;
        }
        cfg
    }

    /// Connect and collect events from a WebSocket endpoint.
    async fn collect_events(
        &self,
        url: &str,
        max_events: Option<usize>,
        cfg: &WebSocketConfig,
    ) -> Result<Vec<StreamEvent>> {
        let mut request = url.into_client_request().map_err(|e| {
            StygianError::Service(ServiceError::Unavailable(format!(
                "invalid WebSocket URL: {e}"
            )))
        })?;

        // Inject auth header if configured
        if let Some(token) = &cfg.bearer_token {
            request.headers_mut().insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {token}").parse().map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "invalid auth header: {e}"
                    )))
                })?,
            );
        }

        let connect_timeout = Duration::from_secs(cfg.timeout_secs);
        let (ws_stream, _) = timeout(connect_timeout, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| {
                StygianError::Service(ServiceError::Unavailable(
                    "WebSocket connection timed out".into(),
                ))
            })?
            .map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "WebSocket connection failed: {e}"
                )))
            })?;

        let (mut write, mut read) = ws_stream.split();

        // Send subscribe message if configured
        if let Some(ref sub_msg) = cfg.subscribe_message {
            use futures::SinkExt;
            write
                .send(Message::Text(sub_msg.clone().into()))
                .await
                .map_err(|e| {
                    StygianError::Service(ServiceError::Unavailable(format!(
                        "failed to send subscribe message: {e}"
                    )))
                })?;
        }

        let mut events = Vec::new();
        let mut frame_idx: u64 = 0;

        while let Some(msg_result) = timeout(Duration::from_secs(cfg.timeout_secs), read.next())
            .await
            .ok()
            .flatten()
        {
            match msg_result {
                Ok(msg) => {
                    if let Some(event) = map_message_to_event(msg, frame_idx) {
                        events.push(event);
                        frame_idx += 1;

                        if let Some(max) = max_events
                            && events.len() >= max
                        {
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("WebSocket receive error: {e}");
                    break;
                }
            }
        }

        Ok(events)
    }
}

/// Map a WebSocket message to a [`StreamEvent`].
///
/// Returns `None` for internal frames (Pong, Close, Frame).
fn map_message_to_event(msg: Message, frame_idx: u64) -> Option<StreamEvent> {
    match msg {
        Message::Text(text) => Some(StreamEvent {
            id: Some(frame_idx.to_string()),
            event_type: Some("text".into()),
            data: text.to_string(),
        }),
        Message::Binary(data) => {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
            Some(StreamEvent {
                id: Some(frame_idx.to_string()),
                event_type: Some("binary".into()),
                data: encoded,
            })
        }
        Message::Ping(data) => Some(StreamEvent {
            id: Some(frame_idx.to_string()),
            event_type: Some("ping".into()),
            data: String::from_utf8_lossy(&data).to_string(),
        }),
        // Pong, Close, and Frame are internal — skip
        Message::Pong(_) | Message::Close(_) | Message::Frame(_) => None,
    }
}

// ─── StreamSourcePort ─────────────────────────────────────────────────────────

#[async_trait]
impl StreamSourcePort for WebSocketSource {
    async fn subscribe(&self, url: &str, max_events: Option<usize>) -> Result<Vec<StreamEvent>> {
        let cfg = self.config.clone();
        let mut last_err = None;

        for attempt in 0..=cfg.max_reconnect_attempts {
            match self.collect_events(url, max_events, &cfg).await {
                Ok(events) => return Ok(events),
                Err(e) => {
                    tracing::warn!(
                        "WebSocket attempt {}/{} failed: {e}",
                        attempt + 1,
                        cfg.max_reconnect_attempts + 1
                    );
                    last_err = Some(e);

                    if attempt < cfg.max_reconnect_attempts {
                        // Exponential backoff: 1s, 2s, 4s ...
                        let backoff = Duration::from_secs(1 << attempt);
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            StygianError::Service(ServiceError::Unavailable(
                "WebSocket connection failed after all retries".into(),
            ))
        }))
    }

    fn source_name(&self) -> &str {
        "websocket"
    }
}

// ─── ScrapingService ──────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for WebSocketSource {
    /// Collect messages from a WebSocket and return as JSON array.
    ///
    /// # Params (optional)
    ///
    /// * `max_events` — integer; maximum messages to collect.
    /// * `subscribe_message` — string; message to send on connect.
    /// * `bearer_token` — string; Bearer token for auth header.
    /// * `timeout_secs` — integer; connection/read timeout.
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let cfg = self.config_from_params(&input.params);
        let max_events = input
            .params
            .get("max_events")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let events = self.collect_events(&input.url, max_events, &cfg).await?;
        let count = events.len();

        let data = serde_json::to_string(&events).map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "websocket serialization failed: {e}"
            )))
        })?;

        Ok(ServiceOutput {
            data,
            metadata: json!({
                "source": "websocket",
                "event_count": count,
                "source_url": input.url,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "websocket"
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_text_frame() {
        let msg = Message::Text(r#"{"price": 42.5}"#.into());
        let event = map_message_to_event(msg, 0).expect("should map");
        assert_eq!(event.id.as_deref(), Some("0"));
        assert_eq!(event.event_type.as_deref(), Some("text"));
        assert_eq!(event.data, r#"{"price": 42.5}"#);
    }

    #[test]
    fn map_binary_frame_to_base64() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let msg = Message::Binary(data.into());
        let event = map_message_to_event(msg, 1).expect("should map");
        assert_eq!(event.event_type.as_deref(), Some("binary"));
        // Verify it's valid base64
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&event.data)
            .expect("valid base64");
        assert_eq!(decoded, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn map_ping_frame() {
        let msg = Message::Ping(vec![1, 2, 3].into());
        let event = map_message_to_event(msg, 2).expect("should map");
        assert_eq!(event.event_type.as_deref(), Some("ping"));
    }

    #[test]
    fn pong_frame_is_skipped() {
        let msg = Message::Pong(vec![].into());
        assert!(map_message_to_event(msg, 0).is_none());
    }

    #[test]
    fn close_frame_is_skipped() {
        let msg = Message::Close(None);
        assert!(map_message_to_event(msg, 0).is_none());
    }

    #[test]
    fn default_config() {
        let cfg = WebSocketConfig::default();
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_reconnect_attempts, 3);
        assert!(cfg.subscribe_message.is_none());
        assert!(cfg.bearer_token.is_none());
    }

    #[test]
    fn config_from_params_overrides() {
        let source = WebSocketSource::default();
        let params = json!({
            "subscribe_message": "{\"action\":\"sub\"}",
            "bearer_token": "tok123",
            "timeout_secs": 60,
            "max_reconnect_attempts": 5
        });
        let cfg = source.config_from_params(&params);
        assert_eq!(
            cfg.subscribe_message.as_deref(),
            Some("{\"action\":\"sub\"}")
        );
        assert_eq!(cfg.bearer_token.as_deref(), Some("tok123"));
        assert_eq!(cfg.timeout_secs, 60);
        assert_eq!(cfg.max_reconnect_attempts, 5);
    }

    #[test]
    fn frame_index_increments() {
        let msgs = vec![
            Message::Text("a".into()),
            Message::Pong(vec![].into()), // skipped
            Message::Text("b".into()),
        ];

        let mut idx: u64 = 0;
        let mut events = Vec::new();
        for msg in msgs {
            if let Some(event) = map_message_to_event(msg, idx) {
                events.push(event);
                idx += 1;
            }
        }

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id.as_deref(), Some("0"));
        assert_eq!(events[1].id.as_deref(), Some("1"));
    }

    // Integration tests require a running WebSocket server — marked #[ignore]
    #[tokio::test]
    #[ignore = "requires WebSocket echo server"]
    async fn connect_to_echo_server() {
        let source = WebSocketSource::new(WebSocketConfig {
            subscribe_message: Some("hello".into()),
            timeout_secs: 5,
            ..WebSocketConfig::default()
        });
        let events = source
            .subscribe("ws://127.0.0.1:9001/echo", Some(1))
            .await
            .expect("connect");
        assert!(!events.is_empty());
    }
}
