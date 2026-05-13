# stygian-plugin

A Chrome browser plugin fallback scraper for Stygian, providing flexible and interactive visual data extraction as a fallback when stygian-graph and stygian-browser cannot scrape a page.

## Features

- **Template-based extraction**: Define a schema once, apply to multiple elements
- **Recording-based**: User clicks/highlights → generates extraction pattern
- **Query-driven**: CSS and XPath selectors with fallback support
- **Region-based**: Multiple independent zones, each with custom rules
- **Multi-instance extraction**: Iterate over matching elements on a page
- **Transformation pipeline**: Trim, normalize, regex, type coercion, HTML stripping, etc.
- **Idempotent operations**: ULID-based deduplication for safe retries
- **Integrated with stygian-graph**: Implements `ScrapingService` trait for pipeline integration

## Architecture

Following Stygian's hexagonal architecture:

- **Domain** (`src/domain/`): Pure Rust, zero I/O dependencies
- **Ports** (`src/ports.rs`): Trait definitions (PluginTemplateStore, PluginExtractionPort, IdempotencyKeyStore)
- **Adapters** (`src/adapters/`): Concrete implementations
  - ExtractionEngine: CSS selector-based DOM extraction
  - PluginExtractionAdapter: Bridges to stygian-graph's ScrapingService
- **Storage** (`src/storage/`): Persistence adapters
  - FileTemplateStore: JSON file-based template storage
  - MemoryIdempotencyStore: In-memory result caching

## Quick Start

### Creating a Template

```rust
use stygian_plugin::{
    domain::{ExtractionTemplate, Region, Selector, Transformation},
    adapters::ExtractionEngine,
    ports::PluginExtractionPort,
};
use serde_json::json;

// Define a template
let template = ExtractionTemplate::new("Product")
    .with_description("Extract product info from a listing")
    .with_region(
        Region::new(
            "name",
            Selector::css("h2.product-title"),
            json!({"type": "string"}),
        )
        .with_transformation(Transformation::Trim)
    )
    .with_region(
        Region::new(
            "price",
            Selector::css(".product-price"),
            json!({"type": "string"}),
        )
        .with_transformation(Transformation::Regex {
            pattern: r"\\$([0-9.]+)".to_string(),
            replacement: "$1".to_string(),
        })
    );
```

### Executing Extraction

```rust
use stygian_plugin::adapters::ExtractionEngine;
use stygian_plugin::domain::ExtractionRequest;

let html = r#"<html><h2 class="product-title">Widget</h2><span class="product-price">$99.99</span></html>"#;

let request = ExtractionRequest::new(template, "https://example.com", html);
let result = ExtractionEngine::execute(&request)?;

println!("Extracted: {:?}", result.data);
```

### Using with stygian-graph

Register the adapter in your service registry:

```rust
use stygian_plugin::adapters::PluginExtractionAdapter;
use stygian_plugin::storage::{FileTemplateStore, MemoryIdempotencyStore};
use std::sync::Arc;

let adapter = Arc::new(PluginExtractionAdapter::new(
    Arc::new(FileTemplateStore::new("./templates".into())),
    Arc::new(ExtractionEngine),
    Arc::new(MemoryIdempotencyStore::new()),
));

registry.register("plugin", adapter).await?;
```

Then use in a pipeline:

```toml
[[nodes]]
name = "extract-products"
kind = "plugin"
params = { template_id = "uuid-of-template", timeout_ms = 30000 }
```

## Selectors

### CSS Selectors

```rust
Selector::css(".product-card")
```

### XPath Selectors

```rust
Selector::xpath("//div[@class='product']")
```

### Dual Selectors (Recommended)

```rust
Selector::dual(".product", "//div[@class='product']")
```

The engine tries CSS first (faster), then falls back to XPath if no matches.

## Transformations

Transformations are applied in order:

- `Trim`: Remove leading/trailing whitespace
- `Lowercase` / `Uppercase`: Case conversion
- `RemoveWhitespace`: Strip all whitespace
- `NormalizeWhitespace`: Collapse multiple spaces to single space
- `StripHtml`: Remove HTML tags
- `DecodeHtml`: Decode HTML entities
- `Regex { pattern, replacement }`: Regex find-and-replace
- `RegexExtract { pattern, group }`: Extract specific capture group
- `Coerce { target_type }`: Convert to "string", "number", "boolean", "date"
- `Filter { pattern }`: Only include if matches regex
- `ParseJson`: Parse as JSON

Example:

```rust
Region::new("price", selector, schema)
    .with_transformation(Transformation::StripHtml)
    .with_transformation(Transformation::Trim)
    .with_transformation(Transformation::Regex {
        pattern: r"\\$(\\d+\\.\\d{2})".to_string(),
        replacement: "$1".to_string(),
    })
    .with_transformation(Transformation::Coerce {
        target_type: "number".to_string(),
    })
```

## Idempotency

Each extraction request can include an idempotency key:

```rust
let request = ExtractionRequest::new(template, url, html)
    .with_idempotency_key(idempotency_key);
```

If the same key is used again, the cached result is returned (safe for retries).

## Storage

### Templates

```rust
let store = FileTemplateStore::new("./templates".into());
store.save(&template).await?;
let retrieved = store.get(&template.id).await?;
let all = store.list().await?;
store.delete(&template.id).await?;
```

### Idempotency

```rust
let store = MemoryIdempotencyStore::new();
store.store_result(&key, &result).await?;
if let Some(cached) = store.get_result(&key).await? {
    // Use cached result
}
```

## Testing

Run tests:

```bash
cargo test -p stygian-plugin
```

Run examples:

```bash
cargo run --example basic_extraction -p stygian-plugin
```

## Next Steps

- **Phase 3**: MCP tool integration (plugin_apply_template, plugin_record_*, etc.)
- **Phase 4**: Chrome extension (TypeScript, content script, service worker, UI)
- **Phase 5**: CircuitBreaker fallback routing from stygian-graph
- **Phase 6**: Full integration tests, CI/CD, documentation

## License

AGPL-3.0-only OR LicenseRef-Commercial
