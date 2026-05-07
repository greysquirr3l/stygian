# Reverse-Engineering Promotion Policy

## Overview

This policy defines the process for promoting reverse-engineering (RE) findings into production stealth rules within Charon. It ensures that only well-validated, reproducible findings are deployed, minimizing regression risk and maintaining detection reliability.

## 1. Promotion Criteria

### 1.1 Evidence Quality Requirements

All RE findings must meet the following evidence quality standards before promotion:

- **Reproducibility:** Finding must be independently verified by at least one additional reviewer on a clean environment (different machine/network if applicable).
- **Root Cause Analysis:** Clear explanation of what the finding detects and why it matters for stealth validation.
- **Signal Specificity:** Finding must target bot/automation detection with minimal false-positive risk. Cross-reference against known legitimate use cases.
- **Attack Surface Justification:** Documented rationale for why this specific signal is worth hardening against.

### 1.2 Testing Requirements

Before promotion, RE findings must pass:

- **Unit Tests:** Coverage of the detection logic with at least 5 test cases (positive, negative, edge cases).
- **Regression Tests:** Verification that the finding doesn't degrade detection scores on existing stealth benchmarks (e.g., PixelScan, Sannysoft, anti-bot APIs).
- **Cross-Profile Tests:** Confirmation that the finding is compatible with all configured stealth profiles (Chrome, Firefox, Safari, Edge variants).
- **Performance Tests:** Confirmation that the finding adds <5ms latency to page load/stealth verification cycles.

### 1.3 Documentation Requirements

- **Finding Summary:** Clear description in GitHub issue, linked from the promotion PR
- **Signal Definition:** Technical specification (JavaScript, CSS, timing, or other mechanism)
- **Threat Model:** What attacker capabilities does this address?
- **Risk Assessment:** Likelihood of false positives, affected workflows, customer impact
- **Rollout Plan:** Proposed rollout schedule (phase 1: advisory only, phase 2: hard fail threshold)

## 2. Approval Path

### 2.1 Promotion Stakeholders

**Required Reviewers (All 3 must approve):**

- **Stealth Lead:** Owns overall detection strategy; reviews signal validity and strategy fit
- **RE Specialist:** Lead reverse engineer; verifies reproducibility and root cause analysis
- **SRE/DevOps:** Validates performance impact, rollback safety, monitoring setup

**Optional Reviewers (Context-specific):**

- **Product/Customer Success:** If finding impacts specific customer workflows
- **Security:** For critical infrastructure signals or attack surface changes

### 2.2 Promotion Process

