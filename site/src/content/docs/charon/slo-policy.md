# SLO & Policy Planning

Charon turns diagnostic evidence into target-aware SLO outcomes and runtime-policy decisions.

---

## SLO zones

Blocked-ratio assessment is evaluated against target-class-specific thresholds and grouped into
zones used by policy planning.

| Zone | Meaning | Typical action |
| --- | --- | --- |
| Acceptable | Within expected tolerance | Keep current strategy |
| Warning | Degrading behavior | Increase retries and warmup |
| Critical | Sustained blocking risk | Escalate acquisition mode |

---

## Planning sequence

1. Start from a normalized report (`investigate_har` or equivalent report source).
2. Infer requirements (`infer_requirements` / `infer_requirements_with_target_class`).
3. Build policy (`build_runtime_policy` or `plan_from_report`).
4. Map policy to acquisition hints (`map_runtime_policy`, adapter strategy mapping).

This makes policy behavior explicit, testable, and consistent across run environments.

---

## Adaptive policy tuning

For long-running systems, use adaptive helpers:

- `AdaptiveSloPolicy` for selecting and updating SLO posture.
- `RegressionHistoryPolicy` for history-driven threshold behavior.

These APIs allow controlled drift handling instead of one-off threshold edits.

---

## Snapshot compatibility and drift

Use snapshot helpers to keep identity checks deterministic:

- schema compatibility validation for normalized snapshots
- drift comparison utilities that focus on meaningful signal changes
- fixture-backed checks in integration tests

These are especially useful when rolling stealth/profile changes that could affect anti-bot
classification confidence.
