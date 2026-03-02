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
|---|---|---|
| `timeout` | 30 s | Per-request timeout |
| `user_agent` | `None` | Override `User-Agent` header |
| `follow_redirects` | `true` | Follow 3xx responses |
| `max_redirects` | `10` | Redirect chain limit |
| `proxy` | `None` | HTTP/HTTPS/SOCKS5 proxy URL |

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
|---|---|
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
