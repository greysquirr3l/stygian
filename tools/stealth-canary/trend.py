"""Stealth canary trend detection (T84).

Computes a rolling-window score regression detector for the stealth
canary workflow. The detector is **pure Python** (no external deps)
so it can run inside `.github/workflows/stealth-canary.yml` on the
default ubuntu-latest runner without installing anything.

## Semantics

For each required (non-advisory) canary target, the detector
evaluates the current score against two rules:

1. **Single-point regression** — the current score dropped below the
   rolling mean of the last ``config.window`` runs by more than
   ``config.delta_threshold``. Catches sudden drops.
2. **Monotonic regression** — the last ``config.monotonic_runs``
   scores have strictly decreased. Catches slow drift.

A target is hard-fail when **either** rule trips, when the probe
itself failed (``ok = false``), or when an optional per-target
baseline (e.g. ``STYGIAN_TIER1_BASELINE_CREEPJS``) was provided and
the current score is below the baseline.

When the history is too short to evaluate either rule the verdict
is ``insufficient_data`` — the target neither passes nor fails on
the trend axis (it still passes/fails on the probe and baseline
axes).

## Composition with T92's `CanaryTrendObservation`

The T92 `CanaryTrendObservation` is the trend seam exposed by the
JavaScript integrity trap canary. The detector reuses that seam
without re-implementing it: any `CanaryTrendObservation` JSON
records attached to a per-run probe entry are merged into the
trend history alongside the plain probe scores, and a signature
that climbs from `clean` → `suspected` → `confirmed` triggers
`monotonic_regression` for the parent target.

## Example

```python
from trend import TrendConfig, evaluate_trend, TrendStatus

config = TrendConfig()  # window=10, delta_threshold=0.05, monotonic_runs=3
history = [0.96, 0.95, 0.96, 0.95, 0.94, 0.95, 0.94, 0.93, 0.94, 0.93]
verdict = evaluate_trend(
    "synthetic-injection", history, current=0.93, config=config
)
assert verdict.status == TrendStatus.STABLE

# Monotonic regression: last 3 strictly decreasing
verdict = evaluate_trend(
    "synthetic-injection", history, current=0.90, config=config
)
assert verdict.status == TrendStatus.MONOTONIC_REGRESSION
```
"""

from __future__ import annotations

import json
import math
import os
from dataclasses import dataclass, field
from enum import Enum
from typing import Iterable, Sequence


class TrendStatus(str, Enum):
    """Coarse trend verdict for a single canary target.

    Members use stable `snake_case` values so the JSON output
    round-trips through downstream automation without remapping.
    """

    STABLE = "stable"
    REGRESSION_DETECTED = "regression_detected"
    MONOTONIC_REGRESSION = "monotonic_regression"
    INSUFFICIENT_DATA = "insufficient_data"


@dataclass(frozen=True)
class TrendConfig:
    """Detector knobs.

    All knobs are also wired to ``STYGIAN_TREND_*`` env vars so the
    CI workflow can override them per-run without editing the
    script. The defaults below match the values documented in
    `docs/stealth-canary-governance.md`.
    """

    window: int = 10
    delta_threshold: float = 0.05
    monotonic_runs: int = 3
    min_history: int = 3

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "TrendConfig":
        """Build a config from ``STYGIAN_TREND_*`` env vars.

        Falls back to the documented defaults when an env var is
        unset or unparseable. Invalid values (negative window,
        non-numeric threshold) are silently ignored so a bad
        override never blocks CI.
        """

        def _int(name: str, default: int) -> int:
            raw = (env or os.environ).get(name)
            if raw is None:
                return default
            try:
                value = int(raw)
            except ValueError:
                return default
            return value if value > 0 else default

        def _float(name: str, default: float) -> float:
            raw = (env or os.environ).get(name)
            if raw is None:
                return default
            try:
                value = float(raw)
            except ValueError:
                return default
            return value if 0.0 <= value <= 1.0 else default

        return cls(
            window=_int("STYGIAN_TREND_WINDOW", cls.window),
            delta_threshold=_float(
                "STYGIAN_TREND_DELTA_THRESHOLD", cls.delta_threshold
            ),
            monotonic_runs=_int(
                "STYGIAN_TREND_MONOTONIC_RUNS",
                cls.monotonic_runs,
            ),
            min_history=_int("STYGIAN_TREND_MIN_HISTORY", cls.min_history),
        )


