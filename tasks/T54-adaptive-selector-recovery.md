# T54 - Adaptive Selector Recovery for Extraction

> Depends on: T33 (`find_similar`) and T34 (MCP DOM tools)

## Goal

Add adaptive selector recovery in stygian-browser extraction flows so selector drift can be repaired automatically using structural similarity and nearby context.

## Why

Layout and class changes frequently break fixed selectors. Automatic recovery reduces maintenance burden and increases extraction durability.

## Scope

- Extend extraction API with recovery mode.
- Add selector fallback strategy stack:
  - strict selector
  - relaxed selector transforms
  - similarity-based nearest candidate
- Persist optional recovery hints for future runs.

## Required Capabilities

- Recovery policy config:
  - `off`, `safe`, `aggressive`
- Explainability output:
  - original selector
  - recovered selector/candidate path
  - confidence score
- Hooks for MCP extraction tools to return recovery diagnostics.

## Tests

- Unit test: selector fallback ordering.
- Unit test: confidence threshold gating.
- Integration test (fixture HTML): broken selector is recovered in `safe` mode.
- Integration test: unrecoverable case returns typed extraction error with diagnostics.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-browser --all-features
cargo clippy -p stygian-browser --all-features -- -D warnings
```

## Exit Criteria

- [ ] Recovery mode available for extraction flows
- [ ] Confidence-scored diagnostics returned
- [ ] MCP extraction output includes recovery information when used
- [ ] Tests cover recoverable and unrecoverable scenarios
