//! Browser session recording and debugging tools.
//!
//! Captures CDP events, network traffic, and performance metrics for
//! debugging failed scraping runs, analysing anti-bot detection, and
//! performance profiling.
//!
//! ## Configuration
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `STYGIAN_RECORD_SESSION` | `false` | Enable recording automatically |
//! | `STYGIAN_RECORD_DIR` | `./recordings` | Output directory |
//!
//! ## HAR export
//!
//! Records all network requests in the
//! [HTTP Archive (HAR 1.2)](https://w3c.github.io/web-performance/specs/HAR/Overview.html)
//! format, which can be opened in Chrome `DevTools`, Fiddler, or analysed
//! programmatically.
//!
//! ## Example
//!
//! ```no_run
//! use stygian_browser::{BrowserPool, BrowserConfig, WaitUntil};
//! use stygian_browser::recorder::{SessionRecorder, RecorderConfig};
//! use std::time::Duration;
//!
//! # async fn run() -> stygian_browser::error::Result<()> {
//! let pool = BrowserPool::new(BrowserConfig::default()).await?;
//! let handle = pool.acquire().await?;
//! let mut page = handle.browser().expect("valid browser").new_page().await?;
//!
//! let mut recorder = SessionRecorder::start(RecorderConfig::default());
//! page.navigate("https://example.com", WaitUntil::Selector("body".to_string()), Duration::from_secs(30)).await?;
//!
//! // Log a CDP event manually
//! recorder.record_event("Page.loadEventFired", serde_json::json!({"timestamp": 1234.5}));
//!
//! // Export HAR
//! recorder.stop();
//! recorder.export_har("session.har")?;
//! # Ok(())
//! # }
//! ```

use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use crate::error::{BrowserError, Result};

// ─── RecorderConfig ───────────────────────────────────────────────────────────

/// Configuration for a [`SessionRecorder`].
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Directory to write recording files to.
    pub output_dir: std::path::PathBuf,
    /// Maximum number of CDP events to buffer (older events are dropped first).
    pub max_events: usize,
    /// Maximum number of network entries to buffer.
    pub max_network_entries: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        let output_dir = std::env::var("STYGIAN_RECORD_DIR").map_or_else(
            |_| std::path::PathBuf::from("./recordings"),
            std::path::PathBuf::from,
        );

        Self {
            output_dir,
            max_events: 10_000,
            max_network_entries: 5_000,
        }
    }
}

// ─── CDP event log ────────────────────────────────────────────────────────────

/// A single recorded CDP event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpEvent {
    /// Monotonic offset from the recorder start in milliseconds.
    pub elapsed_ms: u64,
    /// CDP method name (e.g. `"Network.requestWillBeSent"`).
    pub method: String,
    /// Event payload as JSON.
    pub params: Value,
}

// ─── HAR types ────────────────────────────────────────────────────────────────

/// HAR 1.2 root object.
#[derive(Debug, Serialize, Deserialize)]
pub struct Har {
    /// HAR root.
    pub log: HarLog,
}

/// HAR log.
#[derive(Debug, Serialize, Deserialize)]
pub struct HarLog {
    /// HAR version.
    pub version: String,
    /// Creator metadata.
    pub creator: HarCreator,
    /// List of HTTP transactions.
    pub entries: Vec<HarEntry>,
}

/// HAR creator metadata.
#[derive(Debug, Serialize, Deserialize)]
pub struct HarCreator {
    /// Name.
    pub name: String,
    /// Version.
    pub version: String,
}

/// A single HAR network entry (request + response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarEntry {
    /// ISO 8601 timestamp of the request start.
    pub started_date_time: String,
    /// Total elapsed time in milliseconds.
    pub time: f64,
    /// HTTP request.
    pub request: HarRequest,
    /// HTTP response.
    pub response: HarResponse,
    /// Additional timing details.
    pub timings: HarTimings,
}

/// A HAR HTTP request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarRequest {
    /// HTTP method.
    pub method: String,
    /// Full request URL.
    pub url: String,
    /// HTTP version (e.g. `"HTTP/1.1"`).
    pub http_version: String,
    /// Request headers.
    pub headers: Vec<HarHeader>,
    /// Query string parameters.
    pub query_string: Vec<HarQueryParam>,
    /// Total bytes transferred.
    pub headers_size: i64,
    /// POST body size (-1 = unknown).
    pub body_size: i64,
}

/// A HAR HTTP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarResponse {
    /// HTTP status code.
    pub status: u16,
    /// Status text (e.g. `"OK"`).
    pub status_text: String,
    /// HTTP version.
    pub http_version: String,
    /// Response headers.
    pub headers: Vec<HarHeader>,
    /// MIME type of response body.
    pub content_mime_type: String,
    /// Response body size in bytes (-1 = unknown).
    pub body_size: i64,
}

