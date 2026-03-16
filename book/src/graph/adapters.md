# Built-in Adapters

`stygian-graph` ships adapters for every major scraping and AI workload. All adapters
implement the port traits defined in `src/ports.rs` and are registered by name in the
`ServiceRegistry`.

---

## HTTP Adapter

The default content-fetching adapter. Uses `reqwest` with connection pooling, automatic
redirect following, and configurable retry logic.

```rust
use stygian_graph::adapters::{HttpAdapter, HttpConfig};
use std::time::Duration;

let adapter = HttpAdapter::with_config(HttpConfig {
    timeout:          Duration::from_secs(15),
    user_agent:       Some("stygian/0.1".to_string()),
    follow_redirects: true,
    max_redirects:    10,
    ..Default::default()
});
```

**Registered service name**: `"http"`

| Config field | Default | Description |
| --- | --- | --- |
| `timeout` | 30 s | Per-request timeout |
| `user_agent` | `None` | Override `User-Agent` header |
| `follow_redirects` | `true` | Follow 3xx responses |
| `max_redirects` | `10` | Redirect chain limit |
| `proxy` | `None` | HTTP/HTTPS/SOCKS5 proxy URL |

---

## REST API Adapter

Purpose-built for structured JSON REST APIs. Handles authentication, automatic
multi-strategy pagination, JSON response extraction, and retry — without the caller
needing to manage any of that manually.

```rust
use stygian_graph::adapters::rest_api::{RestApiAdapter, RestApiConfig};
use stygian_graph::ports::{ScrapingService, ServiceInput};
use serde_json::json;
use std::time::Duration;

let adapter = RestApiAdapter::with_config(RestApiConfig {
    timeout:      Duration::from_secs(20),
    max_retries:  3,
    ..Default::default()
});

let input = ServiceInput {
    url: "https://api.github.com/repos/rust-lang/rust/issues".to_string(),
    params: json!({
        "auth":       { "type": "bearer", "token": "${env:GITHUB_TOKEN}" },
        "query":      { "state": "open", "per_page": "100" },
        "pagination": { "strategy": "link_header", "max_pages": 10 },
        "response":   { "data_path": "" }
    }),
};
// let output = adapter.execute(input).await?;
```

**Registered service name**: `"rest-api"`

### Config fields

| Field | Default | Description |
| --- | --- | --- |
| `timeout` | 30 s | Per-request timeout |
| `max_retries` | 3 | Retry attempts on transient errors (`429`, `5xx`, network) |
| `retry_base_delay` | 1 s | Base for exponential backoff |
| `proxy_url` | `None` | HTTP/HTTPS/SOCKS5 proxy URL |

### `ServiceInput.params` contract

| Param | Required | Default | Description |
| --- | --- | --- | --- |
| `method` | — | `"GET"` | `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD` |
| `body` | — | — | JSON body for `POST`/`PUT`/`PATCH` |
| `body_raw` | — | — | Raw string body (takes precedence over `body`) |
| `headers` | — | — | Extra request headers object |
| `query` | — | — | Extra query string parameters object |
| `accept` | — | `"application/json"` | `Accept` header |
| `auth` | — | none | Authentication object (see below) |
| `response.data_path` | — | full body | Dot path into the JSON response to extract |
| `response.collect_as_array` | — | `false` | Force multi-page results into a JSON array |
| `pagination.strategy` | — | `"none"` | `"none"`, `"offset"`, `"cursor"`, `"link_header"` |
| `pagination.max_pages` | — | `1` | Maximum pages to fetch |

### Authentication

```toml
# Bearer token
[nodes.params.auth]
type  = "bearer"
token = "${env:API_TOKEN}"

# HTTP Basic
[nodes.params.auth]
type     = "basic"
username = "${env:API_USER}"
password = "${env:API_PASS}"

# API key in header
[nodes.params.auth]
type   = "api_key_header"
header = "X-Api-Key"
key    = "${env:API_KEY}"

# API key in query string
[nodes.params.auth]
type  = "api_key_query"
param = "api_key"
key   = "${env:API_KEY}"
```