@dataclass(frozen=True)
class TrendVerdict:
    """Verdict for a single canary target."""

    label: str
    current_score: float
    status: TrendStatus
    reason: str
    run_count: int
    rolling_mean: float | None = None
    delta: float | None = None
    consecutive_drops: int = 0
    baseline: float | None = None
    baseline_breach: bool = False
    observation_severity: str | None = None

    @property
    def is_hard_fail(self) -> bool:
        """True when the verdict should fail the workflow.

        The trend axis contributes a hard-fail on
        ``regression_detected`` or ``monotonic_regression``. The
        baseline axis contributes a hard-fail when an optional
        baseline was supplied and the current score is below it.
        ``insufficient_data`` and ``stable`` are not hard-fail.
        """

        if self.status in (
            TrendStatus.REGRESSION_DETECTED,
            TrendStatus.MONOTONIC_REGRESSION,
        ):
            return True
        return self.baseline_breach

    def to_dict(self) -> dict:
        """Serialise to a JSON-friendly dict.

        Keys are stable snake_case so downstream JSON consumers do
        not need to rename fields.
        """

        return {
            "label": self.label,
            "current_score": self.current_score,
            "status": self.status.value,
            "reason": self.reason,
            "run_count": self.run_count,
            "rolling_mean": self.rolling_mean,
            "delta": self.delta,
            "consecutive_drops": self.consecutive_drops,
            "baseline": self.baseline,
            "baseline_breach": self.baseline_breach,
            "observation_severity": self.observation_severity,
            "is_hard_fail": self.is_hard_fail,
        }


# ── Pure trend math ──────────────────────────────────────────────────────────


def _rolling_mean(history: Sequence[float], window: int) -> float:
    """Return the arithmetic mean of the last ``window`` items.

    Raises ``ValueError`` when ``history`` is empty.
    """

    if not history:
        raise ValueError("history must be non-empty")
    sample = history[-window:]
    return sum(sample) / len(sample)


def _consecutive_drops(scores: Sequence[float]) -> int:
    """Count trailing strictly-decreasing pairs in ``scores``.

    A strictly-decreasing pair ``scores[i-1] > scores[i]`` counts
    as one drop. The function returns the maximum number of drops
    observed at the **trailing** end of ``scores`` so callers can
    distinguish "the last 3 runs dropped" from "somewhere in the
    middle, scores dropped".

    For ``[0.88, 0.87, 0.86]`` the function returns 2 (both
    trailing pairs are strictly decreasing). For ``[0.95, 0.95,
    0.94]`` the function returns 1 (only the last pair is
    strictly decreasing — a tie at index 0 breaks the trailing
    streak).
    """

    if len(scores) < 2:
        return 0
    drops = 0
    # Walk backwards: pair (scores[i-1], scores[i]) for i = len-1, len-2, …, 1
    for i in range(len(scores) - 1, 0, -1):
        if scores[i - 1] > scores[i]:
            drops += 1
        else:
            break
    return drops


