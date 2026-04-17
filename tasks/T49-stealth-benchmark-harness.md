# T49 - Stealth Benchmark Harness

> Depends on: none (can start immediately)

## Goal

Create a repeatable benchmark and validation harness for stygian-browser stealth behavior against public fingerprinting and anti-bot test surfaces.

## Why

Current stealth validation is mostly ad-hoc. We need deterministic, repeatable runs with machine-readable outputs so regressions are caught quickly in CI and local development.

## Scope

- Add `crates/stygian-browser/src/validation/benchmark.rs` and module wiring.
- Define benchmark targets and expected outcome schema.
- Add a CLI-oriented example under `crates/stygian-browser/examples/stealth_benchmark.rs`.
- Output JSON report and markdown summary.

## Required Capabilities

- Standardized target model:
  - `name`
  - `url`
  - `category` (`fingerprint`, `challenge`, `network-leak`)
  - `timeout`
  - extraction hooks for score/status
- Metrics collected per target:
  - `pass/fail`
  - elapsed time
  - status code (if available)
  - parsed score fields (if available)
  - screenshot path on failure
- Batch runner:
  - run selected targets
  - run all targets
  - continue-on-error mode

## Test Targets (initial)

- CreepJS
- BrowserLeaks (selected pages)
- bot.sannysoft
- one Cloudflare-protected control URL (ignored test)

## Tests

- Unit test: config parsing and target filtering.
- Unit test: report serialization and deterministic field order.
- Integration test (ignored): run one live target and assert result schema completeness.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-browser --all-features
cargo clippy -p stygian-browser --all-features -- -D warnings
```

## Exit Criteria

- [ ] Reproducible benchmark runner exists and is documented
- [ ] JSON and markdown outputs are generated
- [ ] Basic live-target integration test exists (ignored)
- [ ] No clippy warnings introduced
