# GraphQL Plugins

`stygian-graph` ships a generic, builder-based GraphQL plugin system built on top of
the `GraphQlTargetPlugin` port trait. Instead of writing a dedicated struct for each
API you want to query, reach for `GenericGraphQlPlugin`.

---

## GenericGraphQlPlugin

`GenericGraphQlPlugin` implements `GraphQlTargetPlugin` and is configured entirely
via a fluent builder. Only `name` and `endpoint` are required; everything else is
optional with sensible defaults.

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
    .description("GitHub GraphQL API v4")
    .build()
    .expect("name and endpoint are required");
```

### Builder reference

| Method | Required | Description |
|---|---|---|
| `.name(impl Into<String>)` | **yes** | Plugin identifier used in the registry |
| `.endpoint(impl Into<String>)` | **yes** | Full GraphQL endpoint URL |
| `.bearer_auth(impl Into<String>)` | no | Shorthand: sets a `Bearer` auth token |
| `.auth(GraphQlAuth)` | no | Full auth struct (Bearer, API key, or custom header) |
| `.header(key, value)` | no | Add a single request header (repeatable) |
| `.headers(HashMap<String, String>)` | no | Bulk-replace all headers |
| `.cost_throttle(CostThrottleConfig)` | no | Enable proactive point-budget throttling |
| `.page_size(usize)` | no | Default page size for paginated queries (default `50`) |
| `.description(impl Into<String>)` | no | Human-readable description |
| `.build()` | — | Returns `Result<GenericGraphQlPlugin, BuildError>` |

### Auth options

```rust
use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};

// Bearer token (most common)
let plugin = GenericGraphQlPlugin::builder()
    .name("shopify")
    .endpoint("https://my-store.myshopify.com/admin/api/2025-01/graphql.json")
    .bearer_auth("${env:SHOPIFY_ACCESS_TOKEN}")
    .build()
    .unwrap();

// Custom header (e.g. X-Shopify-Access-Token)
let plugin = GenericGraphQlPlugin::builder()
    .name("shopify-legacy")
    .endpoint("https://my-store.myshopify.com/admin/api/2025-01/graphql.json")
    .auth(GraphQlAuth {
        kind:        GraphQlAuthKind::Header,
        token:       "${env:SHOPIFY_ACCESS_TOKEN}".to_string(),
        header_name: Some("X-Shopify-Access-Token".to_string()),
    })
    .build()
    .unwrap();
```

Tokens containing `${env:VAR_NAME}` are expanded by the pipeline template processor
(`expand_template`) or by `GraphQlService::apply_auth`, not by `EnvAuthPort` itself.
`EnvAuthPort` reads the env var value directly — no template syntax is needed when
using it as a runtime auth port.

---

## AuthPort — runtime credential management

For credentials that rotate, expire, or need a refresh flow, implement the
`AuthPort` trait and inject it into `GraphQlService`.

```rust
use stygian_graph::ports::auth::{AuthPort, AuthError, TokenSet};

pub struct MyOAuthPort { /* ... */ }

impl AuthPort for MyOAuthPort {
    async fn load_token(&self) -> Result<Option<TokenSet>, AuthError> {
        // Return None if no cached token exists; Some(token) if you have one.
        Ok(Some(TokenSet {
            access_token:  fetch_stored_token().await?,
            refresh_token: Some(fetch_stored_refresh_token().await?),
            expires_at:    Some(std::time::SystemTime::now()
                               + std::time::Duration::from_secs(3600)),
            scopes:        vec!["read".to_string()],
        }))
    }

    async fn refresh_token(&self) -> Result<TokenSet, AuthError> {
        // Exchange the refresh token for a new access token.
        Ok(TokenSet {
            access_token:  exchange_refresh_token().await?,
            refresh_token: Some(fetch_new_refresh_token().await?),
            expires_at:    Some(std::time::SystemTime::now()
                               + std::time::Duration::from_secs(3600)),
            scopes:        vec!["read".to_string()],
        })
    }
}
```

### Wiring into GraphQlService

```rust
use std::sync::Arc;
use stygian_graph::adapters::graphql::{GraphQlConfig, GraphQlService};
use stygian_graph::ports::auth::ErasedAuthPort;

