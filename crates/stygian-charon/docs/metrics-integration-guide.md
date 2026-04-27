# Metrics Integration Guide

This guide shows how to use `stygian-charon` metrics for monitoring SLO assessment and escalation operations.

## Enabling Metrics

Add the `metrics` feature to your `Cargo.toml`:

```toml
[dependencies]
stygian-charon = { path = "crates/stygian-charon", features = ["metrics"] }
```

## Basic Usage

```rust
use stygian_charon::metrics::MetricsCollector;
use stygian_charon::{investigate_har, infer_requirements_with_target_class, TargetClass};

// Create a metrics collector
let collector = MetricsCollector::new();

// Fetch and investigate a target
let har_content = fetch_har("https://example.com/api")?;
let report = investigate_har(&har_content)?;

// Infer requirements and get target class
let target_class = TargetClass::Api;
let requirements = infer_requirements_with_target_class(&report, target_class);

// Record metrics
let escalation_level = if requirements.requirements.iter().any(|r| r.id == "adaptive_rate_and_retry_budget") {
    "High"
} else {
    "Acceptable"
};

collector.record_assessment(
    report.blocked_requests,
    report.total_requests,
    "Api",
    escalation_level,
);

// Export as Prometheus metrics
println!("{}", collector.to_prometheus());
```

## Prometheus Export Format

Metrics are exported in Prometheus text format for easy integration with monitoring systems:

```
# HELP slo_assessment_total Total SLO assessments performed
# TYPE slo_assessment_total counter
slo_assessment_total 42

# HELP slo_escalation_warning_total SLO escalations in warning zone
# TYPE slo_escalation_warning_total counter
slo_escalation_warning_total 5

# HELP slo_escalation_critical_total SLO escalations in critical zone
# TYPE slo_escalation_critical_total counter
slo_escalation_critical_total 2

# HELP slo_blocked_ratio_min Minimum blocked ratio observed
# TYPE slo_blocked_ratio_min gauge
slo_blocked_ratio_min 0.02

# HELP slo_blocked_ratio_max Maximum blocked ratio observed
# TYPE slo_blocked_ratio_max gauge
slo_blocked_ratio_max 0.75

# HELP slo_blocked_ratio_avg Average blocked ratio
# TYPE slo_blocked_ratio_avg gauge
slo_blocked_ratio_avg 0.15

# HELP slo_target_class_total Assessments by target class
# TYPE slo_target_class_total counter
slo_target_class_total{class="Api"} 20
slo_target_class_total{class="ContentSite"} 12
slo_target_class_total{class="HighSecurity"} 10

# HELP slo_escalation_level_total Assessments by escalation level
# TYPE slo_escalation_level_total counter
slo_escalation_level_total{level="Acceptable"} 35
slo_escalation_level_total{level="Medium"} 5
slo_escalation_level_total{level="High"} 2
```

## Metrics Definitions

### Counter Metrics

- **slo_assessment_total**: Total number of SLO assessments performed
  - Increments once per HAR investigation
  - Use to measure throughput and frequency of assessments

- **slo_escalation_warning_total**: Count of escalations in warning zone
  - Incremented when blocked ratio exceeds acceptable but not critical threshold
  - Indicates increased anti-bot pressure requiring attention

- **slo_escalation_critical_total**: Count of escalations in critical zone
  - Incremented when blocked ratio exceeds critical threshold
  - Indicates severe blocking requiring immediate response

### Gauge Metrics

- **slo_blocked_ratio_min**: Minimum blocked ratio observed
  - Useful for understanding baseline behavior
  - Lower values indicate consistent success

- **slo_blocked_ratio_max**: Maximum blocked ratio observed
  - Useful for identifying worst-case scenarios
  - High values indicate vulnerability to escalated blocking

- **slo_blocked_ratio_avg**: Average blocked ratio across all assessments
  - Trend indicator for overall health
  - Compare across time periods to detect degradation

### Label-Based Metrics

- **slo_target_class_total{class="..."}**: Breakdown by target class
  - `class` label: Api, ContentSite, HighSecurity, Unknown
  - Use to identify which target types are causing escalations

