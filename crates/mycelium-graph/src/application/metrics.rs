//! Prometheus metrics for mycelium pipelines.
//!
//! Provides a process-wide [`MetricsRegistry`] that tracks:
//!
//! - **Counters**: requests, errors, cache hits/misses
//! - **Histograms**: request duration, pipeline execution time
//! - **Gauges**: active workers, queue depth, circuit-breaker state
//!
//! # Example
//!
//! ```
//! use mycelium_graph::application::metrics::{MetricsRegistry, MetricEvent};
//!
//! let registry = MetricsRegistry::new();
//! registry.record(MetricEvent::RequestStarted { service: "http".into() });
//! registry.record(MetricEvent::RequestCompleted {
//!     service: "http".into(),
//!     duration_ms: 142,
//!     success: true,
//! });
//!
//! let snapshot = registry.snapshot();
//! assert_eq!(snapshot.requests_total, 1);
//! assert_eq!(snapshot.errors_total, 0);
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::fmt::Write;

use serde::{Deserialize, Serialize};

// ─── Events ──────────────────────────────────────────────────────────────────

/// An observable event emitted by the pipeline engine.
///
/// Call [`MetricsRegistry::record`] to update counters and histograms.
#[derive(Debug, Clone)]
pub enum MetricEvent {
    /// A scraping request has started.
    RequestStarted {
        /// Service name (e.g. `"http"`, `"claude"`)
        service: String,
    },
    /// A scraping request has completed (succeeded or failed).
    RequestCompleted {
        /// Service name
        service: String,
        /// Round-trip time in milliseconds
        duration_ms: u64,
        /// `true` on HTTP 2xx / GraphQL without errors
        success: bool,
    },
    /// An AI provider token usage event.
    TokenUsage {
        /// Provider name (e.g. `"claude"`, `"openai"`)
        provider: String,
        /// Input/prompt tokens consumed
        input_tokens: u64,
        /// Output/completion tokens generated
        output_tokens: u64,
    },
    /// A cache lookup was performed.
    CacheAccess {
        /// `true` = cache hit, `false` = miss
        hit: bool,
    },
    /// The active worker count changed.
    WorkerCountChanged {
        /// Current number of live worker tasks
        count: i64,
    },
    /// The worker queue depth changed.
    QueueDepthChanged {
        /// Number of tasks waiting in the queue
        depth: i64,
    },
    /// A full pipeline execution completed.
    PipelineExecuted {
        /// Pipeline identifier (file path or name)
        pipeline_id: String,
        /// Total wall-clock time in milliseconds
        duration_ms: u64,
        /// Whether the pipeline finished without errors
        success: bool,
    },
    /// A circuit breaker changed state.
    CircuitBreakerStateChanged {
        /// Service the breaker protects
        service: String,
        /// `"closed"`, `"open"`, or `"half-open"`
        state: String,
    },
}

// ─── Per-service counters ─────────────────────────────────────────────────────

#[derive(Default)]
struct ServiceCounters {
    requests: AtomicU64,
    errors: AtomicU64,
    total_duration: AtomicU64,
}

// ─── MetricsRegistry ─────────────────────────────────────────────────────────

/// Thread-safe registry for all pipeline metrics.
///
/// Maintains atomic counters and histograms in-process; call [`MetricsRegistry::render_prometheus`]
/// to produce a Prometheus text-format scrape payload suitable for a `/metrics` endpoint.
pub struct MetricsRegistry {
    // Global counters
    requests_total: AtomicU64,
    errors_total: AtomicU64,
    cache_hits_total: AtomicU64,
    cache_misses_total: AtomicU64,
    pipelines_total: AtomicU64,
    pipeline_errors_total: AtomicU64,
    input_tokens_total: AtomicU64,
    output_tokens_total: AtomicU64,

    // Gauges
    active_workers: AtomicI64,
    queue_depth: AtomicI64,

