#!/usr/bin/env python3
"""Validate stealth canary configs (T84).

Validates:

1. ``.github/stealth-canary.toml`` — shape and invariants of the
   per-run probe target list (advisory flag, threshold range,
   required non-advisory entry).
2. ``tools/stealth-canary/data/required-targets.toml`` — shape
   and invariants of the required (non-advisory, hard-fail)
   canary target set with ownership / runbook / artifact
   pointers.
3. The label overlap between the two files: every
   ``[[required]]`` entry must reference a label that is
   declared in ``[[canary]]`` (the workflow will not probe a
   target that is required but not configured).

Exits non-zero on the first validation failure.
"""

from __future__ import annotations

import pathlib
import sys
import tomllib
from typing import Any, NoReturn

REQUIRED_TARGETS_PATH = pathlib.Path("tools/stealth-canary/data/required-targets.toml")
CANARY_CONFIG_PATH = pathlib.Path(".github/stealth-canary.toml")


def fail(message: str) -> NoReturn:
    """Print a user-facing validation error and exit non-zero."""
    print(f"stealth-canary config error: {message}", file=sys.stderr)
    raise SystemExit(1)


def _load_toml(path: pathlib.Path) -> dict[str, Any]:
    if not path.exists():
        fail(f"missing config file: {path}")
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        fail(f"invalid TOML in {path}: {exc}")
    return {}  # unreachable: fail() raises


def _validate_canary_config(path: pathlib.Path) -> list[str]:
    """Validate `.github/stealth-canary.toml`; return the list of labels."""
    data = _load_toml(path)
    canaries_raw: Any = data.get("canary")
    if not isinstance(canaries_raw, list) or not canaries_raw:
        fail(f"{path}: expected at least one [[canary]] entry")

    has_non_advisory = False
    labels: list[str] = []
    for idx, entry in enumerate(canaries_raw, start=1):
        if not isinstance(entry, dict):
            fail(f"{path} entry #{idx} must be a table")

        url = entry.get("url")
        if not isinstance(url, str) or not url.strip():
            fail(f"{path} entry #{idx} requires non-empty string field 'url'")

        label = entry.get("label")
        if not isinstance(label, str) or not label.strip():
            fail(f"{path} entry #{idx} requires " "non-empty string field 'label'")
        labels.append(label)

        threshold = entry.get("threshold", 0.90)
        if not isinstance(threshold, (int, float)):
            fail(f"{path} entry #{idx} field 'threshold' must be numeric")
        threshold = float(threshold)
        if threshold < 0.0 or threshold > 1.0:
            fail(f"{path} entry #{idx} field 'threshold' " "must be within [0.0, 1.0]")

        advisory = entry.get("advisory", False)
        if not isinstance(advisory, bool):
            fail(
                f"{path} entry #{idx} field 'advisory' " "must be boolean when present"
            )

        if not advisory:
            has_non_advisory = True

    if not has_non_advisory:
        fail(f"{path}: at least one non-advisory canary entry is required")

    return labels


def _validate_required_targets(path: pathlib.Path) -> list[str]:
    """Validate the required-targets data file; return the list of labels."""
    data = _load_toml(path)
    raw_entries: Any = data.get("required")
    if not isinstance(raw_entries, list) or not raw_entries:
        fail(f"{path}: expected at least one [[required]] entry")

    labels: list[str] = []
    for idx, entry in enumerate(raw_entries, start=1):
        if not isinstance(entry, dict):
            fail(f"{path} entry #{idx} must be a table")

        label = entry.get("label")
        if not isinstance(label, str) or not label.strip():
            fail(f"{path} entry #{idx} requires " "non-empty string field 'label'")
        labels.append(label)

        url = entry.get("url")
        if not isinstance(url, str) or not url.strip():
            fail(f"{path} entry #{idx} requires non-empty string field 'url'")

        threshold = entry.get("threshold")
        if not isinstance(threshold, (int, float)):
            fail(f"{path} entry #{idx} field 'threshold' must be numeric")
        threshold = float(threshold)
        if threshold < 0.0 or threshold > 1.0:
            fail(f"{path} entry #{idx} field 'threshold' " "must be within [0.0, 1.0]")

        description = entry.get("description")
        if not isinstance(description, str) or not description.strip():
            fail(
                f"{path} entry #{idx} requires non-empty " "string field 'description'"
            )

        owner = entry.get("owner")
        if not isinstance(owner, str) or not owner.strip():
            fail(f"{path} entry #{idx} requires " "non-empty string field 'owner'")

        runbook = entry.get("runbook")
        if not isinstance(runbook, str) or not runbook.strip():
            fail(f"{path} entry #{idx} requires " "non-empty string field 'runbook'")

        artifacts = entry.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts:
            fail(
                f"{path} entry #{idx} field 'artifacts' "
                "must be a non-empty list of strings"
            )
        for art_idx, artifact in enumerate(artifacts):
            if not isinstance(artifact, str) or not artifact.strip():
                fail(
                    f"{path} entry #{idx} artifacts[{art_idx}] "
                    "must be a non-empty string"
                )

        # `secondary` is optional; when present it must be non-empty.
        secondary = entry.get("secondary")
        if secondary is not None and (
            not isinstance(secondary, str) or not secondary.strip()
        ):
            fail(
                f"{path} entry #{idx} field 'secondary' "
                "must be a non-empty string when present"
            )

        # `baseline` is optional; when present it must be in [0.0, 1.0].
        baseline = entry.get("baseline")
        if baseline is not None:
            if not isinstance(baseline, (int, float)):
                fail(
                    f"{path} entry #{idx} field 'baseline' "
                    "must be numeric when present"
                )
            baseline = float(baseline)
            if baseline < 0.0 or baseline > 1.0:
                fail(
                    f"{path} entry #{idx} field 'baseline' " "must be within [0.0, 1.0]"
                )

    return labels


def _validate_label_overlap(
    canary_labels: list[str], required_labels: list[str]
) -> None:
    """Every required label must be declared as a probe target."""
    canary_set = set(canary_labels)
    missing = [label for label in required_labels if label not in canary_set]
    if missing:
        fail(
            "required-targets entries reference labels not declared in "
            f".github/stealth-canary.toml: {missing}. "
            "Add a matching [[canary]] "
            "entry or remove the [[required]] entry."
        )


def main() -> None:
    """Validate the canary config and the required-targets data file."""
    canary_labels = _validate_canary_config(CANARY_CONFIG_PATH)
    required_labels = _validate_required_targets(REQUIRED_TARGETS_PATH)
    _validate_label_overlap(canary_labels, required_labels)
    print(
        f"validated {len(canary_labels)} canary entries and "
        f"{len(required_labels)} required-targets entries "
        "(label overlap: "
        f"{len(set(canary_labels) & set(required_labels))}/"
        f"{len(required_labels)})"
    )


if __name__ == "__main__":
    main()
