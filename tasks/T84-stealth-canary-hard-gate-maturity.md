# T84 — Stealth Canary Hard-Gate Maturity

## Goal

Mature stealth CI from binary pass/fail checks to governance-grade gating with trend awareness.

## Scope

- Add trend-based regression checks for canary score degradation.
- Define required non-advisory canary target set and fail policy.
- Improve CI summaries with ownership/runbook links and artifact pointers.

## Feature flag

This is a CI/operational change, not a library feature. No new feature
gate required. The trend-detection logic should live in a new module under
`tools/stealth-canary/` or as part of the existing CI workflow under
`.github/workflows/`. Document any new scripts in `tools/stealth-canary/README.md`.

## Depends on

- T83 challenge-aware policy feedback loop (challenge outcomes feed the
  trend signal).

## Unblocks

- T88 anti-bot change detection feed (canary deltas are a primary input).

## Must Haves

- Merge-blocking behavior remains explicit and auditable.
- Advisory and hard-fail target semantics documented.

## Test Hints

- Unit: trend threshold logic for degradation detection.
- Workflow validation: summary includes links and clear ownership
  context (validate by inspecting the rendered Markdown, not by running
  CI in a unit test).

## Exit Criteria

- [ ] Trend-aware gating implemented: canary score regression over a
      rolling window fails the workflow.
- [ ] Required non-advisory (hard-fail) canary target set is enumerated
      and committed as data (YAML/JSON in `tools/stealth-canary/`).
- [ ] CI summary updated to include ownership contacts, runbook links,
      and artifact pointers (HAR, canary JSON, etc.).
- [ ] At least 2 unit tests for trend threshold logic (stable score =
      pass, monotonic regression = fail).
- [ ] Governance doc updated (`docs/stealth-canary-governance.md` or
      equivalent) with advisory vs hard-fail semantics.
- [ ] `cargo build --workspace --all-features` clean (CI scripts should
      not introduce Rust build issues).
- [ ] AGENTS.md rules respected for any Rust tooling added.