    // Duration histogram buckets (milliseconds, cumulative counts)
    // Buckets: 10, 50, 100, 250, 500, 1000, 2500, 5000, 10000, +Inf
    request_duration_buckets: [AtomicU64; 10],
    pipeline_duration_buckets: [AtomicU64; 10],
    request_duration_sum: AtomicU64,
    pipeline_duration_sum: AtomicU64,

    // Per-service breakdown
    services: RwLock<HashMap<String, Arc<ServiceCounters>>>,
}

const DURATION_BOUNDS: [u64; 9] = [10, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

fn bucket_index(ms: u64) -> usize {
    DURATION_BOUNDS.iter().position(|&b| ms <= b).unwrap_or(9)
}

#[allow(clippy::unwrap_used)]
impl MetricsRegistry {
    /// Create an empty registry.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::MetricsRegistry;
    /// let r = MetricsRegistry::new();
    /// assert_eq!(r.snapshot().requests_total, 0);
    /// ```
    pub fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            cache_hits_total: AtomicU64::new(0),
            cache_misses_total: AtomicU64::new(0),
            pipelines_total: AtomicU64::new(0),
            pipeline_errors_total: AtomicU64::new(0),
            input_tokens_total: AtomicU64::new(0),
            output_tokens_total: AtomicU64::new(0),
            active_workers: AtomicI64::new(0),
            queue_depth: AtomicI64::new(0),
            request_duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            pipeline_duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            request_duration_sum: AtomicU64::new(0),
            pipeline_duration_sum: AtomicU64::new(0),
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Record a metric event, updating all relevant counters and histograms.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::{MetricsRegistry, MetricEvent};
    ///
    /// let r = MetricsRegistry::new();
    /// r.record(MetricEvent::CacheAccess { hit: true });
    /// assert_eq!(r.snapshot().cache_hits_total, 1);
    /// ```
    #[allow(clippy::indexing_slicing)]
    pub fn record(&self, event: MetricEvent) {
        match event {
            MetricEvent::RequestStarted { service } => {
                self.requests_total.fetch_add(1, Ordering::Relaxed);
                self.service_counters(&service)
                    .requests
                    .fetch_add(1, Ordering::Relaxed);
            }
            MetricEvent::RequestCompleted {
                service,
                duration_ms,
                success,
            } => {
                if !success {
                    self.errors_total.fetch_add(1, Ordering::Relaxed);
                    self.service_counters(&service)
                        .errors
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.service_counters(&service)
                    .total_duration
                    .fetch_add(duration_ms, Ordering::Relaxed);
                let idx = bucket_index(duration_ms);
                for bucket in &self.request_duration_buckets[idx..] {
                    bucket.fetch_add(1, Ordering::Relaxed);
                }
                self.request_duration_sum
                    .fetch_add(duration_ms, Ordering::Relaxed);
            }
            MetricEvent::TokenUsage {
                input_tokens,
                output_tokens,
                ..
            } => {
                self.input_tokens_total
                    .fetch_add(input_tokens, Ordering::Relaxed);
                self.output_tokens_total
                    .fetch_add(output_tokens, Ordering::Relaxed);
            }
            MetricEvent::CacheAccess { hit } => {
                if hit {
                    self.cache_hits_total.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.cache_misses_total.fetch_add(1, Ordering::Relaxed);
                }
            }
            MetricEvent::WorkerCountChanged { count } => {
                self.active_workers.store(count, Ordering::Relaxed);
            }
            MetricEvent::QueueDepthChanged { depth } => {
                self.queue_depth.store(depth, Ordering::Relaxed);
            }
            MetricEvent::PipelineExecuted {
                duration_ms,
                success,
                ..
            } => {
                self.pipelines_total.fetch_add(1, Ordering::Relaxed);
                if !success {
                    self.pipeline_errors_total.fetch_add(1, Ordering::Relaxed);
                }
                let idx = bucket_index(duration_ms);
                for bucket in &self.pipeline_duration_buckets[idx..] {
                    bucket.fetch_add(1, Ordering::Relaxed);
                }
                self.pipeline_duration_sum
                    .fetch_add(duration_ms, Ordering::Relaxed);
            }
            MetricEvent::CircuitBreakerStateChanged { .. } => {
                // State changes are captured in health checks; no separate counter needed.
            }
        }
    }

    /// Take an in-memory snapshot of all current counter values.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::{MetricsRegistry, MetricEvent};
    ///
    /// let r = MetricsRegistry::new();
    /// r.record(MetricEvent::RequestStarted { service: "http".into() });
    /// let snap = r.snapshot();
    /// assert_eq!(snap.requests_total, 1);
    /// ```
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            errors_total: self.errors_total.load(Ordering::Relaxed),
            cache_hits_total: self.cache_hits_total.load(Ordering::Relaxed),
            cache_misses_total: self.cache_misses_total.load(Ordering::Relaxed),
            pipelines_total: self.pipelines_total.load(Ordering::Relaxed),
            pipeline_errors_total: self.pipeline_errors_total.load(Ordering::Relaxed),
            input_tokens_total: self.input_tokens_total.load(Ordering::Relaxed),
            output_tokens_total: self.output_tokens_total.load(Ordering::Relaxed),
            active_workers: self.active_workers.load(Ordering::Relaxed),
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
        }
    }

    /// Render all metrics in Prometheus text exposition format.
    ///
    /// Suitable for serving from a `/metrics` HTTP endpoint.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::MetricsRegistry;
    ///
    /// let r = MetricsRegistry::new();
    /// let text = r.render_prometheus();
    /// assert!(text.contains("mycelium_requests_total"));
    /// assert!(text.contains("mycelium_errors_total"));
    /// ```
    #[allow(clippy::too_many_lines, clippy::indexing_slicing, clippy::format_push_string)]
    pub fn render_prometheus(&self) -> String {
        let snap = self.snapshot();
        let mut out = String::with_capacity(2048);

        macro_rules! counter {
            ($name:expr, $help:expr, $val:expr) => {
                out.push_str(&format!(
                    "# HELP {name} {help}\n# TYPE {name} counter\n{name} {val}\n",
                    name = $name,
                    help = $help,
                    val = $val
                ));
            };
        }
        macro_rules! gauge {
            ($name:expr, $help:expr, $val:expr) => {
                out.push_str(&format!(
                    "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {val}\n",
                    name = $name,
                    help = $help,
                    val = $val
                ));
            };
        }

        counter!(
            "mycelium_requests_total",
            "Total scraping requests initiated",
            snap.requests_total
        );
        counter!(
            "mycelium_errors_total",
            "Total scraping request failures",
            snap.errors_total
        );
        counter!(
            "mycelium_cache_hits_total",
            "Total cache hits",
            snap.cache_hits_total
        );
        counter!(
            "mycelium_cache_misses_total",
            "Total cache misses",
            snap.cache_misses_total
        );
        counter!(
            "mycelium_pipelines_total",
            "Total pipeline executions",
            snap.pipelines_total
        );
        counter!(
            "mycelium_pipeline_errors_total",
            "Total pipeline execution failures",
            snap.pipeline_errors_total
        );
        counter!(
            "mycelium_input_tokens_total",
            "Total AI input/prompt tokens consumed",
            snap.input_tokens_total
        );
        counter!(
            "mycelium_output_tokens_total",
            "Total AI output/completion tokens generated",
            snap.output_tokens_total
        );
        gauge!(
            "mycelium_active_workers",
            "Current number of active worker goroutines",
            snap.active_workers
        );
        gauge!(
            "mycelium_queue_depth",
            "Current worker queue depth",
            snap.queue_depth
        );

        // Request duration histogram
        out.push_str("# HELP mycelium_request_duration_ms Request duration distribution (ms)\n");
        out.push_str("# TYPE mycelium_request_duration_ms histogram\n");
        let labels = [10, 50, 100, 250, 500, 1000, 2500, 5000, 10000];
        for (i, bound) in labels.iter().enumerate() {
            out.push_str(&format!(
                "mycelium_request_duration_ms_bucket{{le=\"{bound}\"}} {}\n",
                self.request_duration_buckets[i].load(Ordering::Relaxed)
            ));
        }
        out.push_str(&format!(
            "mycelium_request_duration_ms_bucket{{le=\"+Inf\"}} {}\n",
            self.request_duration_buckets[9].load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "mycelium_request_duration_ms_sum {}\n",
            self.request_duration_sum.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "mycelium_request_duration_ms_count {}\n",
            snap.requests_total
        ));

        // Pipeline duration histogram
        out.push_str("# HELP mycelium_pipeline_duration_ms Pipeline execution duration (ms)\n");
        out.push_str("# TYPE mycelium_pipeline_duration_ms histogram\n");
        for (i, bound) in labels.iter().enumerate() {
            out.push_str(&format!(
                "mycelium_pipeline_duration_ms_bucket{{le=\"{bound}\"}} {}\n",
                self.pipeline_duration_buckets[i].load(Ordering::Relaxed)
            ));
        }
        out.push_str(&format!(
            "mycelium_pipeline_duration_ms_bucket{{le=\"+Inf\"}} {}\n",
            self.pipeline_duration_buckets[9].load(Ordering::Relaxed)
        ));
        let _ = writeln!(
            &mut out,
            "mycelium_pipeline_duration_ms_sum {}",
            self.pipeline_duration_sum.load(Ordering::Relaxed)
        );
        let _ = writeln!(&mut out, "mycelium_pipeline_duration_ms_count {}", snap.pipelines_total);

        out
    }

    fn service_counters(&self, name: &str) -> Arc<ServiceCounters> {
        {
            let read = self.services.read().unwrap();
            if let Some(c) = read.get(name) {
                return Arc::clone(c);
            }
        }
        let mut write = self.services.write().unwrap();
        write
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(ServiceCounters::default()))
            .clone()
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Snapshot ────────────────────────────────────────────────────────────────

/// Point-in-time snapshot of all metric counters.
///
/// Returned by [`MetricsRegistry::snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Total scraping requests started
    pub requests_total: u64,
    /// Total request failures
    pub errors_total: u64,
    /// Total cache hits
    pub cache_hits_total: u64,
    /// Total cache misses
    pub cache_misses_total: u64,
    /// Total pipeline executions
    pub pipelines_total: u64,
    /// Total pipeline execution errors
    pub pipeline_errors_total: u64,
    /// Total AI input/prompt tokens consumed
    pub input_tokens_total: u64,
    /// Total AI output/completion tokens generated
    pub output_tokens_total: u64,
    /// Current active worker count
    pub active_workers: i64,
    /// Current worker queue depth
    pub queue_depth: i64,
}