let service = GraphQlService::new(GraphQlConfig::default(), None)
    .with_auth_port(Arc::new(MyOAuthPort { /* ... */ }) as Arc<dyn ErasedAuthPort>);
```

The service calls `resolve_token` before each request. If the token is expired (or
within 60 seconds of expiry), `refresh_token` is called automatically.

### EnvAuthPort — zero-config static token

For non-rotating tokens, `EnvAuthPort` reads a bearer token from an environment
variable at load time:

```rust
use stygian_graph::ports::auth::EnvAuthPort;

let auth = EnvAuthPort::new("GITHUB_TOKEN");
```

If `GITHUB_TOKEN` is not set, `EnvAuthPort::load_token` returns `Ok(None)`. An error
(`AuthError::TokenNotFound`) is only raised later when `resolve_token` is called and
finds no token available.

---

## Cost throttling

GraphQL APIs that expose `extensions.cost.throttleStatus` (Shopify Admin API,
Jobber, and others) can be configured for proactive point-budget management.

### CostThrottleConfig

```rust
use stygian_graph::ports::graphql_plugin::CostThrottleConfig;

let config = CostThrottleConfig {
    max_points:      10_000.0,  // bucket capacity
    restore_per_sec: 500.0,     // points restored per second
    min_available:   50.0,      // don't send if fewer points remain
    max_delay_ms:    30_000,    // wait at most 30 s before giving up
};
```

| Field | Default | Description |
|---|---|---|
| `max_points` | `10_000.0` | Total bucket capacity |
| `restore_per_sec` | `500.0` | Points/second restored |
| `min_available` | `50.0` | Points threshold below which we pre-sleep |
| `max_delay_ms` | `30_000` | Hard ceiling on proactive sleep duration (ms) |

Attach config to a plugin via `.cost_throttle(config)` on the builder, or override
`GraphQlTargetPlugin::cost_throttle_config()` on a custom plugin implementation.

### How budget tracking works

1. **Pre-flight**: `pre_flight_delay` inspects the current `LiveBudget` for the
   plugin. If the projected available points fall below `min_available` it sleeps
   for the exact duration needed to restore enough points, up to `max_delay_ms`.
2. **Post-response**: `update_budget` parses `extensions.cost.throttleStatus` out
   of the response JSON and updates the per-plugin `LiveBudget` accordingly.
3. **Reactive back-off**: If a request is throttled anyway (HTTP 429 or
   `extensions.cost` signals exhaustion), `reactive_backoff_ms` computes an
   exponential delay.

The budgets are stored in a `HashMap<String, PluginBudget>` keyed by plugin name
and protected by a `tokio::sync::RwLock`, so all concurrent requests share the
same view of remaining points.

---

## Writing a custom plugin

For complex APIs — multi-tenant endpoints, per-request header mutations, non-standard
auth flows — implement `GraphQlTargetPlugin` directly:

```rust
use std::collections::HashMap;
use stygian_graph::ports::{GraphQlAuth, GraphQlAuthKind};
use stygian_graph::ports::graphql_plugin::{CostThrottleConfig, GraphQlTargetPlugin};

pub struct AcmeApi {
    token: String,
}

impl GraphQlTargetPlugin for AcmeApi {
    fn name(&self) -> &str { "acme" }
    fn endpoint(&self) -> &str { "https://api.acme.io/graphql" }

    fn version_headers(&self) -> HashMap<String, String> {
        [("Acme-Api-Version".to_string(), "2025-01".to_string())]
            .into_iter()
            .collect()
    }

    fn default_auth(&self) -> Option<GraphQlAuth> {
        Some(GraphQlAuth {
            kind:        GraphQlAuthKind::Bearer,
            token:       self.token.clone(),
            header_name: None,
        })
    }

    fn default_page_size(&self) -> usize { 25 }
    fn description(&self) -> &str { "Acme Corp GraphQL API" }
    fn supports_cursor_pagination(&self) -> bool { true }

    // opt-in to proactive throttling
    fn cost_throttle_config(&self) -> Option<CostThrottleConfig> {
        Some(CostThrottleConfig::default())
    }
}
```

Register it the same way as any built-in plugin:

```rust
use stygian_graph::adapters::graphql::GraphQlPluginRegistry;

let mut registry = GraphQlPluginRegistry::new();
registry.register(Arc::new(AcmeApi { token: /* ... */ }));
```
