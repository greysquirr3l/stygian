# Backtest Acceptance Thresholds

## Overview

This document defines acceptance criteria for profile backtests run against new rules or analyzer versions. Backtests compare profiles' behavior on representative historical data, enabling safe rollout decisions with quantified impact.

## When to Run Backtests

Backtests should be run before:

1. **Rolling out new detection rules** — Verify rules don't introduce false positives
2. **Updating analyzer logic** — Ensure changes improve or maintain detection quality
3. **Introducing new profiles** — Validate against baseline profiles
4. **Major stealth updates** — Confirm no significant regression on historical patterns

Backtests are NOT required for:
- Documentation updates
- Non-functional refactoring
- Minor bug fixes in unrelated code

## Acceptance Criteria

All profiles must meet these minimum thresholds to be approved for production rollout:

### 1. Detection Rate

**Metric**: Percentage of cases where profile detects suspicious activity (non-Unknown provider)

- **Minimum**: 70%
  - Profiles detecting fewer than 70% of adversarial cases lack sufficient signal coverage
  - Indicates missing rules or insufficient analyzer maturity

- **Target**: 85%+
  - Indicates well-tuned profile with good coverage

- **Maximum Drift**: ±5% from baseline profile
  - Changes beyond ±5% warrant investigation for false positives or detection gaps

### 2. Average Confidence Score

**Metric**: Mean confidence level across all detections (0.0–1.0)

- **Minimum**: 0.60
  - Low confidence (<0.6) suggests weak signal evidence or ambiguous patterns
  - Rules with such confidence should be marked advisory or refined

- **Target**: 0.75+
  - Indicates high confidence in rule set

- **Maximum Drift**: ±0.10 from baseline profile
  - Larger shifts suggest quality degradation or improved tuning

### 3. Low Confidence Rate

**Metric**: Percentage of samples with confidence < 0.5 (potential false positives)

- **Maximum**: 15%
  - More than 15% low-confidence detections suggest FP risk

- **Target**: <5%
  - Indicates high-quality, confident rules

### 4. Disagreement Count

**Metric**: Number of cases where this profile diverges from other profiles

- **Maximum**: 10% of total cases
  - Profiles diverging on >10% of cases may indicate overfitting or incompatible rules

- **Target**: <5% disagreement rate
  - Good agreement across profiles indicates stable, shared understanding

- **Investigation Required**: >20% disagreement
  - High divergence warrants root cause analysis before rollout

### 5. Overall Impact on Detection Accuracy

**Metric**: Aggregate detection accuracy across all samples

- **Minimum**: 80% accuracy vs. baseline
  - Profiles performing <80% of baseline risk degrading production detection

- **Target**: ≥100% accuracy (no regression)
  - New rules should maintain or improve detection without introducing FPs

- **Acceptance**: Requires baseline profile comparison
  - Compute: (new_profile_detections / baseline_profile_detections) * 100

## Phase Rollout Strategy

### Phase 0: Backtest & Lab

1. **Run backtest** on representative corpus (≥100 real-world cases)
2. **Compute metrics** per acceptance criteria above
3. **Review results**: All 5 metrics must pass
4. **Approval**: Stealth Lead signs off on backtest report

### Phase 1: Advisory (5% Traffic Sample)

- Rules deployed with `advisory=true` in canary config
- No merge blocking; issues created but don't fail CI
- Monitor for false positive spikes
- Duration: 3–7 days depending on traffic volume

**Go/No-Go Decision**:
- **GO**: No regression in metrics, false positives within acceptable bounds
- **NO-GO**: Revert rule, conduct RCA, modify rule, re-backtest

### Phase 2: Hard Fail (100% Traffic)

- Rules deployed with `advisory=false` and defined threshold
- Merge blocked if stealth checks fail
- Full production deployment
- Continuous monitoring for detection drift

**Monitoring Metrics**:
- Daily stealth canary score changes (±1% tolerance)
- Customer-reported false positive incidents
- Detection rate stability over rolling 7-day window

## Metric Computation Reference

Given a backtest report with N samples across P profiles:

```
detection_rate = (cases_with_non_unknown_provider / total_cases) * 100%

avg_confidence = sum(confidence_scores) / total_cases

low_confidence_rate = (cases_with_confidence_lt_0.5 / total_cases) * 100%

disagreement_count = number of cases where profiles diverge (from report.disagreements)

accuracy_vs_baseline = (new_profile_detection_rate / baseline_profile_detection_rate) * 100%
```

## Examples

### Example 1: New Rule Passes Backtest

Profile metrics:
- Detection Rate: 84% ✅
- Avg Confidence: 0.78 ✅
- Low Confidence Rate: 3% ✅
- Disagreement Count: 4 cases (3% of 120) ✅
- Accuracy vs Baseline: 102% ✅

**Decision**: APPROVED for Phase 1 advisory deployment

### Example 2: New Rule Fails Backtest

Profile metrics:
- Detection Rate: 65% ❌ (below 70% minimum)
- Avg Confidence: 0.72 ✅
- Low Confidence Rate: 18% ❌ (above 15% maximum)
- Disagreement Count: 22 cases (18% of 120) ❌
- Accuracy vs Baseline: 76% ❌ (below 80% minimum)

**Decision**: REJECTED. Requires rule refinement before re-testing.

### Example 3: Borderline Backtest (Investigation Required)

Profile metrics:
- Detection Rate: 88% ✅
- Avg Confidence: 0.68 ✅
- Low Confidence Rate: 14% ⚠️ (just under 15% limit)
- Disagreement Count: 18 cases (15% of 120) ⚠️ (above 10% target)
- Accuracy vs Baseline: 98% ✅

**Decision**: CONDITIONAL APPROVAL. Approved for Phase 1 with additional monitoring:
1. Set lower thresholds in Phase 1 (advisory=true, extended monitoring window)
2. Enhance logging for low-confidence cases to understand FP patterns
3. Plan Phase 2 activation only after 7+ days of clean monitoring

## Escalation & Override

### When Metrics Fail

If metrics fall short of minimums:

1. **RCA**: Investigate why rule/profile underperforms
2. **Options**:
   - Refine rule logic, re-backtest
   - Adjust profile weights, re-backtest
   - Mark rule as advisory-only pending future improvements
   - Defer rollout pending other dependent work

### Urgent/Security Override

In case of critical security fix or zero-day bypass:

1. **Stealth Lead approval** required (standard 24h hold waived)
2. **Reduced backtest corpus acceptable** (minimum 20 cases vs. 100)
3. **Phase 0 → Phase 2 directly** (skip advisory phase)
4. **Post-mortem within 48h** to review if fast-track was justified

## Review Cadence

Acceptance thresholds are reviewed quarterly or when:
- Significant false positive incident occurs
- Production regression detected
- New analyzer version or major rule set change
- Customer feedback suggests threshold misalignment

Reviewers: Stealth Lead, RE Specialist, SRE

---

**Last Updated**: 2026-05-06  
**Next Review**: 2026-08-06