### Pagination strategies

| Strategy | How it works | Best for |
| --- | --- | --- |
| `"none"` | Single request | Simple endpoints |
| `"offset"` | Increments `page_param` from `start_page` | REST APIs with `?page=N` |
| `"cursor"` | Extracts next cursor from `cursor_field` (dot path), sends as `cursor_param` | GraphQL-REST hybrids, Stripe-style |
| `"link_header"` | Follows RFC 8288 `Link: <url>; rel="next"` | GitHub API, GitLab API |

#### Offset example

```toml
[nodes.params.pagination]
strategy        = "offset"
page_param      = "page"
page_size_param = "per_page"
page_size       = 100
start_page      = 1
max_pages       = 20
```

#### Cursor example

```toml
[nodes.params.pagination]
strategy     = "cursor"
cursor_param = "after"
cursor_field = "meta.next_cursor"
max_pages    = 50
```

### Output

`ServiceOutput.data` — pretty-printed JSON string of the extracted data.

`ServiceOutput.metadata`:

```json
{
  "url":        "https://...",
  "page_count": 3
}
```

---

## OpenAPI Adapter

Purpose-built for APIs with an [OpenAPI 3.x](https://spec.openapis.org/oas/v3.0.0)
specification document. At runtime the adapter fetches and caches the spec, resolves the
target operation by `operationId` or `"METHOD /path"` syntax, binds arguments to path/
query/body parameters, and delegates the actual HTTP call to the inner `RestApiAdapter`.

```rust
use stygian_graph::adapters::openapi::{OpenApiAdapter, OpenApiConfig};
use stygian_graph::adapters::rest_api::RestApiConfig;
use stygian_graph::ports::{ScrapingService, ServiceInput};
use serde_json::json;
use std::time::Duration;

let adapter = OpenApiAdapter::with_config(OpenApiConfig {
    rest: RestApiConfig {
        timeout:     Duration::from_secs(20),
        max_retries: 3,
        ..Default::default()
    },
});

let input = ServiceInput {
    url: "https://petstore3.swagger.io/api/v3/openapi.json".to_string(),
    params: json!({
        "operation": "findPetsByStatus",
        "args": { "status": "available" },
        "auth": { "type": "api_key_header", "header": "api_key", "key": "${env:PETSTORE_API_KEY}" },
    }),
};
// let output = adapter.execute(input).await?;
```

**Registered service name**: `"openapi"`

### `ServiceInput` contract

| Field | Required | Description |
| --- | --- | --- |
| `url` | ✅ | URL of the OpenAPI spec document (`.json` or `.yaml`) |
| `params.operation` | ✅ | `operationId` (e.g. `"listPets"`) **or** `"METHOD /path"` (e.g. `"GET /pet/findByStatus"`) |
| `params.args` | — | Key/value map of path, query, and body arguments (all merged; adapter classifies them) |
| `params.auth` | — | Same shape as [REST API auth](#authentication) |
| `params.server.url` | — | Override spec's `servers[0].url` at runtime |
| `params.rate_limit` | — | Proactive rate throttle (see below) |

### Operation resolution

```toml
# By operationId
[nodes.params]
operation = "listPets"

# By HTTP method + path
[nodes.params]
operation = "GET /pet/findByStatus"
```

### Argument binding

The adapter classifies each key in `params.args` against the operation's declared
parameters:

| Parameter location | What happens |
| --- | --- |
| `in: path` | Value is substituted into the URL template (`{petId}` → `42`) |
| `in: query` | Value is appended to the query string |
| `in: body` (requestBody) | All remaining keys are collected into the JSON request body |

```toml
[nodes.params]
operation = "getPetById"

[nodes.params.args]
petId = 42          # path parameter — substituted into /pet/{petId}
```

### Rate limiting

Two independent layers operate simultaneously:

1. **Proactive** — optional `params.rate_limit` block enforced before the HTTP call:

```toml
[nodes.params.rate_limit]
max_requests = 100
window_secs  = 60
strategy     = "token_bucket"  # or "sliding_window" (default)
max_delay_ms = 30000
```

2. **Reactive** — inherited from `RestApiAdapter`: a `429` response with a `Retry-After`
   header causes an automatic sleep-and-retry without any additional configuration.

### Spec caching

The parsed `OpenAPI` document is cached per spec URL for the lifetime of the adapter
instance (or any clone of it — all clones share the same cache `Arc`). The cache is
populated on the first call and reused on every subsequent call, so spec fetching is
a one-time cost per URL.

### Full TOML example

```toml
[[services]]
name = "openapi"
kind = "openapi"

# Unauthenticated list
[[nodes]]
id      = "list-pets"
service = "openapi"
url     = "https://petstore3.swagger.io/api/v3/openapi.json"

[nodes.params]
operation = "findPetsByStatus"

[nodes.params.args]
status = "available"

# API key auth + server override + proactive rate limit
[[nodes]]
id      = "get-pet"
service = "openapi"
url     = "https://petstore3.swagger.io/api/v3/openapi.json"

[nodes.params]
operation = "getPetById"

[nodes.params.args]
petId = 1

[nodes.params.auth]
type   = "api_key_header"
header = "api_key"
key    = "${env:PETSTORE_API_KEY}"

[nodes.params.server]
url = "https://petstore3.swagger.io/api/v3"

[nodes.params.rate_limit]
max_requests = 50
window_secs  = 60
strategy     = "token_bucket"
```

### Output

`ServiceOutput.data` — pretty-printed JSON string returned by the resolved endpoint.

`ServiceOutput.metadata`:

```json
{
  "url":              "https://...",
  "page_count":       1,
  "openapi_spec_url": "https://petstore3.swagger.io/api/v3/openapi.json",
  "operation_id":     "findPetsByStatus",
  "method":           "GET",
  "path_template":    "/pet/findByStatus",
  "server_url":       "https://petstore3.swagger.io/api/v3",
  "resolved_url":     "https://petstore3.swagger.io/api/v3/pet/findByStatus"
}
```

---

## Browser Adapter

Delegates to `stygian-browser` for JavaScript-rendered pages. Requires the `browser`
feature flag and a Chrome binary.

```rust
use stygian_graph::adapters::{BrowserAdapter, BrowserAdapterConfig};
use stygian_browser::StealthLevel;
use std::time::Duration;

let adapter = BrowserAdapter::with_config(BrowserAdapterConfig {
    headless:          true,
    stealth_level:     StealthLevel::Advanced,
    timeout:           Duration::from_secs(30),
    viewport_width:    1920,
    viewport_height:   1080,
    ..Default::default()
});
```

**Registered service name**: `"browser"`

See the [Browser Automation](../browser/overview.md) section for the full feature set.

---

## AI Adapters

All AI adapters implement `AIProvider` and perform structured extraction: they receive raw
HTML (or text) from an upstream scraping node and return a typed JSON object matching the
schema declared in the node config.

### Claude (Anthropic)

```rust
use stygian_graph::adapters::ClaudeAdapter;

let adapter = ClaudeAdapter::new(
    std::env::var("ANTHROPIC_API_KEY")?,
    "claude-3-5-sonnet-20241022",
);
```

**Registered service name**: `"ai_claude"`

| Config field | Description |
| --- | --- |
| `model` | Model ID (e.g. `claude-3-5-sonnet-20241022`) |
| `max_tokens` | Max response tokens (default `4096`) |
| `system_prompt` | Optional system-level instruction |
| `schema` | JSON schema for structured output |

### OpenAI

```rust
use stygian_graph::adapters::OpenAiAdapter;

let adapter = OpenAiAdapter::new(
    std::env::var("OPENAI_API_KEY")?,
    "gpt-4o",
);
```

**Registered service name**: `"ai_openai"`

### Gemini (Google)

```rust
use stygian_graph::adapters::GeminiAdapter;

let adapter = GeminiAdapter::new(
    std::env::var("GOOGLE_API_KEY")?,
    "gemini-2.0-flash",
);
```

**Registered service name**: `"ai_gemini"`

### GitHub Copilot

Uses the Copilot API with your personal access token (PAT) or GitHub App credentials.

```rust
use stygian_graph::adapters::CopilotAdapter;

let adapter = CopilotAdapter::new(
    std::env::var("GITHUB_TOKEN")?,
    "gpt-4o",
);
```

**Registered service name**: `"ai_copilot"`

### Ollama (local)

Run any GGUF model locally without sending data to an external API.

```rust
use stygian_graph::adapters::OllamaAdapter;

let adapter = OllamaAdapter::new(
    "http://localhost:11434",
    "llama3.3",
);
```

**Registered service name**: `"ai_ollama"`

---

## AI fallback chain

Adapters can be wrapped in a fallback chain. If the primary provider fails (rate-limit,
outage), the next in the list is tried:

```rust
use stygian_graph::adapters::AiFallbackChain;

let chain = AiFallbackChain::new(vec![
    Arc::new(ClaudeAdapter::new(api_key.clone(), "claude-3-5-sonnet-20241022")),
    Arc::new(OpenAiAdapter::new(openai_key, "gpt-4o")),
    Arc::new(OllamaAdapter::new("http://localhost:11434", "llama3.3")),
]);
```

**Registered service name**: `"ai_fallback"`

---

## Resilience adapters

Wrap any `ScrapingService` with circuit breaker and retry logic without touching the
underlying implementation:

```rust
use stygian_graph::adapters::resilience::{
    CircuitBreakerImpl, RetryPolicy, ResilientAdapter,
};
use std::time::Duration;

let cb = CircuitBreakerImpl::new(
    5,                           // open after 5 consecutive failures
    Duration::from_secs(120),    // half-open attempt after 2 min
);

let policy = RetryPolicy::exponential(
    3,                           // max 3 attempts
    Duration::from_millis(200),  // initial back-off
    Duration::from_secs(5),      // max back-off
);

let resilient = ResilientAdapter::new(
    Arc::new(http_adapter),
    Arc::new(cb),
    policy,
);
```

---

## Cache adapters

Two in-process cache implementations are included. Both implement `CachePort`.

### `BoundedLruCache`

Thread-safe LRU with a hard capacity limit:

```rust
use stygian_graph::adapters::BoundedLruCache;
use std::num::NonZeroUsize;

let cache = BoundedLruCache::new(NonZeroUsize::new(10_000).unwrap());
```

### `DashMapCache`

Concurrent hash-map backed cache with a background TTL cleanup task:

```rust
use stygian_graph::adapters::DashMapCache;
use std::time::Duration;

let cache = DashMapCache::new(Duration::from_secs(300)); // 5-minute default TTL
```

---

## GraphQL adapter

The `GraphQlService` adapter executes queries against any GraphQL endpoint using
`GraphQlTargetPlugin` implementations registered in a `GraphQlPluginRegistry`.

For most APIs, use `GenericGraphQlPlugin` via the fluent builder rather than writing
a dedicated struct:

```rust
use stygian_graph::adapters::graphql_plugins::generic::GenericGraphQlPlugin;
use stygian_graph::adapters::graphql_throttle::CostThrottleConfig;

let plugin = GenericGraphQlPlugin::builder()
    .name("github")
    .endpoint("https://api.github.com/graphql")
    .bearer_auth("${env:GITHUB_TOKEN}")
    .header("X-Github-Next-Global-ID", "1")
    .cost_throttle(CostThrottleConfig::default())
    .page_size(30)
    .build()
    .expect("name and endpoint required");
```

For runtime-rotating credentials inject an `AuthPort`:

```rust
use std::sync::Arc;
use stygian_graph::adapters::graphql::{GraphQlConfig, GraphQlService};
use stygian_graph::ports::auth::{EnvAuthPort, ErasedAuthPort};

let service = GraphQlService::new(GraphQlConfig::default(), Some(Arc::new(registry)))
    .with_auth_port(Arc::new(EnvAuthPort::new("MY_API_TOKEN")) as Arc<dyn ErasedAuthPort>);
```

See the [GraphQL Plugins](./graphql-plugins.md) page for the full builder reference,
`AuthPort` implementation guide, proactive cost throttling, and custom plugin examples.

---

## Cloudflare Browser Rendering adapter

Submits a multi-page crawl job to the [Cloudflare Browser Rendering API](https://developers.cloudflare.com/browser-rendering/),
polls until it completes, and returns the aggregated content. All page rendering is done
inside Cloudflare's infrastructure — no local Chrome binary needed.

**Feature flag**: `cloudflare-crawl` (not included in `default` or `browser`; add it
explicitly or use `full`).

### Quick start

```toml
# Cargo.toml
[dependencies]
stygian-graph = { version = "0.1", features = ["cloudflare-crawl"] }
```

```rust
use stygian_graph::adapters::cloudflare_crawl::{
    CloudflareCrawlAdapter, CloudflareCrawlConfig,
};
use std::time::Duration;

let adapter = CloudflareCrawlAdapter::with_config(CloudflareCrawlConfig {
    poll_interval: Duration::from_secs(3),
    job_timeout:   Duration::from_secs(120),
    ..Default::default()
});
```

**Registered service name**: `"cloudflare-crawl"`

### `ServiceInput.params` contract

All per-request options are passed via `ServiceInput.params`. `account_id` and
`api_token` are **required**; the rest are optional and forwarded verbatim to the
Cloudflare API.

| Param key | Required | Default | Description |
| --- | --- | --- | --- |
| `account_id` | ✅ | — | Cloudflare account ID |
| `api_token` | ✅ | — | Cloudflare API token with Browser Rendering permission |
| `output_format` | — | `"markdown"` | `"markdown"`, `"html"`, or `"raw"` |
| `max_depth` | — | API default | Maximum crawl depth from the seed URL |
| `max_pages` | — | API default | Maximum pages to crawl |
| `url_pattern` | — | API default | Regex or glob restricting which URLs are followed |
| `modified_since` | — | API default | ISO-8601 timestamp; skip pages not modified since |
| `max_age_seconds` | — | API default | Skip cached pages older than this many seconds |
| `static_mode` | — | `false` | Set `"true"` to skip JS execution (faster, static HTML only) |

### Config fields

| Field | Default | Description |
| --- | --- | --- |
| `poll_interval` | 2 s | How often to poll for job completion |
| `job_timeout` | 5 min | Hard timeout per crawl job; returns `ServiceError::Timeout` if exceeded |

### Output

`ServiceOutput.data` contains the page content of all crawled pages joined by newlines.
`ServiceOutput.metadata` is a JSON object:

```json
{
  "job_id":        "some-uuid",
  "pages_crawled": 12,
  "output_format": "markdown"
}
```

### TOML pipeline usage

```toml
[[nodes]]
id     = "crawl"
type   = "scrape"
target = "https://docs.example.com"

  [nodes.params]
  account_id    = "${env:CF_ACCOUNT_ID}"
  api_token     = "${env:CF_API_TOKEN}"
  output_format = "markdown"
  max_depth     = "3"
  max_pages     = "50"
  url_pattern   = "https://docs.example.com/**"

  [nodes.service]
  name = "cloudflare-crawl"
```

### Error mapping

| Condition | `StygianError` variant |
| --- | --- |
| Missing `account_id` or `api_token` | `ServiceError::Unavailable` |
| Cloudflare API non-2xx | `ServiceError::Unavailable` (with CF error code) |
| Job still pending after `job_timeout` | `ServiceError::Timeout` |
| Unexpected response shape | `ServiceError::InvalidResponse` |