1. **Discovery Phase:**
   - RE finding documented in GitHub issue (linked to #56 CHR-013 or new issue)
   - Initial evidence attached: screenshots, HAR files, replay videos

2. **Validation Phase:**
   - Find assigned to primary reviewer (Stealth Lead or RE Specialist)
   - Reviewer reproduces finding independently
   - Fixes/clarifications requested in issue comments
   - Evidence quality checklist completed

3. **Implementation Phase:**
   - Promotion branch created from `main` (`git checkout -b chr-###-re-signal-name`)
   - Unit tests and cross-profile tests added
   - Regression test against existing benchmarks run
   - Draft PR opened with finding documentation

4. **Review & Approval Phase:**
   - All 3 required reviewers assigned to PR
   - Each reviewer checks their domain (strategy, reproducibility, performance)
   - Feedback incorporated; `Approved` given when ready
   - Minimum 24h hold for async review, unless urgent escalation approved by Stealth Lead

5. **Merge & Rollout Phase:**
   - PR merged to `main` with squash commit
   - Commit message links GitHub issue and references all approvers
   - Rollout configured in `.github/stealth-canary.toml` (advisory=true for phase 1)
   - Monitoring dashboards set up before phase 2 activation

### 2.3 Approval Authority

- **Low Risk** (cosmetic signals, existing detector improvements): Stealth Lead + RE Specialist approval sufficient
- **Medium Risk** (new attack surface, <10% of traffic impact): All 3 reviewers + SRE sign-off required
- **High Risk** (major signal type, >10% false positive potential): All 3 reviewers + Product/Customer Success + escalation vote by majority

## 3. Rollback Procedure

### 3.1 Automatic Rollback Triggers

The following events automatically trigger rollback without explicit approval:

- **Score Drop >5%:** Any stealth profile score drops >5% when finding activated on real traffic
- **False Positive Surge:** False positive rate exceeds documented baseline by >200%
- **Performance Regression:** Page load latency increases >10ms on average
- **Crash/Memory Leak:** Finding causes JavaScript errors, memory leaks, or browser crashes
- **Customer Escalation:** Critical customer impact reported with reproducible scenario

### 3.2 Manual Rollback Procedure

**Triggered when:** Medium-risk issues detected but not auto-trigger criteria

**Steps:**

1. Issue created and labeled `rollback-candidate` with evidence
2. Stealth Lead reviews within 2 hours
3. If approved, create rollback branch: `git checkout main && git checkout -b rollback/chr-###-YYYYMMDD-hhmmss`
4. Revert the promotion commit: `git revert <commit-hash> --no-edit`
5. Add commit message suffix: `[ROLLBACK] <reason>`
6. Push branch and fast-track PR to main (expedited review, 30min timeout)
7. Once merged, rollout disabled in canary config
8. Post-mortem started within 24h

### 3.3 Rollback Communication

Upon rollback:

- GitHub issue updated with status: `ROLLED_BACK`
- Team Slack notification posted (#stealth-channel or similar)
- Customer support notified if affecting live traffic
- RCA initiated; findings documented in incident runbook

### 3.4 Re-Promotion After Rollback

After rollback, finding may be re-promoted once:

- Root cause of rollback fully understood
- Fix implemented and tested
- Evidence re-collected and documented
- Approval re-obtained from all original reviewers + 1 additional independent reviewer

## 4. Monitoring & Metrics

### 4.1 Pre-Promotion Checklist

- [ ] Unit tests: ≥5 cases, ≥80% coverage of signal detection logic
- [ ] Regression tests: Stealth scores stable (±1%) on benchmark sites
- [ ] Performance tests: <5ms added latency (measured across 3+ runs)
- [ ] Documentation: Signal definition, threat model, risk assessment, rollout plan complete
- [ ] Reproducibility: Finding verified independently by at least 1 reviewer
- [ ] Cross-profile: Confirmed working on Chrome, Firefox, Safari major versions
- [ ] Approvals: All required stakeholders signed off

### 4.2 Post-Promotion Monitoring

After merge, monitor for 7 days before phase 2 activation:

- **Daily Checks:**
  - Stealth canary scores stable (within ±2% of baseline)
  - Error logs clean (no JavaScript errors from finding code)
  - Performance metrics normal (<1% latency increase)

- **Weekly Checks:**
  - False positive reports from support/customers (target: 0)
  - Compatibility issues on edge browser versions
  - A/B test results (if applicable)

### 4.3 Phase 1 → Phase 2 Transition

After 7 days of stable monitoring, finding moves from `advisory=true` to hard-fail threshold:

1. Evidence collected from phase 1 monitoring
2. Stealth Lead approves phase 2 transition (or schedules another review week)
3. Config updated in `.github/stealth-canary.toml`: set `advisory=false` and `threshold` to target value
4. Commit and deploy; canary re-validates
5. Announce in team meeting; document in changelog

## 5. Escalation & Appeals

### 5.1 Approval Deadlock

If reviewers cannot reach consensus within 5 business days:

1. Stealth Lead mediates; documents decision rationale
2. If still unresolved, escalate to project maintainer (@greysquirr3l) for tie-break
3. Decision documented and linked in GitHub issue for future reference

### 5.2 Urgent Promotion (Security Fix)

In case of critical security regression or zero-day bypass:

1. Create issue with label `security-urgent` and link to vulnerability details
2. Stealth Lead + RE Specialist approval sufficient (SRE async approval acceptable)
3. Expedited PR review (1h timeout); 24h phase 1 monitoring window before hard-fail activation
4. Post-mortem scheduled to review whether process should be updated

## 6. Examples

### Example 1: Low-Risk Cosmetic Signal

- **Finding:** Detect common AI-generated User-Agent strings
- **Evidence Quality:** Found in 2 major LLM tools; reproducible with public APIs
- **Approvals:** Stealth Lead + RE Specialist (SRE approval optional for cosmetic)
- **Timeline:** ~3 days from discovery to production
- **Rollout:** Phase 1 (advisory) immediately; phase 2 after 7d monitoring

### Example 2: Medium-Risk New Attack Vector

- **Finding:** New timing-based fingerprint correlation attack
- **Evidence Quality:** Requires controlled lab setup; impacts ~5% of traffic
- **Approvals:** All 3 reviewers + Product sign-off required
- **Testing:** 10+ unit tests; cross-profile verification on 5 major browsers
- **Timeline:** ~2 weeks from discovery to phase 1; ~3 weeks to phase 2
- **Rollback:** Automatic if stealth scores drop >3%

### Example 3: High-Risk Attack Surface

- **Finding:** New fingerprinting vector via undocumented WebGL extension
- **Evidence Quality:** Affects 30%+ of traffic; false positive risk <2%
- **Approvals:** All 3 reviewers + majority vote by Stealth Lead/Product/Security
- **Testing:** 20+ unit tests; cross-profile on 8+ device/browser combinations
- **Timeline:** ~1 month from discovery to production; phased rollout (5% → 25% → 100%)
- **Rollback:** Manual approval required; auto-trigger only for >10% score drop

## 7. Review & Updates

This policy is reviewed quarterly or when significant issues arise. Stakeholders:

- Stealth Lead
- RE Specialist
- SRE/DevOps representative

Updates require consensus among all three parties and documentation in GitHub issues.

---

**Last Updated:** 2026-05-06  
**Next Review:** 2026-08-06
