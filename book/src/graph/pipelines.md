# Building Pipelines

A mycelium pipeline is a **directed acyclic graph (DAG)** of service nodes. Pipelines can be
defined in JSON, TOML, or built programmatically in Rust.

---

## JSON pipeline format

```json
{
  "id": "product-scraper",
  "nodes": [
    {
      "id": "fetch_html",
      "service": "http",
      "config": {
        "timeout_ms": 10000,
        "user_agent": "Mozilla/5.0 (compatible; mycelium/0.1)"
      }
    },
    {
      "id": "render_js",
      "service": "browser",
      "config": {
        "wait_strategy": "network_idle",
        "stealth_level": "advanced"
      }
    },
    {
      "id": "extract_data",
      "service": "ai_claude",
      "config": {
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 2048,
        "schema": {
          "title":        "string",
          "price":        "number",
          "availability": "boolean",
          "images":       ["string"]
        }
      }
    }
  ],
  "edges": [
    {"from": "fetch_html",  "to": "render_js"},
    {"from": "render_js",   "to": "extract_data"}
  ]
}
```

### Field reference

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | no | Human-readable pipeline identifier |
| `nodes[].id` | string | yes | Unique node identifier within the pipeline |
| `nodes[].service` | string | yes | Registered service name (must exist in `ServiceRegistry`) |
| `nodes[].config` | object | no | Service-specific configuration (passed as `ServiceInput.config`) |
| `edges[].from` | string | yes | Source node `id` |
| `edges[].to` | string | yes | Target node `id` |

---

## TOML pipeline format

The same structure works in TOML:

```toml
id = "product-scraper"

[[nodes]]
id      = "fetch_html"
service = "http"
[nodes.config]
timeout_ms = 10000

[[nodes]]
id      = "extract_data"
service = "ai_claude"
[nodes.config]
model      = "claude-3-5-sonnet-20241022"
max_tokens = 2048

[[edges]]
from = "fetch_html"
to   = "extract_data"
```

---

## Programmatic builder

For pipelines constructed at runtime:

```rust
use mycelium_graph::domain::pipeline::PipelineUnvalidated;
use mycelium_graph::domain::graph::{Node, Edge};
use serde_json::json;

let pipeline = PipelineUnvalidated::builder()
    .id("product-scraper")
    .node(Node { id: "fetch".into(), service: "http".into(), config: json!({}) })
    .node(Node { id: "extract".into(), service: "ai_claude".into(), config: json!({
        "model": "claude-3-5-sonnet-20241022"
    })})
    .edge(Edge { from: "fetch".into(), to: "extract".into() })
    .build()?;

let validated = pipeline.validate()?;
let results   = validated.execute().await?;
```

---

## Pipeline validation

`validate()` runs four checks before the pipeline may execute:

1. **Node uniqueness** — all `node.id` values are distinct.
2. **Edge validity** — every edge references nodes that exist.
3. **Cycle detection** — Kahn's topological sort; fails if a cycle is detected.
4. **Connectivity** — all nodes are reachable from at least one source node.

Any validation failure returns a typed `PipelineError` — never panics.

```rust
use mycelium_graph::domain::error::PipelineError;

match pipeline.validate() {
    Ok(validated)                     => { /* proceed */ }
    Err(PipelineError::DuplicateNode(id)) => eprintln!("duplicate node: {id}"),
    Err(PipelineError::CycleDetected)    => eprintln!("pipeline has a cycle"),
    Err(PipelineError::UnknownEdge { from, to }) =>
        eprintln!("edge {from} → {to} references unknown nodes"),
    Err(e) => eprintln!("validation failed: {e}"),
}
```

---

## Idempotency

Every execution is assigned an `IdempotencyKey` — a ULID that acts as a deduplication
token across retries:

```rust
use mycelium_graph::domain::idempotency::IdempotencyKey;

// Auto-generated (recommended)
let key = IdempotencyKey::new();

// Deterministic from a stable input (replays return the same result)
let key = IdempotencyKey::from_input("pipeline-1", "https://example.com/product/123");
```

Pass the key to `execute_idempotent()`. If the same key is seen again within the TTL,
the cached result is returned immediately without re-executing.

---

## Branching and fan-out

A node can have multiple outgoing edges. All downstream nodes receive the same output:

```json
{
  "nodes": [
    {"id": "fetch",    "service": "http"},
    {"id": "store_raw","service": "s3"},
    {"id": "extract",  "service": "ai_claude"}
  ],
  "edges": [
    {"from": "fetch", "to": "store_raw"},
    {"from": "fetch", "to": "extract"}
  ]
}
```

`store_raw` and `extract` run **concurrently** in the same wave.

---

## Conditional execution

Nodes support an optional `condition` field (JSONPath expression):

```json
{
  "id": "render_js",
  "service": "browser",
  "condition": "$.content_type == 'application/javascript'"
}
```

When the condition evaluates to `false` the node is skipped and its outputs are forwarded
as empty, allowing downstream nodes to handle the gap gracefully.
