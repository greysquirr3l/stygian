# stygian-charon

Anti-bot diagnostics, HAR forensics, SLO assessment, and runtime-policy planning for the Stygian ecosystem.

[![Crates.io](https://img.shields.io/crates/v/stygian-charon.svg)](https://crates.io/crates/stygian-charon)
[![Documentation](https://img.shields.io/badge/docs-docs.rs-blue)](https://docs.rs/stygian-charon)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](../../LICENSE)

---

## What It Does

`stygian-charon` turns raw HTTP transaction evidence into actionable acquisition guidance.

Core capabilities:

- classify likely anti-bot providers from transaction signals or HAR payloads
- investigate HAR files into normalized reports
- infer requirement profiles by target class (`Api`, `ContentSite`, `HighSecurity`)
- build runtime policy recommendations from observed blocking behavior
- map runtime policies into acquisition hints for higher-level runners
- optionally cache repeated investigations
- optionally expose metrics for assessment and escalation flows
- optionally tune SLOs from historical observations

---

## Installation

```toml
[dependencies]
stygian-charon = "*"
```

Enable optional features as needed:

```toml
stygian-charon = { version = "*", features = ["metrics", "caching"] }
```

### Feature Reference

| Feature | Purpose |
| ------- | ------- |
| `metrics` | Collect telemetry counters and blocked-ratio aggregates |
| `live-validation` | Enable the live validator example for URL-backed HAR capture |
| `caching` | Enable in-memory investigation caching APIs |
| `redis-cache` | Enable Redis-backed caching on top of `caching` |

---

## Quick Start

### Investigate a HAR and Build a Runtime Policy

```rust
use stygian_charon::{
    TargetClass,
    build_runtime_policy,
    infer_requirements_with_target_class,
    investigate_har,
    map_runtime_policy,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let har = r#"{
        "log": {
            "version": "1.2",
            "creator": {"name": "example", "version": "1.0"},
            "pages": [{
                "id": "page_1",
                "title": "https://example.com",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "pageTimings": {"onLoad": 0}
            }],
            "entries": [{
                "pageref": "page_1",
                "startedDateTime": "2026-01-01T00:00:00.000Z",
                "time": 0,
                "request": {
                    "method": "GET",
                    "url": "https://example.com",
                    "httpVersion": "HTTP/2",
                    "headers": [],
                    "queryString": [],
                    "cookies": [],
                    "headersSize": -1,
                    "bodySize": 0
                },
                "response": {
                    "status": 403,
                    "statusText": "Forbidden",
                    "httpVersion": "HTTP/2",
                    "headers": [],
                    "cookies": [],
                    "content": {"size": 0, "mimeType": "text/html", "text": "captcha-delivery.com"},
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": 0
                },
                "cache": {},
                "timings": {
                    "blocked": 0,
                    "dns": 0,
                    "connect": 0,
                    "send": 0,
                    "wait": 0,
                    "receive": 0,
                    "ssl": 0
                }
            }]
        }
    }"#;

    let report = investigate_har(har)?;
    let requirements = infer_requirements_with_target_class(&report, TargetClass::ContentSite);
    let policy = build_runtime_policy(&report, &requirements);
    let acquisition = map_runtime_policy(&policy);

    println!("provider: {:?}", report.aggregate.provider);
    println!("risk_score: {}", policy.risk_score);
    println!("recommended acquisition mode: {:?}", acquisition.mode);

    Ok(())
}
```

### Classify a Single Transaction View

```rust
use stygian_charon::{TransactionView, classify_transaction};
use std::collections::BTreeMap;

fn main() {
    let transaction = TransactionView {
        url: "https://example.com".to_string(),
        host: "example.com".to_string(),
        status: 403,
        resource_type: Some("document".to_string()),
        response_headers: BTreeMap::new(),
        response_body_excerpt: Some("captcha-delivery.com".to_string()),
    };

    let detection = classify_transaction(&transaction);
    println!("provider: {:?}", detection.provider);
}
```

---

## Public API Areas

- classification: `classify_transaction`, `classify_har`
- HAR investigation: `investigate_har`, `compare_reports`
- SLO and requirement inference: `infer_requirements`, `infer_requirements_with_target_class`
- policy planning: `build_runtime_policy`, `analyze_and_plan`, `plan_from_report`
- acquisition mapping: `map_runtime_policy`, `map_policy_hints`, `map_adapter_strategy`
- adaptive thresholds: `AdaptiveSloPolicy`, `RegressionHistoryPolicy`
- snapshot compatibility and drift analysis: snapshot normalization, validation, and drift helpers

---

## Examples

Run included examples from the crate directory or workspace root:

```bash
cargo run -p stygian-charon --example recon
cargo run -p stygian-charon --example cache_benchmark --features caching
cargo run -p stygian-charon --example live_slo_validator --features live-validation -- --help
```

---

## Documentation

Additional user-facing guides shipped with the crate:

- `docs/slo-usage-guide.md`
- `docs/caching-integration-guide.md`
- `docs/metrics-integration-guide.md`
- `docs/output-structure.md`
- `docs/signal-coverage-matrix.md`

---

## License

Licensed under:

- AGPL-3.0-only
- or the commercial Stygian license
