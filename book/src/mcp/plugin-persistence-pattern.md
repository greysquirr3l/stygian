# Plugin Persistence Pattern

Use this page as a practical pattern for moving data extracted by the browser plugin into the
MCP server, then routing it to downstream systems (data sinks, ingestion APIs, or database
writers).

---

## Positioning

The bundled extension in `crates/stygian-plugin/extension/` is a basic reference implementation.
It is designed to demonstrate:

- interactive template recording
- MCP tool invocation over HTTP
- local template storage plus optional backend sync

Production deployments should typically add:

- user and tenant identity propagation
- signed requests and service-side authorization
- server-side persistence/routing policy
- delivery guarantees (idempotency, retries, dead-letter handling)

---

## End-to-end flow

```text
Browser Extension
  -> plugin_extract_batch (MCP HTTP)
  -> receives structured records
  -> posts records to your ingestion endpoint (or directly to MCP graph pipeline)
  -> graph_pipeline_run routes records to sink adapter
  -> sink adapter persists to backend (DB/API/queue)
```

If you run the full aggregator, prefer namespaced tools:

- `plugin_extract_batch`
- `graph_pipeline_run`

---

## Example A: Extension extraction + MCP pipeline routing

### Step 1: Extract repeated rows/cards in the plugin surface

Call `plugin_extract_batch` from the extension or your backend-for-frontend:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "plugin_extract_batch",
    "arguments": {
      "template_id": "contacts-template-v1",
      "html": "<html>...</html>",
      "url": "https://app.example.com/contacts",
      "root_selector": "tbody > tr"
    }
  }
}
```

### Step 2: Route extracted payloads through graph pipeline execution

Send each extracted record (or a batched array) into a pipeline run request. A common pattern is
an ingestion gateway that receives plugin output, then invokes `graph_pipeline_run`.

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "graph_pipeline_run",
    "arguments": {
      "toml": "[[services]]\nname='scrape-exchange-sink'\nkind='scrape-exchange'\nschema_id='contact-v1'\n\n[[nodes]]\nname='publish-contact'\nservice='scrape-exchange-sink'\n[nodes.params]\nmetadata={ source='plugin', route='contacts' }"
    }
  }
}
```

This keeps persistence concerns in server-side adapters, not in extension code.

---

## Example B: Route to your own database ingestion API

If your primary persistence layer is an internal API that writes to Postgres/MySQL/etc., use this
pattern:

1. Extension calls `plugin_extract_batch` and gets structured records.
2. Extension sends records to your API (for example `/ingest/contacts`).
3. Your API validates schema and auth.
4. Your API forwards to MCP (`graph_pipeline_run`) or writes directly through your own adapter.
5. API returns a receipt (`accepted`, `record_id`, `ingested_at`).

This preserves a clean boundary:

- extension handles capture UX
- MCP/services handle routing and durability

---

## Reliability checklist

For production persistence flows, apply these controls:

- idempotency key per record or per batch
- deterministic record identity (hash or source key)
- bounded retries with backoff
- dead-letter queue for poison payloads
- schema versioning (`contact-v1`, `contact-v2`)
- audit metadata (`source_url`, `captured_at`, `template_id`, `actor`)

---

## Related docs

- [Plugin Tools](./plugin-tools.md)
- [Aggregator (stygian-mcp)](./aggregator.md)
- [Graph Tools](./graph-tools.md)
- [Data Sinks](../graph/data-sinks.md)
- [Extension README](../../crates/stygian-plugin/extension/README.md)
