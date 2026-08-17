#!/usr/bin/env python3
"""Stealth canary trend detector — CLI entry point (T84).

Reads a per-run probe report (the JSON output of the
`stealth_probe` example + the `[[canary]]` entries from
`.github/stealth-canary.toml`), reads the trend history JSONL,
appends the current run, evaluates the per-target trend verdicts,
and emits:

  * `--verdict <path>`: JSON dict with the per-target verdicts +
    aggregate `hard_fail` / `hard_fail_labels` (used by the
    workflow to set the build status).
  * `--summary <path>`: a Markdown summary ready to append to
    `$GITHUB_STEP_SUMMARY` — includes the verdict table, the
    ownership / runbook / artifact pointers from the required
    targets data file, and links to the uploaded artifacts.
  * Updated history JSONL written back to `--history <path>`.

Exit codes:
  * 0 — every required target stable (or trend-insufficient) AND
         no per-target baseline breach.
  * 1 — at least one required target is hard-fail (regression
         detected, monotonic regression, or baseline breach).

Usage:

```
python3 trend_cli.py \
    --probe-report probe-report.json \
    --canary-config .github/stealth-canary.toml \
    --required-targets tools/stealth-canary/data/required-targets.toml \
    --history history/canary-history.jsonl \
    --run-id "${{ github.run_id }}" \
    --verdict /tmp/canary-verdict.json \
    --summary /tmp/canary-summary.md
```
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time
import tomllib
from typing import Any

# Allow `python3 trend_cli.py` to find the sibling `trend.py` module
# without requiring a package install. CI runs the script from the
# `tools/stealth-canary/` directory, so the relative import resolves
# directly; this fallback handles `python3 -m tools.stealth_canary.trend_cli`
# and tests that launch the script from a different cwd.
_HERE = pathlib.Path(__file__).resolve().parent
if str(_HERE) not in sys.path:
    sys.path.insert(0, str(_HERE))

# pylint: disable=wrong-import-position
import trend  # noqa: E402  (path-adjusted import)

# ── Helpers ──────────────────────────────────────────────────────────────────


def _load_toml(path: pathlib.Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def _load_required_targets(path: pathlib.Path) -> dict[str, dict[str, Any]]:
    """Read the required-targets data file into a label→entry map."""

    doc = _load_toml(path)
    raw_entries = doc.get("required")
    if not isinstance(raw_entries, list) or not raw_entries:
        raise SystemExit(
            f"required-targets file {path} must contain a non-empty "
            "[[required]] table list"
        )
    out: dict[str, dict[str, Any]] = {}
    for idx, entry in enumerate(raw_entries, start=1):
        if not isinstance(entry, dict):
            raise SystemExit(
                f"required-targets file {path} entry #{idx} must be a table"
            )
        label = entry.get("label")
        if not isinstance(label, str) or not label.strip():
            raise SystemExit(f"required-targets file {path} entry #{idx} missing label")
        out[label] = entry
    return out


def _load_canary_config(path: pathlib.Path) -> dict[str, dict[str, Any]]:
    """Read `.github/stealth-canary.toml` into a label→entry map."""

    doc = _load_toml(path)
    raw_entries = doc.get("canary")
    if not isinstance(raw_entries, list) or not raw_entries:
        raise SystemExit(
            f"canary config {path} must contain a non-empty " "[[canary]] table list"
        )
    out: dict[str, dict[str, Any]] = {}
    for entry in raw_entries:
        if not isinstance(entry, dict):
            continue
        label = entry.get("label")
        if isinstance(label, str) and label:
            out[label] = entry
    return out


def _load_probe_report(path: pathlib.Path) -> list[dict[str, Any]]:
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data, list):
        raise SystemExit(f"probe report {path} must be a JSON array")
    return data


def _env_baseline(env_name: str) -> float | None:
    """Parse an optional ``STYGIAN_*_BASELINE_*`` env var.

    Mirrors the T58 ``STYGIAN_TIER1_BASELINE_CREEPJS`` /
    ``STYGIAN_TIER1_BASELINE_BROWSERSCAN`` pattern: an unset or
    unparseable var yields ``None`` (the trend gate still runs,
    it just doesn't enforce a per-target baseline).
    """

    raw = os.environ.get(env_name)
    if raw is None or not raw.strip():
        return None
    try:
        value = float(raw)
    except ValueError:
        return None
    if not 0.0 <= value <= 1.0:
        return None
    return value


def _resolve_baselines(
    label: str, entry: dict[str, Any]
) -> tuple[float | None, str | None]:
    """Pick a baseline for ``label``.

    Order of precedence:
      1. ``STYGIAN_TREND_BASELINE_<LABEL_UPPER>`` env var (operator override)
      2. ``STYGIAN_TIER1_BASELINE_<LABEL_UPPER>`` env var (T58 contract)
      3. ``baseline`` field in the required-targets entry (data-file pin)

    Returns ``(value, env_var_name)``. The env-var name is included
    in the Markdown summary so operators can tell which source
    supplied the baseline.
    """

    upper = label.upper().replace("-", "_")
    for env_name in (
        f"STYGIAN_TREND_BASELINE_{upper}",
        f"STYGIAN_TIER1_BASELINE_{upper}",
    ):
        value = _env_baseline(env_name)
        if value is not None:
            return value, env_name
    raw_baseline = entry.get("baseline")
    if isinstance(raw_baseline, (int, float)) and 0.0 <= float(raw_baseline) <= 1.0:
        return float(raw_baseline), "required-targets.toml:baseline"
    return None, None


def _observation_severity_for(
    report_entry: dict[str, Any],
) -> str | None:
    """Extract a T92 ``CanaryTrendObservation`` severity, if any.

    The probe JSON may include a ``trend_observations`` field (an
    array of T92 ``CanaryTrendObservation`` records) on each
    entry. We use the highest-severity record as the per-target
    trend signal; the detector still receives the score from the
    probe itself.
    """

    severity_rank = {"clean": 0, "suspected": 1, "confirmed": 2}
    observations = report_entry.get("trend_observations")
    if not isinstance(observations, list):
        return None
    best_rank = -1
    best_severity: str | None = None
    for obs in observations:
        if not isinstance(obs, dict):
            continue
        severity = obs.get("severity")
        if not isinstance(severity, str):
            continue
        rank = severity_rank.get(severity, -1)
        if rank > best_rank:
            best_rank = rank
            best_severity = severity
    return best_severity


# ── Markdown summary ─────────────────────────────────────────────────────────


def _verdict_table(verdicts: list[trend.TrendVerdict]) -> str:
    """Render the per-target verdict table."""

    header = (
        "| label | status | current | rolling_mean | delta |"
        " consecutive_drops | baseline | observation |"
    )
    lines = [
        header,
        "|---|---|---:|---:|---:|---:|---|---|",
    ]
    for v in verdicts:
        rolling = f"{v.rolling_mean:.4f}" if v.rolling_mean is not None else "—"
        delta = f"{v.delta:+.4f}" if v.delta is not None else "—"
        baseline = (
            f"{v.baseline:.4f}{' ⚠️' if v.baseline_breach else ''}"
            if v.baseline is not None
            else "—"
        )
        observation = v.observation_severity or "—"
        lines.append(
            f"| `{v.label}` | {v.status.value} | {v.current_score:.4f} | "
            f"{rolling} | {delta} | {v.consecutive_drops} | "
            f"{baseline} | {observation} |"
        )
    return "\n".join(lines)


def _ownership_table(
    required: dict[str, dict[str, Any]],
    verdicts: dict[str, trend.TrendVerdict],
) -> str:
    """Render the ownership / runbook / artifact pointers table."""

    lines = [
        "| label | owner | secondary | runbook | artifacts |",
        "|---|---|---|---|---|",
    ]
    for label, entry in required.items():
        verdict = verdicts.get(label)
        if verdict is None:
            # Required target missing from current run — surface as a
            # special row so the on-call sees it.
            owner = entry.get("owner", "—")
            secondary = entry.get("secondary", "—")
            runbook = entry.get("runbook", "—")
            artifacts = ", ".join(f"`{a}`" for a in entry.get("artifacts", []))
            lines.append(f"| `{label}` | {owner} | {secondary} | {runbook} | ")
            lines[-1] += f"{artifacts} |"
            continue
        owner = entry.get("owner", "—")
        secondary = entry.get("secondary", "—")
        runbook = entry.get("runbook", "—")
        artifacts = ", ".join(f"`{a}`" for a in entry.get("artifacts", []))
        if verdict.is_hard_fail:
            owner = f"🛑 {owner}"
        lines.append(f"| `{label}` | {owner} | {secondary} | {runbook} | ")
        lines[-1] += f"{artifacts} |"
    return "\n".join(lines)


def _build_markdown(
    verdicts: dict[str, trend.TrendVerdict],
    required: dict[str, dict[str, Any]],
    config: trend.TrendConfig,
    run_id: str,
    run_url: str | None,
    artifacts: list[str],
) -> str:
    """Compose the final Markdown summary."""

    hard_fails = [v for v in verdicts.values() if v.is_hard_fail]
    headline = "## Stealth Canary — Trend Report"
    if hard_fails:
        headline += " 🛑\n\n" "**At least one required canary target is hard-fail.**"
    else:
        headline += "\n\nAll required canary targets are stable "
        headline += "(or trend-insufficient)."

    detector = (
        f"**Detector config:** window={config.window}, "
        f"delta_threshold={config.delta_threshold}, "
        f"monotonic_runs={config.monotonic_runs}, "
        f"min_history={config.min_history}."
    )
    if run_url:
        run_line = f"**Run:** [{run_id}]({run_url})"
    else:
        run_line = f"**Run:** `{run_id}`"

    artifact_lines = "\n".join(f"- `{a}`" for a in artifacts) or "- _(none)_"

    return (
        f"{headline}\n\n"
        f"{detector}\n"
        f"{run_line}\n\n"
        "### Trend verdicts\n\n"
        f"{_verdict_table(list(verdicts.values()))}\n\n"
        "### Ownership, runbook & artifacts\n\n"
        f"{_ownership_table(required, verdicts)}\n\n"
        "### Uploaded artifacts\n\n"
        f"{artifact_lines}\n"
    )


# Public wrappers used by tests to avoid private-member access diagnostics.
def env_baseline(env_name: str) -> float | None:
    """Public wrapper around baseline env parsing."""

    return _env_baseline(env_name)


def resolve_baselines(
    label: str,
    entry: dict[str, Any],
) -> tuple[float | None, str | None]:
    """Public wrapper around baseline resolution precedence logic."""

    return _resolve_baselines(label, entry)


def build_markdown(
    verdicts: dict[str, trend.TrendVerdict],
    required: dict[str, dict[str, Any]],
    config: trend.TrendConfig,
    run_id: str,
    run_url: str | None,
    artifacts: list[str],
) -> str:
    """Public wrapper around markdown rendering for summary tests."""

    return _build_markdown(
        verdicts,
        required,
        config,
        run_id,
        run_url,
        artifacts,
    )


# ── Main ─────────────────────────────────────────────────────────────────────


def main(argv: list[str] | None = None) -> int:
    """Parse CLI args, evaluate canary trend verdicts, and emit outputs."""
    parser = argparse.ArgumentParser(
        description="Stealth canary trend detector (T84)",
    )
    parser.add_argument(
        "--probe-report",
        type=pathlib.Path,
        required=True,
        help="Path to the per-run probe JSON report (the array of "
        "stealth_probe output objects augmented with label/url/threshold).",
    )
    parser.add_argument(
        "--canary-config",
        type=pathlib.Path,
        required=True,
        help="Path to .github/stealth-canary.toml.",
    )
    parser.add_argument(
        "--required-targets",
        type=pathlib.Path,
        required=True,
        help="Path to tools/stealth-canary/data/required-targets.toml.",
    )
    parser.add_argument(
        "--history",
        type=pathlib.Path,
        required=True,
        help="Path to the rolling trend history JSONL (read + append).",
    )
    parser.add_argument(
        "--run-id",
        default=os.environ.get("GITHUB_RUN_ID", "local"),
        help="Workflow run ID (defaults to $GITHUB_RUN_ID).",
    )
    parser.add_argument(
        "--run-url",
        default=os.environ.get("GITHUB_RUN_URL"),
        help="Workflow run URL (defaults to $GITHUB_RUN_URL).",
    )
    parser.add_argument(
        "--verdict",
        type=pathlib.Path,
        required=True,
        help="Path to write the JSON verdict file (consumed by the workflow).",
    )
    parser.add_argument(
        "--summary",
        type=pathlib.Path,
        required=True,
        help="Path to write the Markdown summary (appended to "
        "$GITHUB_STEP_SUMMARY).",
    )
    parser.add_argument(
        "--artifact",
        action="append",
        default=[],
        help="Artifact path/URL to surface in the summary. May be repeated.",
    )
    args = parser.parse_args(argv)

    config = trend.TrendConfig.from_env()
    history = trend.read_history(str(args.history))
    _load_canary_config(args.canary_config)
    required = _load_required_targets(args.required_targets)
    probe_report = _load_probe_report(args.probe_report)

    # Build the per-target current score map from the probe report.
    # The report rows are dicts with `label`, `score`, `ok`, etc.
    # (the same shape the workflow writes to probe-results.ndjson).
    current_by_label: dict[str, float] = {}
    trend_obs_by_label: dict[str, str] = {}
    for entry in probe_report:
        label = entry.get("label")
        if not isinstance(label, str) or not label:
            continue
        score = entry.get("score")
        if not isinstance(score, (int, float)):
            continue
        current_by_label[label] = float(score)
        severity = _observation_severity_for(entry)
        if severity is not None:
            trend_obs_by_label[label] = severity

    # Append the current run to the history so the next run can use it.
    now_ms = int(time.time() * 1000)
    new_entries: list[trend.HistoryEntry] = []
    for entry in probe_report:
        label = entry.get("label")
        if not isinstance(label, str) or not label:
            continue
        score = entry.get("score")
        threshold = entry.get("threshold")
        ok = entry.get("ok")
        trend_observations = entry.get("trend_observations")
        if not isinstance(score, (int, float)):
            continue
        new_entries.append(
            trend.HistoryEntry(
                label=label,
                score=float(score),
                threshold=(
                    float(threshold) if isinstance(threshold, (int, float)) else 0.0
                ),
                ok=bool(ok) if isinstance(ok, bool) else False,
                run_id=args.run_id,
                captured_at_epoch_ms=now_ms,
                trend_observations=(
                    list(trend_observations)
                    if isinstance(trend_observations, list)
                    else []
                ),
            )
        )
    if new_entries:
        trend.write_history(str(args.history), new_entries)

    # Evaluate trend verdicts for every required target that has a
    # current score. Targets missing from the probe report surface as
    # a synthetic 0.0 score so the on-call sees the gap.
    baselines: dict[str, float] = {}
    baseline_sources: dict[str, str] = {}
    for label, entry in required.items():
        baseline, source = _resolve_baselines(label, entry)
        if baseline is not None:
            baselines[label] = baseline
            baseline_sources[label] = source or "data-file"

    eval_labels = set(current_by_label) | set(required)
    eval_current: dict[str, float] = {
        label: current_by_label.get(label, 0.0) for label in eval_labels
    }
    eval_history: dict[str, list[float]] = {
        label: trend.history_to_score_map(history, label) for label in eval_labels
    }
    eval_severity: dict[str, str] = {
        label: (
            trend_obs_by_label.get(label)
            or trend.severity_for_label(history, label)
            or "clean"
        )
        for label in eval_labels
    }
    verdicts = trend.evaluate_per_target(
        eval_history,
        eval_current,
        config,
        baselines=baselines,
        observation_severity_by_label=eval_severity,
    )

    # Annotate the verdict with the resolved baseline source for the
    # Markdown summary.
    annotated_verdicts: dict[str, trend.TrendVerdict] = {}
    for label, verdict in verdicts.items():
        # Replace the verdict with a copy that carries the baseline
        # source. TrendVerdict is frozen so we build a new instance.
        if label in baseline_sources:
            verdict = trend.TrendVerdict(
                label=verdict.label,
                current_score=verdict.current_score,
                status=verdict.status,
                reason=(
                    verdict.reason + f" (baseline from {baseline_sources[label]})"
                    if verdict.baseline_breach
                    else verdict.reason
                ),
                run_count=verdict.run_count,
                rolling_mean=verdict.rolling_mean,
                delta=verdict.delta,
                consecutive_drops=verdict.consecutive_drops,
                baseline=verdict.baseline,
                baseline_breach=verdict.baseline_breach,
                observation_severity=verdict.observation_severity,
            )
        annotated_verdicts[label] = verdict

    aggregate = trend.aggregate_verdict(annotated_verdicts)
    aggregate["run_id"] = args.run_id
    aggregate["config"] = {
        "window": config.window,
        "delta_threshold": config.delta_threshold,
        "monotonic_runs": config.monotonic_runs,
        "min_history": config.min_history,
    }
    aggregate["required_labels"] = sorted(required.keys())
    aggregate["current_labels"] = sorted(current_by_label.keys())

    # Write the verdict JSON. Workflow reads this file to set the
    # `regression` / `issue` step outputs.
    args.verdict.parent.mkdir(parents=True, exist_ok=True)
    with args.verdict.open("w", encoding="utf-8") as handle:
        json.dump(aggregate, handle, indent=2)

    # Write the Markdown summary. Workflow appends it to
    # $GITHUB_STEP_SUMMARY.
    summary_md = _build_markdown(
        annotated_verdicts,
        required,
        config,
        args.run_id,
        args.run_url,
        args.artifact,
    )
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    with args.summary.open("w", encoding="utf-8") as handle:
        handle.write(summary_md)

    # Also echo a one-line JSON summary to stderr so the workflow log
    # shows the trend gate decision even if the Markdown file is lost.
    sys.stderr.write(
        "stealth-canary-trend: "
        + json.dumps(
            {
                "hard_fail": aggregate["hard_fail"],
                "hard_fail_labels": aggregate["hard_fail_labels"],
                "verdict_count": aggregate["verdict_count"],
            },
            separators=(",", ":"),
        )
        + "\n"
    )

    return 1 if aggregate["hard_fail"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
