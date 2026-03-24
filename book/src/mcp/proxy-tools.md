# Proxy MCP Tools

`stygian-proxy` exposes six tools for managing a proxy pool plus a readable pool-stats resource.

---

## Enabling

```toml
[dependencies]
stygian-proxy = { version = "0.4", features = ["mcp"] }
```

The proxy MCP server is primarily designed to be used through the [aggregator](./aggregator.md),
which layers it with graph and browser tools. When running standalone, start it directly from
your crate; there is no dedicated binary target for the proxy server alone.

---

## Handle lifecycle

Proxy handles are tracked across tool calls by a `handle_token` (a ULID string):

```
proxy_add → proxy_acquire / proxy_acquire_for_domain → [use proxy] → proxy_release
```

The handle acts as a circuit-breaker ticket. **Always call `proxy_release`** — dropping the
token without releasing counts the request as a failure toward the proxy's circuit breaker.

---

## Tools

### `proxy_add`

Register a new proxy in the pool.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `url` | string | ✓ | Proxy URL — e.g. `http://proxy:8080` or `socks5://user:pass@host:1080` |
| `proxy_type` | string | | `Http` \| `Https` \| `Socks4` \| `Socks5` (default: inferred from URL scheme) |
| `username` | string | | Proxy username |
| `password` | string | | Proxy password |
| `weight` | integer | | Selection weight for weighted rotation (default: 1) |
| `tags` | array | | String tags for grouping — e.g. `["us-east", "datacenter"]` |

**Returns:**

```json
{ "proxy_id": "550e8400-e29b-41d4-a716-446655440000" }
```

---

### `proxy_remove`

Remove a proxy from the pool by its UUID.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `proxy_id` | string | ✓ | UUID returned by `proxy_add` |

**Returns:**

```json
{ "removed": true }
```

---

### `proxy_pool_stats`

Return current pool statistics. No parameters required.

**Returns:**

```json
{
  "total":           10,
  "healthy":          8,
  "open":             2,
  "active_sessions":  3
}
```

| Field | Description |
| ----- | ----------- |
| `total` | Total number of registered proxies |
| `healthy` | Proxies with circuit breaker in `Closed` state |
| `open` | Proxies with circuit breaker in `Open` state (cooling down) |
| `active_sessions` | Handles currently acquired and not yet released |

---

### `proxy_acquire`

Acquire a proxy handle using the pool's configured rotation strategy (default: round-robin).

No parameters required.

**Returns:**

```json
{
  "handle_token": "<acquire-token>",
  "proxy_url":    "http://proxy1.example.com:8080"
}
```

Use `proxy_url` to route your HTTP or browser request. Pass `handle_token` to `proxy_release`
when done.

---

### `proxy_acquire_for_domain`

Acquire a proxy with sticky session semantics — the same proxy is returned for subsequent
calls with the same domain while the session TTL has not expired.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `domain` | string | ✓ | Domain name — e.g. `example.com` |

**Returns:** Same as `proxy_acquire`:

```json
{
  "handle_token": "01HV4...",
  "proxy_url":    "http://proxy2.example.com:8080"
}
```

**Use case:** Authenticated scraping sessions where the target site associates your session
cookie with a specific IP. Using sticky sessions ensures login cookies and subsequent requests
all go through the same proxy IP.

---

### `proxy_release`

Release a previously acquired handle. Informs the circuit breaker whether the request
succeeded or failed.

| Parameter | Type | Required | Description |
| --------- | ---- | -------- | ----------- |
| `handle_token` | string | ✓ | Token returned by `proxy_acquire` or `proxy_acquire_for_domain` |
| `success` | boolean | | Whether the request succeeded (default: `true`) — failures increment the circuit-breaker failure counter |

**Returns:**

```json
{ "released": true }
```

---

## Resources

The proxy MCP exposes pool statistics as an MCP resource:

| URI | MIME type | Description |
| --- | --------- | ----------- |
| `proxy://pool/stats` | `application/json` | Current `PoolStats` — same as `proxy_pool_stats` |

**Example `resources/read` request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "resources/read",
  "params": { "uri": "proxy://pool/stats" }
}
```

---

## Circuit breaker behaviour

Each proxy has an independent lock-free circuit breaker with three states:

```
Closed (healthy)
  │  failure threshold exceeded
  ▼
Open (cool-down)
  │  cooldown period elapsed
  ▼
HalfOpen (probe)
  │  probe succeeds          probe fails
  ▼                               │
Closed                          Open ◄──┘
```

Releasing a handle with `"success": false` increments the failure counter. Once the
threshold is reached the proxy enters `Open` state and is excluded from `proxy_acquire` until
the cooldown period passes.