impl MetricsSnapshot {
    /// Cache hit rate in the range `[0.0, 1.0]`.  Returns `0.0` when no
    /// cache accesses have been recorded.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::{MetricsRegistry, MetricEvent};
    ///
    /// let r = MetricsRegistry::new();
    /// r.record(MetricEvent::CacheAccess { hit: true });
    /// r.record(MetricEvent::CacheAccess { hit: false });
    /// let snap = r.snapshot();
    /// assert!((snap.cache_hit_rate() - 0.5).abs() < f64::EPSILON);
    /// ```
    #[allow(clippy::cast_precision_loss)]
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits_total + self.cache_misses_total;
        if total == 0 {
            0.0
        } else {
            self.cache_hits_total as f64 / total as f64
        }
    }

    /// Error rate across all scraping requests in the range `[0.0, 1.0]`.
    ///
    /// # Example
    ///
    /// ```
    /// use mycelium_graph::application::metrics::{MetricsRegistry, MetricEvent};
    ///
    /// let r = MetricsRegistry::new();
    /// r.record(MetricEvent::RequestStarted { service: "http".into() });
    /// r.record(MetricEvent::RequestCompleted { service: "http".into(), duration_ms: 100, success: false });
    /// let snap = r.snapshot();
    /// assert!((snap.error_rate() - 1.0).abs() < f64::EPSILON);
    /// ```
    #[allow(clippy::cast_precision_loss)]
    pub fn error_rate(&self) -> f64 {
        if self.requests_total == 0 {
            0.0
        } else {
            self.errors_total as f64 / self.requests_total as f64
        }
    }
}