/// A single HTTP header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarHeader {
    /// Header name.
    pub name: String,
    /// Header value.
    pub value: String,
}

/// Query string parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarQueryParam {
    /// Parameter name.
    pub name: String,
    /// Parameter value.
    pub value: String,
}

/// HAR timing breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarTimings {
    /// Time to receive response (ms).
    pub receive: f64,
}

// ─── Network entry (internal) ─────────────────────────────────────────────────

/// Internal representation of a recorded network transaction.
#[derive(Debug, Clone)]
struct NetworkEntry {
    started_at: Instant,
    started_iso: String,
    #[allow(dead_code)]
    request_id: String,
    method: String,
    url: String,
    request_headers: Vec<HarHeader>,
    status: u16,
    status_text: String,
    response_headers: Vec<HarHeader>,
    mime_type: String,
    encoded_data_length: i64,
}

/// Returns the current time as an ISO 8601 string.
fn iso_timestamp() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = d.as_secs();
    let millis = d.subsec_millis();
    // Simple ISO 8601 (no chrono dep): YYYY-MM-DDTHH:MM:SS.mmmZ
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Epoch is 1970-01-01 — compute approximate date (no tz, no leap seconds)
    let (year, month, day) = epoch_days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Very small epoch-days → (y, m, d) conversion (Gregorian, UTC approximate).
fn epoch_days_to_ymd(days: u64) -> (u32, u32, u32) {
    // 400-year cycle = 146097 days
    let d = i64::try_from(days)
        .unwrap_or(i64::MAX)
        .saturating_add(719_468); // offset to 0000-03-01
    let era = d.div_euclid(146_097);
    let doe = d.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (
        u32::try_from(year).unwrap_or(9999),
        u32::try_from(month).unwrap_or(12),
        u32::try_from(day).unwrap_or(31),
    )
}

/// Parse query string `?k=v&k2=v2` into HAR params.
fn parse_query(url: &str) -> Vec<HarQueryParam> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| HarQueryParam {
            name: k.to_string(),
            value: v.to_string(),
        })
        .collect()
}

// ─── SessionRecorder ──────────────────────────────────────────────────────────

/// Records CDP events and network traffic during a browser session.
///
/// Create one per scraping job, call [`record_event`](Self::record_event) for
/// each CDP event you want to log, then call [`stop`](Self::stop) and
/// [`export_har`](Self::export_har) when the session ends.
pub struct SessionRecorder {
    config: RecorderConfig,
    start: Instant,
    running: AtomicBool,
    events: std::sync::Mutex<Vec<CdpEvent>>,
    /// Pending requests by requestId
    pending: std::sync::Mutex<HashMap<String, NetworkEntry>>,
    /// Completed network transactions
    completed: std::sync::Mutex<Vec<NetworkEntry>>,
}

