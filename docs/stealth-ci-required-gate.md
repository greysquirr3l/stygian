# Stealth CI Required Gate

This document records the repository-level guardrail for CHR-010: stealth checks
must block merge when non-advisory regressions are detected.

For the **authoritative reference** on what fails the build today, what
warns, and why, see
[`stealth-canary-governance.md`](stealth-canary-governance.md) (T84). The
two documents are complementary: this one is the **branch-protection
contract**, the other is the **target + trend semantics**.

## Required Branch Protection Settings

For the `main` branch protection rule:

1. Enable required status checks.
2. Include the status check context that contains `Stealth probe` (emitted by
   `.github/workflows/stealth-canary.yml`).
3. Keep advisory targets non-blocking by leaving their `advisory = true`
   entries in `.github/stealth-canary.toml`.

## Continuous Audit Workflow

Workflow: `.github/workflows/stealth-gate-audit.yml`

What it verifies:

1. A branch protection rule exists for `main`.
2. `requiresStatusChecks` is enabled.
3. Required status check contexts include a value containing `Stealth probe`.

The audit runs on:

1. Weekly schedule.
2. Manual `workflow_dispatch`.

## Expected Failure Modes

The audit fails when:

1. Branch protection for `main` is missing.
2. Required status checks are disabled.
3. The stealth check context is not required.

Use the failed job logs and step summary to identify the missing context name
and update branch protection accordingly.

## Trend-Aware Hardening (T84)

The T84 trend detector adds a **second failure axis** on top of the
branch-protection gate: a run that passes the probe axis can still
fail the build when the rolling-window score regression detector
flags a regression. See
[`stealth-canary-governance.md`](stealth-canary-governance.md#2-required-non-advisory-target-set)
for the target set, the trend knobs, and the opt-in baseline
env vars.
