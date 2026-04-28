# Operations & Runbooks

This section collects the release-day and production references for `stygian-charon`.

---

## Test and validation commands

```bash
# Crate tests with all features
cargo test -p stygian-charon --all-features

# Strict linting for the crate
cargo clippy -p stygian-charon --all-features --examples --tests -- -D warnings
```

Optional integration checks:

- Redis cache integration test requires `STYGIAN_REDIS_URL`.
- Live target validation smoke test requires `STYGIAN_LIVE_URL`.

---

## Diagnostic and integration guides

The crate ships additional guides under `crates/stygian-charon/docs/`:

- `caching-integration-guide.md`
- `metrics-integration-guide.md`
- `slo-usage-guide.md`
- `output-structure.md`
- `signal-coverage-matrix.md`
- `incident-runbook.md`

Use these during rollout planning and incident triage to keep operator behavior consistent.

---

## Suggested release checklist

1. Run crate tests and clippy with all release features enabled.
2. Verify fixture drift checks are green.
3. Confirm docs for metrics/caching match enabled feature flags in deployment manifests.
4. Validate that runbook references map to current alerting and on-call workflows.

---

## Where Charon fits

Charon is a diagnostics-and-guidance component. It does not replace execution adapters.

- Use `stygian-graph` to run pipelines.
- Use `stygian-browser` / `stygian-proxy` for acquisition execution.
- Use Charon output to choose and tune those execution strategies.
