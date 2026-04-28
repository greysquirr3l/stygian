# Getting Started

---

## Install

```toml
[dependencies]
stygian-charon = "*"
```

Enable optional features when needed:

```toml
stygian-charon = { version = "*", features = ["metrics", "caching"] }
```

---

## Quick start: investigate HAR and build policy

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
                    "content": {
                        "size": 0,
                        "mimeType": "text/html",
                        "text": "captcha-delivery.com"
                    },
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
    println!("risk score: {}", policy.risk_score);
    println!("recommended acquisition mode: {:?}", acquisition.mode);

    Ok(())
}
```

---

## Quick start: classify one transaction

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

## Example commands

```bash
cargo run -p stygian-charon --example recon
cargo run -p stygian-charon --example cache_benchmark --features caching
cargo run -p stygian-charon --example live_slo_validator --features live-validation -- --help
```
