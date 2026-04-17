# T53 - FreeAPIProxies Source Adapter (Optional)

> Depends on: T52 preferred

## Goal

Implement an optional proxy-source adapter in stygian-proxy for `freeapiproxies.azurewebsites.net` with strict quality gating and conservative defaults.

## Why

Provides fast bootstrap proxy ingestion for experiments and low-risk workloads while maintaining production-safe behavior through aggressive filtering and quarantine.

## Scope

- Add adapter under `crates/stygian-proxy/src/adapters/`.
- Parse JSON/TXT responses from API.
- Normalize into internal proxy model.
- Add configuration for protocol/country/provider filters.

## Safety Defaults

- Disabled by default behind feature flag.
- Short TTL for sourced proxies.
- High initial penalty score until validated.
- Automatic quarantine on repeated failures.

## Required Capabilities

- Config:
  - type (`http`/`socks4`/`socks5`)
  - country/isocode
  - https support filter
  - request timeout
  - max imported per sync
- Validation pipeline:
  - basic connect check
  - optional target health probe
  - ban list and cooldown windows

## Tests

- Unit test: JSON parser and normalization.
- Unit test: filter query generation.
- Integration test (mocked HTTP): import + score + quarantine transitions.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-proxy --all-features
cargo clippy -p stygian-proxy --all-features -- -D warnings
```

## Exit Criteria

- [ ] Feature-gated adapter implemented
- [ ] Strict default filters and quarantine behavior in place
- [ ] Parsing and import logic tested
- [ ] Documentation warns against production-critical reliance on free pools