// ─── Global registry ─────────────────────────────────────────────────────────

/// Process-wide global [`MetricsRegistry`] instance.
///
/// Use this in production code to record metrics without threading a registry
/// through every call.
///
/// # Example
///
/// ```
/// use mycelium_graph::application::metrics::{global_metrics, MetricEvent};
///
/// global_metrics().record(MetricEvent::CacheAccess { hit: true });
/// ```
pub fn global_metrics() -> &'static MetricsRegistry {
    static INSTANCE: LazyLock<MetricsRegistry> = LazyLock::new(MetricsRegistry::new);
    &INSTANCE
}

// ─── Tracing init helper ──────────────────────────────────────────────────────

/// Log output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable coloured output (default)
    #[default]
    Pretty,
    /// Machine-readable JSON (for log aggregation pipelines)
    Json,
    /// Compact single-line text
    Compact,
}

/// Tracing initialisation configuration.
///
/// Initialises `tracing-subscriber` with the specified format and level filter.
/// Typically called once at process startup.
///
/// # Example
///
/// ```no_run
/// use mycelium_graph::application::metrics::{TracingInit, LogFormat};
///
/// TracingInit {
///     format: LogFormat::Json,
///     env_filter: "info".to_string(),
/// }
/// .init();
/// ```
#[derive(Debug, Clone)]
pub struct TracingInit {
    /// Log output format
    pub format: LogFormat,
    /// `RUST_LOG`-style filter (e.g. `"info"`, `"mycelium=debug,info"`)
    pub env_filter: String,
}

