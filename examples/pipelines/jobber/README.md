# Jobber GraphQL Pipelines

Ready-to-run Mycelium pipelines for the [Jobber](https://d.getjobber.com/docs/api/) field-service management GraphQL API.

## Requirements

### Environment variables

| Variable | Required | Purpose |
| --- | --- | --- |
| `JOBBER_ACCESS_TOKEN` | Yes (simple auth) | Pre-obtained OAuth2 access token |
| `JOBBER_CLIENT_ID` | For full OAuth2 PKCE | App client ID from developer portal |
| `JOBBER_CLIENT_SECRET` | For full OAuth2 PKCE | App client secret |

### Obtaining a Jobber access token

1. Sign in to the [Jobber Developer Portal](https://developer.getjobber.com/)
2. Create an application under **Apps** → **Create App**
3. Note your **Client ID** and **Client Secret**
4. Obtain an access token via the OAuth2 PKCE flow (see reference implementation in `reference_materials/oauth/`)

OAuth2 endpoints:

```text
Authorization: https://api.getjobber.com/api/oauth/authorize
Token:         https://api.getjobber.com/api/oauth/token
```

### API version header

All requests require:

```text
X-JOBBER-GRAPHQL-VERSION: 2025-04-16
```

This header is **automatically injected** by the `JobberPlugin` — you do not need to add it to your TOML files.

## Running pipelines

```bash
# Export your token
export JOBBER_ACCESS_TOKEN="your-token-here"

# Validate a pipeline without executing
mycelium check examples/pipelines/jobber/clients.toml

# Fetch all clients
mycelium run examples/pipelines/jobber/clients.toml

# Fetch all jobs
mycelium run examples/pipelines/jobber/jobs.toml

# Fetch invoices with line items
mycelium run examples/pipelines/jobber/invoices.toml

# Fetch quotes
mycelium run examples/pipelines/jobber/quotes.toml

# Fetch expenses
mycelium run examples/pipelines/jobber/expenses.toml

# Fetch visits
mycelium run examples/pipelines/jobber/visits.toml

# Full sync with AI normalisation (requires ANTHROPIC_API_KEY)
export ANTHROPIC_API_KEY="sk-ant-..."
mycelium run examples/pipelines/jobber/full_sync.toml

# Introspect the Jobber schema (writes to ~/.mycelium/cache/jobber_schema.json)
mycelium run examples/pipelines/jobber/introspect.toml
```

## Pipeline structure

All pipelines follow the same pattern:

```toml
[[services]]
name   = "jobber"
kind   = "graphql"
plugin = "jobber"   # Resolves JobberPlugin: endpoint, auth, version header

[[nodes]]
name    = "fetch_clients"
service = "jobber"

[nodes.params]
query = "..."

[nodes.params.pagination]
strategy       = "cursor"
page_info_path = "data.clients.pageInfo"
edges_path     = "data.clients.edges"
```

The `plugin = "jobber"` declaration injects:
- Endpoint: `https://api.getjobber.com/api/graphql`
- Version header: `X-JOBBER-GRAPHQL-VERSION: 2025-04-16`
- Auth: Bearer token from `JOBBER_ACCESS_TOKEN`

Override any default by adding the field explicitly in the TOML — explicit params always win.

## Expected output

Each node emits `ServiceOutput` where:
- `data` — JSON string containing the paginated response body
- `metadata` — execution metadata including:
  - `status_code` — HTTP status (should be 200)
  - `cost` — Jobber API cost points consumed by this request
  - `response_time_ms` — request duration

Cost metadata example (from Jobber's rate-limit headers):

```json
{
  "status_code": 200,
  "cost": {
    "requested": 200,
    "actual": 150,
    "throttle_status": { "currently_available": 9850, "restore_rate": 500 }
  }
}
```

## Canonical output schemas

The `schemas/` directory contains JSON Schema definitions for each domain object.
These are used by the AI normalisation nodes in `full_sync.toml`:

| Schema file | Root type |
| --- | --- |
| `schemas/client.schema.json` | `Client` |
| `schemas/job.schema.json` | `Job` |
| `schemas/invoice.schema.json` | `Invoice` |
| `schemas/quote.schema.json` | `Quote` |
| `schemas/expense.schema.json` | `Expense` |
| `schemas/visit.schema.json` | `Visit` |

## Extending pipelines

### Adding custom fields

Edit the `query` in the TOML to add or remove GraphQL fields:

```toml
[nodes.params]
query = """
query ListClients($first: Int, $after: String) {
  clients(first: $first, after: $after) {
    edges {
      node {
        id
        name
        # Add your custom fields here:
        companyName
        tags { label }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}
"""
```

### Filtering by date range

Use GraphQL variables in the `params` section:

```toml
[nodes.params]
variables = { filter = { createdAt = { gte = "2025-01-01", lte = "2025-12-31" } } }
```

### Adding a new plugin target

To add a new GraphQL target (e.g. Shopify, GitHub, Linear):

1. Create `crates/mycelium-graph/src/adapters/graphql_plugins/<target>.rs` implementing `GraphQlTargetPlugin`
2. Add `pub mod <target>;` to `adapters/graphql_plugins/mod.rs`
3. Register the plugin at startup
4. Reference it with `plugin = "<target>"` in TOML

No changes to `GraphQlService`, the port, or the registry are required.

## Rate limits

Jobber rate limits requests using a cost-based throttle:
- **Max points**: 10,000 per rolling window
- **Restore rate**: 500 points/second

The `GraphQlService` adapter automatically detects `THROTTLED` errors and retries with exponential back-off.
Monitor cost consumption via the `metadata.cost` field in `ServiceOutput`.

## Reference materials

- Go OAuth implementation: `reference_materials/oauth/`
- GraphQL rate-limit config: `reference_materials/graphql/ratelimit_config.go`
- Jobber API docs: <https://d.getjobber.com/docs/api/>