- **slo_escalation_level_total{level="..."}**: Breakdown by escalation level
  - `level` label: Acceptable, Medium, High
  - Understand distribution of assessment outcomes

## Integration with Prometheus

### Scrape Configuration

Add to your Prometheus `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'stygian-charon'
    static_configs:
      - targets: ['localhost:9090']
```

### Recording Rules

Define recording rules to aggregate metrics:

```yaml
groups:
  - name: stygian_rules
    interval: 1m
    rules:
      # Track escalation rate (escalations per minute)
      - record: stygian:escalation_rate:1m
        expr: increase(slo_escalation_critical_total[1m])

      # Track assessment rate (assessments per minute)
      - record: stygian:assessment_rate:1m
        expr: increase(slo_assessment_total[1m])

      # Track escalation percentage
      - record: stygian:escalation_ratio:1m
        expr: slo_escalation_critical_total / slo_assessment_total
```

### Alert Rules

Define alerts for degradation:

```yaml
groups:
  - name: stygian_alerts
    interval: 1m
    rules:
      # Alert if critical escalation rate exceeds 10%
      - alert: HighCriticalEscalationRate
        expr: rate(slo_escalation_critical_total[5m]) / rate(slo_assessment_total[5m]) > 0.1
        for: 5m
        annotations:
          summary: "High critical escalation rate detected"
          description: "{{ $value | humanizePercentage }} of assessments are in critical zone"

      # Alert if max blocked ratio exceeds 60%
      - alert: SevereBlockingDetected
        expr: slo_blocked_ratio_max > 0.6
        for: 10m
        annotations:
          summary: "Severe blocking detected"
          description: "Maximum blocked ratio reached {{ $value | humanizePercentage }}"
```

## Thread Safety

The `MetricsCollector` is thread-safe and can be shared across threads using `Arc`:

```rust
use std::sync::Arc;

let collector = Arc::new(MetricsCollector::new());

// Spawn multiple worker threads
for _ in 0..4 {
    let collector = Arc::clone(&collector);
    std::thread::spawn(move || {
        collector.record_assessment(50, 1000, "Api", "Acceptable");
    });
}
```

## Performance Considerations

- **Zero overhead when disabled**: Without the `metrics` feature, the module is not compiled
- **Lock-free counters**: Assessment counters use atomic operations (no locks)
- **Minimal lock contention**: Distribution maps use locks only during recording
- **Memory efficient**: Only stores aggregate counts, not individual samples

## Examples

### Monitoring Script

```rust
use stygian_charon::metrics::MetricsCollector;

fn main() {
    let collector = MetricsCollector::new();

    // Simulate some assessments
    for i in 0..100 {
        let blocked = (i % 20) as u64;
        let total = 100;
        let target_class = ["Api", "ContentSite", "HighSecurity"][(i as usize) % 3];
        let level = match (blocked / 5) % 3 {
            0 => "Acceptable",
            1 => "Medium",
            _ => "High",
        };

        collector.record_assessment(blocked, total, target_class, level);
    }

    // Print Prometheus metrics
    println!("{}", collector.to_prometheus());
}
```

## Testing with Metrics

```rust
#[test]
fn test_with_metrics() {
    let collector = MetricsCollector::new();

    // Run your assessment code
    let report = investigate_har(har_content)?;
    let requirements = infer_requirements_with_target_class(&report, TargetClass::Api);

    // Record metrics
    collector.record_assessment(
        report.blocked_requests,
        report.total_requests,
        "Api",
        "High",
    );

    // Verify metrics were recorded
    let prometheus = collector.to_prometheus();
    assert!(prometheus.contains("slo_assessment_total 1"));
    assert!(prometheus.contains("slo_escalation_critical_total 1"));
}
```

## Future Enhancements

Potential additions to the metrics framework:

- **Latency histograms**: Measure time to complete assessment
- **Multi-level metrics**: Per-target or per-endpoint breakdowns
- **Export to OpenTelemetry**: Integration with OTEL collectors
- **Statistical aggregations**: Percentile (p50, p95, p99) calculations
- **Time-series storage**: Long-term metric retention
