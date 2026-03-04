# GitHub GraphQL Pipelines

Ready-to-run Stygian pipelines for the [GitHub GraphQL API v4](https://docs.github.com/en/graphql).

## Requirements

### Environment variables

| Variable | Required | Purpose |
| --- | --- | --- |
| `GITHUB_TOKEN` | Yes | Personal access token |
| `ANTHROPIC_API_KEY` | Only for `full_sync` | Claude API key for the analysis node |

### Obtaining a GitHub personal access token

1. Go to **GitHub → Settings → Developer settings → Personal access tokens → Tokens (classic)**
2. Click **Generate new token**
3. Select the following scopes:
   - `read:user` — viewer profile queries
   - `public_repo` — public repository queries
   - `repo` — add this if you also want to read private repositories
4. Copy the generated token and export it:

```bash
export GITHUB_TOKEN="ghp_..."
```

No OAuth app registration or paid subscription required.

## Running pipelines

```bash
# Validate a pipeline without executing
stygian check examples/pipelines/github/repositories.toml

# Fetch all of your own repositories (cursor-paginated)
stygian run examples/pipelines/github/repositories.toml

# Fetch open issues from rust-lang/rust
stygian run examples/pipelines/github/issues.toml

# Fetch open pull requests from rust-lang/rust
stygian run examples/pipelines/github/pull_requests.toml

# Fetch your starred repositories (cursor-paginated)
stygian run examples/pipelines/github/starred.toml

# Full profile sync with AI analysis (requires ANTHROPIC_API_KEY)
export ANTHROPIC_API_KEY="sk-ant-..."
stygian run examples/pipelines/github/full_sync.toml

# Introspect the GitHub schema
stygian run examples/pipelines/github/introspect.toml > github_schema.json
```

## Targeting a different repository

The `issues.toml` and `pull_requests.toml` pipelines default to `rust-lang/rust`.
Edit the `[nodes.params.variables]` block to target any public repository:

```toml
[nodes.params.variables]
owner = "your-org"
name  = "your-repo"
```

## Pipeline structure

All pipelines follow this pattern — auth is inline on each node, and the
GitHub endpoint URL is specified directly rather than via a registered plugin:

```toml
[[services]]
name = "github"
kind = "graphql"

[[nodes]]
name    = "fetch_repositories"
service = "github"
url     = "https://api.github.com/graphql"

[nodes.params]
query = "..."

[nodes.params.auth]
kind  = "bearer"
token = "${env:GITHUB_TOKEN}"   # expanded at execution time

[nodes.params.pagination]
strategy       = "cursor"
page_info_path = "data.viewer.repositories.pageInfo"
edges_path     = "data.viewer.repositories.edges"
```

## DAG pipeline (`full_sync.toml`)

`full_sync.toml` shows multi-node DAG execution with parallel fetch and a
dependent AI analysis step:

```
fetch_viewer ─────┐
                  ├──► analyse_profile (Claude)
fetch_repositories┤
                  │
fetch_starred ────┘
```

The three fetch nodes run concurrently. `analyse_profile` only executes once
all upstream nodes have successfully completed, and receives their combined
output as context.

## Schemas

The `schemas/` directory contains JSON Schema definitions for the normalised
output of each pipeline:

| File | Description |
| --- | --- |
| `repository.schema.json` | Owned/starred repository record |
| `issue.schema.json` | GitHub issue record |
| `pull_request.schema.json` | Pull request record |
| `profile_summary.schema.json` | AI-generated developer profile summary |