impl SessionRecorder {
    /// Start a new recorder with the given `config`.
    pub fn start(config: RecorderConfig) -> Self {
        debug!("SessionRecorder started");
        Self {
            config,
            start: Instant::now(),
            running: AtomicBool::new(true),
            events: std::sync::Mutex::new(Vec::new()),
            pending: std::sync::Mutex::new(HashMap::new()),
            completed: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Returns `true` if the recorder is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Stop the recorder. No more events will be buffered after this.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        debug!("SessionRecorder stopped");
    }

    /// Record a raw CDP event by method name and parameters.
    ///
    /// Call this for every CDP event you receive from the browser.
    /// The recorder automatically tracks `Network.requestWillBeSent` and
    /// `Network.responseReceived` events to build HAR entries.
    pub fn record_event(&self, method: &str, params: Value) {
        if !self.is_running() {
            return;
        }

        let elapsed_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Handle network events for HAR building
        match method {
            "Network.requestWillBeSent" => self.on_request_sent(&params, elapsed_ms),
            "Network.responseReceived" => self.on_response_received(&params),
            "Network.loadingFinished" => self.on_loading_finished(&params),
            _ => {}
        }

        let Ok(mut guard) = self.events.lock() else {
            return;
        };

        if guard.len() >= self.config.max_events {
            guard.remove(0);
        }
        guard.push(CdpEvent {
            elapsed_ms,
            method: method.to_string(),
            params,
        });
    }

    /// Export the buffered CDP event log as a newline-delimited JSON file.
    ///
    /// # Errors
    ///
    /// Returns an IO error if the file cannot be written.
    pub fn export_event_log(&self, path: impl AsRef<Path>) -> Result<()> {
        let guard = self
            .events
            .lock()
            .map_err(|_| BrowserError::ConfigError("event log lock poisoned".to_string()))?;

        let mut lines: Vec<String> = Vec::with_capacity(guard.len());
        for event in guard.iter() {
            if let Ok(s) = serde_json::to_string(event) {
                lines.push(s);
            }
        }
        drop(guard);

        std::fs::write(path, lines.join("\n")).map_err(BrowserError::Io)
    }

    /// Export captured network transactions as a HAR 1.2 file.
    ///
    /// # Errors
    ///
    /// Returns an IO or serialisation error if the file cannot be written.
    pub fn export_har(&self, path: impl AsRef<Path>) -> Result<()> {
        let har = self.build_har();
        let json = serde_json::to_string_pretty(&har)
            .map_err(|e| BrowserError::ConfigError(format!("Failed to serialise HAR: {e}")))?;
        std::fs::create_dir_all(path.as_ref().parent().unwrap_or_else(|| Path::new(".")))
            .map_err(BrowserError::Io)?;
        std::fs::write(path, json).map_err(BrowserError::Io)
    }

    /// Return the number of buffered CDP events.
    pub fn event_count(&self) -> usize {
        self.events.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Return the number of completed network entries.
    pub fn network_entry_count(&self) -> usize {
        self.completed.lock().map(|g| g.len()).unwrap_or(0)
    }

    // ── Network event handlers ─────────────────────────────────────────────────

    fn on_request_sent(&self, params: &Value, _elapsed_ms: u64) {
        let request_id = params
            .get("requestId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let method = params
            .pointer("/request/method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();
        let url = params
            .pointer("/request/url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let request_headers: Vec<HarHeader> = params
            .pointer("/request/headers")
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| {
                m.iter().map(|(k, v)| HarHeader {
                    name: k.clone(),
                    value: v.as_str().unwrap_or("").to_string(),
                })
            })
            .collect();

        let entry = NetworkEntry {
            started_at: Instant::now(),
            started_iso: iso_timestamp(),
            request_id: request_id.clone(),
            method,
            url,
            request_headers,
            status: 0,
            status_text: String::new(),
            response_headers: vec![],
            mime_type: String::new(),
            encoded_data_length: -1,
        };

        if let Ok(mut guard) = self.pending.lock() {
            guard.insert(request_id, entry);
        }
    }

    fn on_response_received(&self, params: &Value) {
        let request_id = params
            .get("requestId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let status = u16::try_from(
            params
                .pointer("/response/status")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let status_text = params
            .pointer("/response/statusText")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mime_type = params
            .pointer("/response/mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let response_headers: Vec<HarHeader> = params
            .pointer("/response/headers")
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| {
                m.iter().map(|(k, v)| HarHeader {
                    name: k.clone(),
                    value: v.as_str().unwrap_or("").to_string(),
                })
            })
            .collect();

        if let Ok(mut guard) = self.pending.lock()
            && let Some(entry) = guard.get_mut(&request_id)
        {
            entry.status = status;
            entry.status_text = status_text;
            entry.mime_type = mime_type;
            entry.response_headers = response_headers;
        }
    }

    fn on_loading_finished(&self, params: &Value) {
        let request_id = params
            .get("requestId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let encoded_data_length = params
            .get("encodedDataLength")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);

        let Ok(mut pending_guard) = self.pending.lock() else {
            return;
        };

        if let Some(mut entry) = pending_guard.remove(&request_id) {
            entry.encoded_data_length = encoded_data_length;
            if let Ok(mut completed) = self.completed.lock() {
                if completed.len() >= self.config.max_network_entries {
                    completed.remove(0);
                }
                completed.push(entry);
            }
        }
    }

    fn build_har(&self) -> Har {
        let completed = self.completed.lock().map(|g| g.clone()).unwrap_or_default();

        let entries: Vec<HarEntry> = completed
            .into_iter()
            .map(|entry| {
                let elapsed = entry.started_at.elapsed().as_secs_f64() * 1000.0;
                let query_string = parse_query(&entry.url);
                HarEntry {
                    started_date_time: entry.started_iso.clone(),
                    time: elapsed,
                    request: HarRequest {
                        method: entry.method,
                        url: entry.url,
                        http_version: "HTTP/1.1".to_string(),
                        headers: entry.request_headers,
                        query_string,
                        headers_size: -1,
                        body_size: -1,
                    },
                    response: HarResponse {
                        status: entry.status,
                        status_text: entry.status_text,
                        http_version: "HTTP/1.1".to_string(),
                        headers: entry.response_headers,
                        content_mime_type: entry.mime_type,
                        body_size: entry.encoded_data_length,
                    },
                    timings: HarTimings { receive: elapsed },
                }
            })
            .collect();

        Har {
            log: HarLog {
                version: "1.2".to_string(),
                creator: HarCreator {
                    name: "stygian-browser".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                entries,
            },
        }
    }
}

// ─── Convenience helpers ──────────────────────────────────────────────────────

/// Returns `true` if session recording is enabled via `STYGIAN_RECORD_SESSION`.
pub fn is_recording_enabled() -> bool {
    matches!(
        std::env::var("STYGIAN_RECORD_SESSION")
            .unwrap_or_default()
            .to_lowercase()
            .as_str(),
        "true" | "1" | "yes"
    )
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_recorder() -> SessionRecorder {
        SessionRecorder::start(RecorderConfig {
            output_dir: std::env::temp_dir(),
            max_events: 100,
            max_network_entries: 50,
        })
    }

    #[test]
    fn recorder_starts_running() {
        let r = test_recorder();
        assert!(r.is_running());
    }

    #[test]
    fn recorder_stops() {
        let r = test_recorder();
        r.stop();
        assert!(!r.is_running());
    }

    #[test]
    fn records_events_while_running() {
        let r = test_recorder();
        r.record_event("Page.loadEventFired", serde_json::json!({"timestamp": 1.0}));
        r.record_event("Page.frameNavigated", serde_json::json!({}));
        assert_eq!(r.event_count(), 2);
    }

    #[test]
    fn does_not_record_after_stop() {
        let r = test_recorder();
        r.stop();
        r.record_event("Page.loadEventFired", serde_json::json!({}));
        assert_eq!(r.event_count(), 0);
    }

    #[test]
    fn max_events_caps_buffer() {
        let r = SessionRecorder::start(RecorderConfig {
            output_dir: std::env::temp_dir(),
            max_events: 3,
            max_network_entries: 10,
        });
        for i in 0..10 {
            r.record_event("Test.event", serde_json::json!({"i": i}));
        }
        assert_eq!(r.event_count(), 3);
    }

    #[test]
    fn network_tracking_builds_entry() {
        let r = test_recorder();

        r.record_event(
            "Network.requestWillBeSent",
            serde_json::json!({
                "requestId": "req-1",
                "request": {
                    "method": "GET",
                    "url": "https://example.com/api?foo=bar",
                    "headers": {"User-Agent": "test/1.0"}
                }
            }),
        );

        r.record_event(
            "Network.responseReceived",
            serde_json::json!({
                "requestId": "req-1",
                "response": {
                    "status": 200,
                    "statusText": "OK",
                    "mimeType": "application/json",
                    "headers": {"Content-Type": "application/json"}
                }
            }),
        );

        r.record_event(
            "Network.loadingFinished",
            serde_json::json!({
                "requestId": "req-1",
                "encodedDataLength": 512
            }),
        );

        assert_eq!(r.network_entry_count(), 1);
    }

    #[test]
    fn export_har_writes_valid_json() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let r = test_recorder();

        // Send a complete network transaction
        r.record_event(
            "Network.requestWillBeSent",
            serde_json::json!({
                "requestId": "r1",
                "request": {"method": "GET", "url": "https://example.com/", "headers": {}}
            }),
        );
        r.record_event(
            "Network.responseReceived",
            serde_json::json!({
                "requestId": "r1",
                "response": {"status": 200, "statusText": "OK", "mimeType": "text/html", "headers": {}}
            }),
        );
        r.record_event(
            "Network.loadingFinished",
            serde_json::json!({"requestId": "r1", "encodedDataLength": 1024}),
        );

        let path = std::env::temp_dir().join("stygian_test.har");
        r.export_har(&path)?;

        let contents = std::fs::read_to_string(&path)?;
        let har: Har = serde_json::from_str(&contents)?;
        assert_eq!(har.log.entries.len(), 1);
        if let Some(entry) = har.log.entries.first() {
            assert_eq!(entry.request.method, "GET");
            assert_eq!(entry.response.status, 200);
        }
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn event_log_export_writes_ndjson() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let r = test_recorder();
        r.record_event("A", serde_json::json!({"x": 1}));
        r.record_event("B", serde_json::json!({"y": 2}));

        let path = std::env::temp_dir().join("stygian_events.ndjson");
        r.export_event_log(&path)?;

        let contents = std::fs::read_to_string(&path)?;
        assert_eq!(contents.lines().count(), 2);
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    #[test]
    fn parse_query_string() {
        let params = parse_query("https://example.com/path?a=1&b=hello%20world");
        assert_eq!(params.len(), 2);
        let names: Vec<_> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"a"), "missing 'a'");
        assert!(names.contains(&"b"), "missing 'b'");
    }

    #[test]
    fn iso_timestamp_format() {
        let ts = iso_timestamp();
        // Should look like 2024-01-15T12:34:56.789Z
        assert!(ts.ends_with('Z'), "should end with Z: {ts}");
        assert_eq!(ts.len(), 24, "length should be 24: {ts}");
    }
}
