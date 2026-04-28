# P0 to P1 E2E Latency Profiling

This document captures how to profile end-to-end Charon pipeline latency for
P0->P1 validation scenarios.

## Command

```bash
cargo test -p stygian-charon --all-features --test p0_p1_e2e -- --nocapture
```

## What the suite validates

- Target-class happy paths for `Api`, `ContentSite`, and `HighSecurity`.
- Zone transitions (acceptable/warning/critical).
- Edge cases:
  - mixed 403 + 429 signals,
  - exact threshold boundary behavior,
  - requirement severity escalation.
- Per-scenario latency assertion (`< 1s`).

## Notes

- The suite is deterministic and uses synthetic HAR payloads.
- The latency check is bounded per scenario to avoid long CI regressions.
- If required by ops, measured timings from a given run can be pasted below.

### Last measured snapshot

- Latest run: `cargo test -p stygian-charon --all-features --test p0_p1_e2e` passed (5/5).
- The suite enforces sub-second elapsed time per scenario through test assertions.
