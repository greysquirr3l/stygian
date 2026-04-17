# T50 - Transport Profile Packs and Cadence

> Depends on: T49 recommended (for validation), not required for implementation

## Goal

Expand TLS/HTTP fingerprint profile modeling from static presets to versioned profile packs with clear update cadence and profile metadata.

## Why

Stealth quality depends on coherent transport behavior over time. We need predictable profile updates and metadata to keep fingerprints aligned with browser release drift.

## Scope

- Add profile pack abstraction in `crates/stygian-browser/src/tls.rs` (or split module).
- Define named channels:
  - `chrome-latest`
  - pinned historical profiles (existing `CHROME_131`, etc.)
- Add profile metadata:
  - browser family/version
  - platform class
  - h2/h3 support flags
  - generation date and source notes

## Required Capabilities

- Resolve profile by channel + platform hint.
- Keep existing API stable for pinned constants.
- Add helper to report profile provenance for diagnostics.
- Ensure UA and transport profile alignment helpers remain coherent.

## Tests

- Unit test: channel resolution returns expected pinned profile.
- Unit test: metadata is present and serialized.
- Unit test: invalid channel returns typed error.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-browser --all-features
cargo clippy -p stygian-browser --all-features -- -D warnings
```

## Exit Criteria

- [ ] Channel-based profile resolution implemented
- [ ] Metadata and provenance available to callers
- [ ] Existing pinned-profile APIs remain backward compatible
- [ ] Tests cover resolution and error cases
