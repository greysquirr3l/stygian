//! Server-Sent Events (SSE) stream adapter.
//!
//! Implements [`StreamSourcePort`] and [`ScrapingService`] for consuming
//! SSE event streams via HTTP.  Uses `reqwest` for the underlying connection.
//!
//! # Example
//!
//! ```no_run
//! use stygian_graph::adapters::stream::SseSource;
//! use stygian_graph::ports::stream_source::StreamSourcePort;
//!
//! # async fn example() {
//! let source = SseSource::new(None);
//! let events = source.subscribe("https://api.example.com/events", Some(5)).await.unwrap();
//! println!("received {} events", events.len());
//! # }
//! ```

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::domain::error::{Result, ServiceError, StygianError};
use crate::ports::stream_source::{StreamEvent, StreamSourcePort};
use crate::ports::{ScrapingService, ServiceInput, ServiceOutput};

// ─────────────────────────────────────────────────────────────────────────────
// SseSource
// ─────────────────────────────────────────────────────────────────────────────

/// Adapter: Server-Sent Events stream source.
///
/// Connects to an SSE endpoint and collects events until `max_events`
/// is reached or the stream closes.
pub struct SseSource {
    client: Client,
}

impl SseSource {
    /// Create a new SSE stream source.
    ///
    /// # Arguments
    ///
    /// * `client` - Optional pre-configured `reqwest::Client`.  If `None`,
    ///   a default client is created.
    #[must_use]
    pub fn new(client: Option<Client>) -> Self {
        Self {
            client: client.unwrap_or_default(),
        }
    }

    /// Parse a single SSE frame from accumulated field lines.
    fn parse_event(lines: &[String]) -> Option<StreamEvent> {
        let mut id = None;
        let mut event_type = None;
        let mut data_lines: Vec<&str> = Vec::new();

        for line in lines {
            if let Some(value) = line.strip_prefix("id:") {
                id = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("event:") {
                event_type = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim());
            }
        }

        if data_lines.is_empty() {
            return None;
        }

        Some(StreamEvent {
            id,
            event_type,
            data: data_lines.join("\n"),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StreamSourcePort
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl StreamSourcePort for SseSource {
    async fn subscribe(&self, url: &str, max_events: Option<usize>) -> Result<Vec<StreamEvent>> {
        let response = self
            .client
            .get(url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| {
                StygianError::Service(ServiceError::Unavailable(format!(
                    "SSE connection to {url} failed: {e}"
                )))
            })?;

        let text = response.text().await.map_err(|e| {
            StygianError::Service(ServiceError::InvalidResponse(format!(
                "failed to read SSE body: {e}"
            )))
        })?;

        let mut events = Vec::new();
        let mut current_frame: Vec<String> = Vec::new();

        for line in text.lines() {
            if line.is_empty() {
                // Empty line = event boundary in SSE
                if let Some(event) = Self::parse_event(&current_frame) {
                    events.push(event);
                    if let Some(max) = max_events
                        && events.len() >= max
                    {
                        break;
                    }
                }
                current_frame.clear();
            } else if !line.starts_with(':') {
                // Lines starting with ':' are SSE comments — skip them
                current_frame.push(line.to_string());
            }
        }

        // Handle final event if no trailing blank line
        if !current_frame.is_empty()
            && let Some(event) = Self::parse_event(&current_frame)
        {
            events.push(event);
        }

        Ok(events)
    }

    fn source_name(&self) -> &str {
        "sse"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScrapingService (DAG integration)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl ScrapingService for SseSource {
    /// Connect to an SSE endpoint and collect events.
    ///
    /// Expected params:
    /// ```json
    /// { "max_events": 10 }
    /// ```
    async fn execute(&self, input: ServiceInput) -> Result<ServiceOutput> {
        let max_events = input.params["max_events"].as_u64().map(|n| n as usize);

        let events = self.subscribe(&input.url, max_events).await?;
        let event_count = events.len();

        Ok(ServiceOutput {
            data: serde_json::to_string(&events).unwrap_or_default(),
            metadata: json!({
                "source": "sse",
                "event_count": event_count,
            }),
        })
    }

    fn name(&self) -> &'static str {
        "stream"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_basic() {
        let lines = vec![
            "event:message".to_string(),
            "data:{\"price\":29.99}".to_string(),
        ];
        let event = SseSource::parse_event(&lines).unwrap();
        assert_eq!(event.event_type.as_deref(), Some("message"));
        assert_eq!(event.data, r#"{"price":29.99}"#);
        assert!(event.id.is_none());
    }

    #[test]
    fn parse_event_with_id() {
        let lines = vec![
            "id:42".to_string(),
            "event:update".to_string(),
            "data:hello".to_string(),
        ];
        let event = SseSource::parse_event(&lines).unwrap();
        assert_eq!(event.id.as_deref(), Some("42"));
        assert_eq!(event.event_type.as_deref(), Some("update"));
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn parse_event_multiline_data() {
        let lines = vec!["data:line one".to_string(), "data:line two".to_string()];
        let event = SseSource::parse_event(&lines).unwrap();
        assert_eq!(event.data, "line one\nline two");
    }

    #[test]
    fn parse_event_no_data_returns_none() {
        let lines = vec!["event:ping".to_string()];
        assert!(SseSource::parse_event(&lines).is_none());
    }
}