impl Default for TracingInit {
    fn default() -> Self {
        Self {
            format: LogFormat::Pretty,
            env_filter: std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        }
    }
}

impl TracingInit {
    /// Initialise the global tracing subscriber.
    ///
    /// Calling this more than once is a no-op (the subscriber is only set
    /// the first time; subsequent calls are silently ignored via `try_init`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use mycelium_graph::application::metrics::{TracingInit, LogFormat};
    ///
    /// TracingInit {
    ///     format: LogFormat::Compact,
    ///     env_filter: "debug".to_string(),
    /// }
    /// .init();
    /// ```
    pub fn init(self) {
        use tracing_subscriber::EnvFilter;

        let filter =
            EnvFilter::try_new(&self.env_filter).unwrap_or_else(|_| EnvFilter::new("info"));

        match self.format {
            LogFormat::Pretty => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_target(true)
                    .pretty()
                    .try_init();
            }
            LogFormat::Json => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_target(true)
                    .json()
                    .try_init();
            }
            LogFormat::Compact => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_target(false)
                    .compact()
                    .try_init();
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn registry() -> MetricsRegistry {
        MetricsRegistry::new()
    }

    #[test]
    fn new_registry_starts_at_zero() {
        let snap = registry().snapshot();
        assert_eq!(snap.requests_total, 0);
        assert_eq!(snap.errors_total, 0);
        assert_eq!(snap.cache_hits_total, 0);
        assert_eq!(snap.pipelines_total, 0);
    }

    #[test]
    fn request_started_increments_counter() {
        let r = registry();
        r.record(MetricEvent::RequestStarted {
            service: "http".into(),
        });
        r.record(MetricEvent::RequestStarted {
            service: "claude".into(),
        });
        assert_eq!(r.snapshot().requests_total, 2);
    }

