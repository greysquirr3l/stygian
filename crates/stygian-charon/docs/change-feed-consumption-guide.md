# Change-Feed Consumption Guide

The change-feed module (`crates/stygian-charon/src/change_feed/`)
emits structured incident packets when canary, proxy, and extraction
pipelines agree that an anti-bot vendor has rotated its wall-logic.
This guide explains how to consume the feed, route the events into the
existing observability stack, and act on the recommended mitigation
path.

## Detection flow

```text
T92 IntegrityCanaryReport        ─┐
T84 CanaryTrendObservation       ─┼─▶  ChangeDeltaInput
T86 ProxyScore / ProxyScoreStore ─┤            │
T87 ReliabilityScore             ─┘            ▼
                                            ChangeDetector::detect
                                                  │
                                                  ├── ChangeFeedReport
                                                  │     (per-cycle aggregate)
                                                  │
                                                  └── ChangeEventSink
                                                        ├── InMemoryChangeFeedSink
                                                        │     (always available)
                                                        └── MetricsCollector
                                                              (with `metrics` feature)
```

The detector runs synchronously and consumes a slice of
`ChangeDeltaInput` records. Each delta is converted from its upstream
source at the call site — the change_feed module does not reach into
`stygian-browser` / `stygian-proxy` / `stygian-plugin`.

## Classification

| Band        | Default score range       | Operator action          |
|-------------|---------------------------|--------------------------|
| `Noise`     | `< 0.20`                  | Log only, no event       |
| `Suspected` | `[0.20, 0.55)`            | Advisory event           |
| `Probable`  | `≥ 0.55` or critical tier | Runbook event            |

A single canary delta cannot reach `Probable` on its own — the default
`canary_weight` is `1.00`, so a delta with weight `0.55` reaches the
floor only when paired with another source or marked `Critical` by the
upstream signal.

## Wire format

```json
{
  "aggregate_classification": "probable",
  "aggregate_score": 0.81,
  "noise_targets": ["quiet.example.com"],
  "suspected_targets": ["watch.example.com"],
  "probable_targets": ["hot.example.com"],
  "events": [
    {
      "event_id": "cf-1718616000-hot.example.com",
      "detected_at_unix_secs": 1718616000,
      "affected_target": "hot.example.com",
      "classification": "probable",
      "delta_summary": {
        "headline": "integrity probe webdriver regressed",
        "score": 0.81,
        "sources": ["canary"],
        "severities": ["critical"],
        "highest_severity": "critical"
      },
      "vendor_hint": "datadome",
      "target_class": "high_security",
      "recommended_mitigation_path": {
        "path": "category-a-fingerprint-identity-regression",
        "hint": "apply browser+sticky escalation, refresh fingerprint profile",
        "url": "docs/incident-runbook.md#category-afingerprintidentity-regression"
      },
      "evidence": { "canary.baseline_score": "0.85" }
    }
  ],
  "thresholds": {
    "noise_ceiling": 0.20,
    "probable_floor": 0.55,
    "canary_weight": 1.00,
    "proxy_weight": 0.80,
    "extraction_weight": 0.70
  }
}
```

The `event_id` is a stable composite of
`cf-<detected_at_unix_secs>-<affected_target>` so downstream tooling
can dedupe by ID without trusting insertion order.

## Metrics surface integration

When the `metrics` feature is enabled on `stygian-charon`, the
`MetricsCollector` implements `ChangeEventSink`. Wire the detector
straight into the collector and the change-feed counters appear in
the existing Prometheus export:

```text
# HELP change_feed_events_total Change-feed events emitted per classification band
# TYPE change_feed_events_total counter
change_feed_events_total{classification="noise"} 0
change_feed_events_total{classification="suspected"} 2
change_feed_events_total{classification="probable"} 1

# HELP change_feed_runs_total Change-feed detection cycles executed
# TYPE change_feed_runs_total counter
change_feed_runs_total 14
```

The change-feed block is **only** emitted when at least one counter
is non-zero, so existing dashboards that have not wired the feed in
keep their layout unchanged. Operators that want a permanent counter
should bump `change_feed_runs_total` once per detection cycle via
`MetricsCollector::record_change_feed_run()`.

## Recommended mitigation paths

The `MitigationPath` field on every `ChangeEvent` is the
operator-facing pointer to the runbook section that matches the
classification and (when available) the vendor hint. The mapping
mirrors the categories in `docs/incident-runbook.md`:

| Classification | Vendor hint                | Runbook section                          |
|----------------|----------------------------|------------------------------------------|
| `Suspected`    | any / none                 | `category-a-fingerprint-identity-regression` |
| `Probable`     | `DataDome`                 | `category-a-fingerprint-identity-regression` |
| `Probable`     | `PerimeterX` / `Akamai` / `Imperva` | `category-b-rate-limit-backoff-regression` |
| `Probable`     | `Cloudflare`               | `category-b-rate-limit-backoff-regression` |
| `Probable`     | other / none               | `category-b-rate-limit-backoff-regression` |

The mapping is encoded in `MitigationPath::for_classification` — keep
it in sync with the runbook when either side changes.

## Threshold overrides

`ChangeFeedThresholds` is the configuration surface. Every field has
a public constant (`DEFAULT_NOISE_CEILING`, `DEFAULT_PROBABLE_FLOOR`,
`DEFAULT_CANARY_WEIGHT`, `DEFAULT_PROXY_WEIGHT`,
`DEFAULT_EXTRACTION_WEIGHT`). The struct is `Copy`, serialises
through `serde`, and round-trips through the config layer without
additional plumbing. Out-of-range inputs (NaN, negative, infinite)
fall back to the documented defaults so the classifier cannot be
silently disabled by a bad config.

## Test coverage

| Suite | Count | Coverage |
|-------|-------|----------|
| `crates/stygian-charon/src/change_feed/delta.rs::tests` | 6 | delta sanitisation, serde round-trip, builder methods |
| `crates/stygian-charon/src/change_feed/classification.rs::tests` | 18 | banding, threshold overrides, deterministic output, multi-target cases |
| `crates/stygian-charon/src/change_feed/event.rs::tests` | 10 | event_id stability, mitigation path routing, summary dedup |
| `crates/stygian-charon/tests/change_feed_integration.rs` | 6 (5 + 1 `#[ignore]`) | end-to-end event packet, threshold round-trip, runbook payload shape |

## See also

- `docs/incident-runbook.md` — Category A / B / C runbook sections
- `docs/metrics-integration-guide.md` — Prometheus export format
- `docs/slo-usage-guide.md` — SLO assessment and escalation workflow
- `docs/monitoring/prometheus-scrape-example.yml` — scrape config
- `docs/monitoring/slo-dashboard.json` — starter dashboard
