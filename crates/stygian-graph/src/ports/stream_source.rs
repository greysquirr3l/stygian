//! Stream source port trait for event-driven data sources.
//!
//! Defines the interface for streaming data sources such as WebSocket feeds,
//! Server-Sent Events (SSE), or message queues.  Adapters implement this
//! trait to provide event streams that can integrate into the DAG pipeline.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::error::Result;

/// A single event received from a streaming source.
///
/// # Example
///
/// ```
/// use stygian_graph::ports::stream_source::StreamEvent;
///
/// let event = StreamEvent {
///     id: Some("42".into()),
///     event_type: Some("message".into()),
///     data: r#"{"price": 29.99}"#.into(),
/// };
/// assert_eq!(event.event_type.as_deref(), Some("message"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Optional event identifier (e.g. SSE `id` field).
    pub id: Option<String>,
    /// Optional event type (e.g. SSE `event` field).
    pub event_type: Option<String>,
    /// Event payload (JSON, text, etc.).
    pub data: String,
}

/// Port trait for streaming data sources.
///
/// Implementations connect to event-driven sources and collect events
/// until a termination condition is met (timeout, count limit, or
/// stream close).
///
/// # Example
///
/// ```no_run
/// use stygian_graph::ports::stream_source::{StreamSourcePort, StreamEvent};
///
/// # async fn example(source: impl StreamSourcePort) {
/// let events = source
///     .subscribe("wss://feed.example.com/prices", Some(100))
///     .await
///     .unwrap();
/// for event in &events {
///     println!("got: {}", event.data);
/// }
/// # }
/// ```
#[async_trait]
pub trait StreamSourcePort: Send + Sync {
    /// Subscribe to a stream and collect events.
    ///
    /// # Arguments
    ///
    /// * `url` - Stream endpoint (wss://, https:// for SSE, etc.)
    /// * `max_events` - Optional cap on number of events to collect before
    ///   returning.  `None` means collect until the stream closes or a
    ///   provider-defined timeout.
    ///
    /// # Returns
    ///
    /// A vector of collected [`StreamEvent`]s.
    async fn subscribe(&self, url: &str, max_events: Option<usize>) -> Result<Vec<StreamEvent>>;

    /// Name of this stream source for logging and identification.
    fn source_name(&self) -> &str;
}