    #[test]
    fn request_completed_failure_increments_errors() {
        let r = registry();
        r.record(MetricEvent::RequestStarted {
            service: "http".into(),
        });
        r.record(MetricEvent::RequestCompleted {
            service: "http".into(),
            duration_ms: 500,
            success: false,
        });
        let snap = r.snapshot();
        assert_eq!(snap.errors_total, 1);
        assert!((snap.error_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn request_completed_success_does_not_increment_errors() {
        let r = registry();
        r.record(MetricEvent::RequestStarted {
            service: "http".into(),
        });
        r.record(MetricEvent::RequestCompleted {
            service: "http".into(),
            duration_ms: 100,
            success: true,
        });
        assert_eq!(r.snapshot().errors_total, 0);
    }

    #[test]
    fn cache_hit_rate_calculation() {
        let r = registry();
        r.record(MetricEvent::CacheAccess { hit: true });
        r.record(MetricEvent::CacheAccess { hit: true });
        r.record(MetricEvent::CacheAccess { hit: false });
        let snap = r.snapshot();
        assert_eq!(snap.cache_hits_total, 2);
        assert_eq!(snap.cache_misses_total, 1);
        let rate = snap.cache_hit_rate();
        assert!((rate - 2.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn cache_hit_rate_zero_when_no_accesses() {
        let snap = registry().snapshot();
        assert_eq!(snap.cache_hit_rate(), 0.0);
    }

    #[test]
    fn token_usage_accumulates() {
        let r = registry();
        r.record(MetricEvent::TokenUsage {
            provider: "claude".into(),
            input_tokens: 1000,
            output_tokens: 500,
        });
        r.record(MetricEvent::TokenUsage {
            provider: "openai".into(),
            input_tokens: 200,
            output_tokens: 100,
        });
        let snap = r.snapshot();
        assert_eq!(snap.input_tokens_total, 1200);
        assert_eq!(snap.output_tokens_total, 600);
    }

    #[test]
    fn worker_gauge_reflects_changes() {
        let r = registry();
        r.record(MetricEvent::WorkerCountChanged { count: 4 });
        assert_eq!(r.snapshot().active_workers, 4);
        r.record(MetricEvent::WorkerCountChanged { count: 2 });
        assert_eq!(r.snapshot().active_workers, 2);
    }

    #[test]
    fn queue_depth_gauge_reflects_changes() {
        let r = registry();
        r.record(MetricEvent::QueueDepthChanged { depth: 10 });
        assert_eq!(r.snapshot().queue_depth, 10);
    }

    #[test]
    fn pipeline_executed_increments_pipelines_counter() {
        let r = registry();
        r.record(MetricEvent::PipelineExecuted {
            pipeline_id: "test".into(),
            duration_ms: 250,
            success: true,
        });
        assert_eq!(r.snapshot().pipelines_total, 1);
        assert_eq!(r.snapshot().pipeline_errors_total, 0);
    }

    #[test]
    fn pipeline_failure_increments_errors() {
        let r = registry();
        r.record(MetricEvent::PipelineExecuted {
            pipeline_id: "test".into(),
            duration_ms: 100,
            success: false,
        });
        assert_eq!(r.snapshot().pipeline_errors_total, 1);
    }

    #[test]
    fn render_prometheus_contains_required_metric_names() {
        let r = registry();
        r.record(MetricEvent::RequestStarted {
            service: "http".into(),
        });
        let text = r.render_prometheus();
        assert!(text.contains("mycelium_requests_total"));
        assert!(text.contains("mycelium_errors_total"));
        assert!(text.contains("mycelium_cache_hits_total"));
        assert!(text.contains("mycelium_active_workers"));
        assert!(text.contains("mycelium_request_duration_ms_bucket"));
        assert!(text.contains("mycelium_pipeline_duration_ms_bucket"));
    }

    #[test]
    fn tracing_init_default_does_not_panic() {
        // try_init silently ignores "already initialised" errors
        TracingInit::default().init();
    }
}