def evaluate_trend(
    label: str,
    history: Sequence[float],
    current: float,
    config: TrendConfig,
    *,
    baseline: float | None = None,
    observation_severity: str | None = None,
) -> TrendVerdict:
    """Evaluate the trend verdict for a single canary target.

    Parameters
    ----------
    label
        The canary target label (used in the verdict and the
        ``reason`` field for context).
    history
        Chronological list of historical scores (oldest first).
    current
        The score from the most recent run.
    config
        Detector knobs.
    baseline
        Optional pinned baseline score (e.g. from
        ``STYGIAN_TIER1_BASELINE_CREEPJS``). When supplied and the
        current score is below it, the verdict is hard-fail.
    observation_severity
        Optional ``clean`` / ``suspected`` / ``confirmed`` value
        from a T92 ``CanaryTrendObservation`` attached to the
        current run. Stored on the verdict for the Markdown
        summary; does not change the trend-axis verdict.

    Returns
    -------
    TrendVerdict
        Structured verdict. Callers should treat
        ``verdict.is_hard_fail`` as the workflow exit-code signal.
    """

    history_list = [float(s) for s in history]
    run_count = len(history_list) + 1  # history + current
    baseline_breach = baseline is not None and current < baseline

    # Not enough history to evaluate either trend rule.
    if len(history_list) < config.min_history:
        reason = (
            f"insufficient data: {len(history_list)} runs < "
            f"min_history={config.min_history}"
        )
        if baseline_breach:
            reason += f"; baseline={baseline:.4f} breached"
        return TrendVerdict(
            label=label,
            current_score=current,
            status=TrendStatus.INSUFFICIENT_DATA,
            reason=reason,
            run_count=run_count,
            baseline=baseline,
            baseline_breach=baseline_breach,
            observation_severity=observation_severity,
        )

    rolling = _rolling_mean(history_list, config.window)
    delta = current - rolling

    # Rule 1: monotonic regression (last N strictly decreasing).
    # Window is chronological: oldest of the last N first, current last.
    monotonic_window = [*history_list[-(config.monotonic_runs - 1) :], current]
    if len(monotonic_window) >= 2 and _consecutive_drops(monotonic_window) >= (
        config.monotonic_runs - 1
    ):
        consecutive = _consecutive_drops([*history_list, current])
        return TrendVerdict(
            label=label,
            current_score=current,
            status=TrendStatus.MONOTONIC_REGRESSION,
            reason=(
                f"monotonic regression: {consecutive} consecutive drops; "
                f"current={current:.4f}, rolling_mean={rolling:.4f}, "
                f"delta={delta:+.4f}"
            ),
            run_count=run_count,
            rolling_mean=rolling,
            delta=delta,
            consecutive_drops=consecutive,
            baseline=baseline,
            baseline_breach=baseline_breach,
            observation_severity=observation_severity,
        )

    # Rule 2: single-point drop
    if delta < -config.delta_threshold:
        return TrendVerdict(
            label=label,
            current_score=current,
            status=TrendStatus.REGRESSION_DETECTED,
            reason=(
                f"single-point regression: current={current:.4f} dropped "
                f"{abs(delta):.4f} below rolling_mean={rolling:.4f} "
                f"(delta={delta:+.4f}, threshold={config.delta_threshold:.4f})"
            ),
            run_count=run_count,
            rolling_mean=rolling,
            delta=delta,
            baseline=baseline,
            baseline_breach=baseline_breach,
            observation_severity=observation_severity,
        )

    # Stable
    reason = (
        f"stable: current={current:.4f} within {config.delta_threshold:.4f} "
        f"of rolling_mean={rolling:.4f} (delta={delta:+.4f})"
    )
    if baseline_breach:
        reason += f"; baseline={baseline:.4f} breached"
    return TrendVerdict(
        label=label,
        current_score=current,
        status=TrendStatus.STABLE,
        reason=reason,
        run_count=run_count,
        rolling_mean=rolling,
        delta=delta,
        baseline=baseline,
        baseline_breach=baseline_breach,
        observation_severity=observation_severity,
    )


# ── Per-target aggregation across the required set ───────────────────────────


def evaluate_per_target(
    history_by_label: dict[str, list[float]],
    current_by_label: dict[str, float],
    config: TrendConfig,
    *,
    baselines: dict[str, float] | None = None,
    observation_severity_by_label: dict[str, str] | None = None,
) -> dict[str, TrendVerdict]:
    """Evaluate the trend verdict for every required target.

    Parameters
    ----------
    history_by_label
        Map of label → chronological score list. Labels missing
        from the map but present in ``current_by_label`` are
        evaluated with an empty history (which yields
        ``insufficient_data``).
    current_by_label
        Map of label → current score. Labels in this map that are
        not in the required set are silently ignored.
    config
        Detector knobs.
    baselines
        Optional map of label → pinned baseline score. Mirrors
        the ``STYGIAN_TIER1_BASELINE_*`` env-var pattern.
    observation_severity_by_label
        Optional map of label → T92 ``CanaryTrendObservation``
        severity (`clean` / `suspected` / `confirmed`) attached
        to the current run.
    """

    baselines = baselines or {}
    observation_severity_by_label = observation_severity_by_label or {}
    verdicts: dict[str, TrendVerdict] = {}
    for label, current in current_by_label.items():
        verdicts[label] = evaluate_trend(
            label,
            history_by_label.get(label, []),
            current,
            config,
            baseline=baselines.get(label),
            observation_severity=observation_severity_by_label.get(label),
        )
    return verdicts


