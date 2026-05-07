# Release Risk Score and Trend Thresholds

This document defines CHR-014 rollout criteria for release-candidate (RC) risk scoring and trend analysis.

## Purpose

Before promoting a candidate, Charon aggregates regression pressure from three signal families:

1. Probe regressions (`ProbePackReport` failures)
2. Drift regressions (`ModeDifferentialRunReport` failing pairs)
3. Observatory regressions (`ObservatoryReport` comparisons with `investigate_regression`)

It then overlays incident pressure from recent operations (7-day and 30-day windows).

## Aggregate Score Model

The release risk score is a weighted sum in [0.0, 1.0]:

- `probe_failure_ratio * 0.35`
- `drift_failure_ratio * 0.25`
- `observatory_regression_ratio * 0.20`
- `incident_pressure_7d * 0.15`
- `incident_pressure_30d * 0.05`

Incident pressure saturation points:

- 7-day incident pressure saturates at 3 incidents
- 30-day incident pressure saturates at 10 incidents

## Risk Levels

`ReleaseRiskThresholds` default cutoffs:

- `Low`: score < 0.30
- `Guarded`: score >= 0.30 and < 0.55
- `Elevated`: score >= 0.55 and < 0.75
- `Critical`: score >= 0.75

## Escalation Gates

Escalation is required when any of the following are true:

1. Aggregate score is `Critical`
2. Probe failure ratio >= 10%
3. Drift failure ratio >= 20%
4. Observatory regression ratio >= 25%
5. Incident count in last 7 days >= 3

## Trend Report Rules

`ReleaseTrendReport` computes per-candidate score deltas and trend direction:

- `Degrading` when delta >= +0.03
- `Improving` when delta <= -0.03
- `Stable` otherwise

Trend-level escalation triggers when:

1. Latest candidate already requires escalation, or
2. Degrading streak reaches 3 consecutive candidate transitions

## Operational Guidance

1. `Low`: Proceed with routine release checks.
2. `Guarded`: Proceed with elevated monitoring and rollback readiness.
3. `Elevated`: Require explicit reviewer sign-off and staged rollout.
4. `Critical`: Block release until regressions are mitigated.

## References

- Source implementation: `crates/stygian-charon/src/release_risk.rs`
- Related policy: `crates/stygian-charon/docs/re-promotion-policy.md`
- Backtest thresholds: `crates/stygian-charon/docs/backtest-acceptance-thresholds.md`
