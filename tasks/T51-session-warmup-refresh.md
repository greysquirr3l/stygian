# T51 - Session Warmup and Refresh Primitives

> Depends on: T50 preferred (profile coherence), can proceed independently

## Goal

Introduce browser session warmup and refresh primitives that make follow-up requests look like realistic user continuation instead of cold starts.

## Why

Many anti-bot systems score request context from prior resource loads, cookie state, and connection continuity. Warmup/refresh improves realism and reduces immediate challenge rates.

## Scope

- Add warmup API on `PageHandle` or `BrowserHandle`:
  - navigate + optional subresource stabilization
  - capture warmup metadata
- Add refresh primitive:
  - logical session refresh retaining cookies/storage state
  - configurable connection reset behavior
- Expose these capabilities through MCP browser tools where appropriate.

## Required Capabilities

- `warmup(url, options)`
- `refresh(options)`
- idempotent behavior for repeated calls
- report diagnostics:
  - resources observed
  - warmup elapsed
  - stabilization status

## Tests

- Unit test: warmup option defaults and serialization.
- Integration test (ignored): warmup then extraction on same origin.
- Integration test (ignored): refresh keeps cookie/session state.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-browser --all-features
cargo clippy -p stygian-browser --all-features -- -D warnings
```

## Exit Criteria

- [ ] Warmup and refresh APIs implemented and documented
- [ ] Session-state continuity verified by tests
- [ ] MCP exposure added or explicitly documented as deferred
- [ ] No regressions in existing navigation flows