# ── History persistence ──────────────────────────────────────────────────────


@dataclass
class HistoryEntry:
    """One row in the trend history JSONL.

    Mirrors the per-run fields the workflow writes to
    ``history/canary-history.jsonl`` — the trend detector reads
    and writes this shape so the schema stays stable across runs.
    """

    label: str
    score: float
    threshold: float
    ok: bool
    run_id: str
    captured_at_epoch_ms: int
    trend_observations: list[dict] = field(default_factory=list)

    def to_jsonl(self) -> str:
        return json.dumps(self.__dict__, separators=(",", ":"))

    @classmethod
    def from_jsonl(cls, line: str) -> "HistoryEntry":
        data = json.loads(line)
        if not isinstance(data, dict):
            raise ValueError(f"history line is not an object: {line!r}")
        return cls(
            label=str(data.get("label", "")),
            score=float(data.get("score", 0.0)),
            threshold=float(data.get("threshold", 0.0)),
            ok=bool(data.get("ok", False)),
            run_id=str(data.get("run_id", "")),
            captured_at_epoch_ms=int(data.get("captured_at_epoch_ms", 0)),
            trend_observations=list(data.get("trend_observations", [])),
        )


def read_history(path: str) -> list[HistoryEntry]:
    """Read the trend history JSONL file.

    Returns an empty list when the file is missing. Malformed
    lines raise ``ValueError`` — the history file is the source
    of truth for the trend signal, so a corrupt history is a
    hard error.
    """

    entries: list[HistoryEntry] = []
    try:
        with open(path, "r", encoding="utf-8") as handle:
            for raw in handle:
                line = raw.strip()
                if not line:
                    continue
                entries.append(HistoryEntry.from_jsonl(line))
    except FileNotFoundError:
        return []
    return entries


def write_history(path: str, entries: Iterable[HistoryEntry]) -> int:
    """Append the supplied entries to the history JSONL.

    Returns the number of entries written. Creates the parent
    directory if it does not exist.
    """

    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    written = 0
    with open(path, "a", encoding="utf-8") as handle:
        for entry in entries:
            handle.write(entry.to_jsonl())
            handle.write("\n")
            written += 1
    return written


def history_to_score_map(
    entries: Sequence[HistoryEntry],
    label: str,
) -> list[float]:
    """Extract the chronological score list for a single label."""

    return [e.score for e in entries if e.label == label]


def severity_for_label(
    entries: Sequence[HistoryEntry],
    label: str,
) -> str | None:
    """Return the most-recent T92 ``CanaryTrendObservation`` severity
    attached to the most recent history entry for ``label``.

    Returns ``None`` when no observation was attached. The
    severity is one of ``clean`` / ``suspected`` / ``confirmed``
    per the T92 trend.rs contract.
    """

    for entry in reversed(entries):
        if entry.label != label:
            continue
        for obs in entry.trend_observations:
            severity = obs.get("severity")
            if isinstance(severity, str):
                return severity
        return None
    return None


# ── Aggregation across the canary run ────────────────────────────────────────


def aggregate_verdict(verdicts: dict[str, TrendVerdict]) -> dict:
    """Aggregate per-target verdicts into a single run-level summary.

    Returns a dict with a stable shape so the workflow can render
    a Markdown summary and `jq` consumers can extract fields.
    """

    hard_fails = [v.label for v in verdicts.values() if v.is_hard_fail]
    return {
        "verdicts": {label: v.to_dict() for label, v in verdicts.items()},
        "hard_fail": bool(hard_fails),
        "hard_fail_labels": hard_fails,
        "verdict_count": len(verdicts),
    }


def approx_equal(a: float, b: float, tol: float = 1e-9) -> bool:
    """Float equality with a 1e-9 tolerance.

    Used by the unit tests so that exact equality on scores
    survives the 1e-9 floating-point noise introduced by the
    rolling-mean arithmetic.
    """

    if math.isnan(a) or math.isnan(b):
        return math.isnan(a) and math.isnan(b)
    return abs(a - b) < tol
