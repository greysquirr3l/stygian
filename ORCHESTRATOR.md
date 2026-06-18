---
description: opencode orchestrator for the Ralph Wiggum loop — drives subagents to implement all stygian tasks
---

# Stygian Ralph Wiggum Orchestrator (opencode)

<PLAN>./IMPLEMENTATION_PLAN.md</PLAN>
<TASKS>./tasks</TASKS>
<PROGRESS>./PROGRESS.md</PROGRESS>

## Purpose

This file drives the opencode-based Ralph Wiggum development loop for the
stygian project. The orchestrator (this opencode session) reads PROGRESS.md,
finds the next pending task whose dependencies are satisfied, and spawns an
implementation subagent. The subagent implements the task, runs preflight,
updates PROGRESS.md, and (if enabled) commits. The orchestrator verifies and
repeats.

**You (this session) do NOT implement code yourself. You only spawn
subagents and verify their output.**

## Setup

1. Read PROGRESS.md to understand current state.
2. If PROGRESS.md does not exist, fail immediately — it should have been
   created by Wiggum.

## Implementation loop

Repeat until **every task in PROGRESS.md is `[x]` across all phases**:

1. Read PROGRESS.md and the "Priority hint" section in the active phase.
2. Find the highest-priority task that is `[ ]` and whose dependencies (per
   the `## Depends on` section of its task file) are all `[x]`. If
   PROGRESS.md lists a "First wave", prefer tasks in that wave.
3. Mark the chosen task `[~]` in PROGRESS.md.
4. **Read the Accumulated Learnings section** of PROGRESS.md — apply any
   relevant insights to the subagent prompt.
5. Spawn an implementation subagent using the `task` tool with
   `subagent_type: "general"` and the SUBAGENT_PROMPT below, interpolating
   the task ID, task file path, and current phase.
6. Wait for the subagent to complete.
7. Read PROGRESS.md again.
8. Verify the task is now `[x]` and that preflight passed. If it is not,
   mark it `[!]` and output a warning, then continue to the next available
   task.
9. Repeat.

When every task across all phases is `[x]`, output:

```
✅ All stygian implementation tasks complete.
```

## Spawning implementation subagents

Use opencode's `task` tool:

- `subagent_type: "general"` — the implementation subagent needs read, write,
  edit, glob, grep, and bash access. (opencode's `explore` type is
  read-only and cannot be used here.)
- `description: "Implement T{NN} — {task title}"` — short label.
- `prompt:` — the SUBAGENT_PROMPT below with `TASK_ID`, `TASK_FILE`, and
  the workspace root interpolated.

If the `task` tool is unavailable in this session, fail immediately with:

```
⛔ opencode `task` tool is not available. Run this prompt inside an opencode
session with the `general` subagent type enabled.
```

## SUBAGENT_PROMPT

You are a senior Rust systems architect specializing in hexagonal
architecture, high-concurrency I/O, and trait-based plugin systems.

### Your context

- Project plan: `./IMPLEMENTATION_PLAN.md`
- Progress tracker: `./PROGRESS.md`
- Task files: `./tasks/`
- Your assigned task: **TASK_ID** (file: TASK_FILE)
- Workspace root: `./` (treat this as the cargo workspace root)

### Your job

1. Read PROGRESS.md.
2. Read the "Accumulated Learnings" section — apply relevant insights from
   prior tasks.
3. Confirm the dependencies of TASK_ID are all `[x]` in PROGRESS.md. If any
   are not, stop and report a dependency violation. Do not start work.
4. Mark TASK_ID `[~]` in PROGRESS.md immediately.
5. Read the corresponding task file at TASK_FILE. Read the full task spec,
   including the `## Feature flag` and `## Exit criteria` sections.
6. Implement the task completely — create all files, write all code, add all
   tests, and follow the project's AGENTS.md rules. **Implement THIS TASK
   ONLY.** Do not touch code from other tasks.
7. Run the preflight check (see "Preflight" section below for the current
   policy, including the per-crate clippy split). Fix all errors and
   warnings until preflight passes.
8. Verify every checkbox in the task file's `## Exit criteria` section is
   met. If a criterion cannot be met, mark it `[!]` in the task file and
   surface it in your final report — do not silently skip.
9. Update PROGRESS.md: change `[~]` to `[x]` for this task.
10. Append any learnings to the "Accumulated Learnings" section in
    PROGRESS.md. Format: `- T{NN}: {what you learned}`.
11. **Commit policy:** prefer to leave the commit to the human reviewer. Only
    auto-commit if the user has explicitly enabled auto-commit in the
    orchestrator session. If you do commit, use a conventional commit
    message focused on user impact (not file counts or line numbers), and
    never force-push, amend a pushed commit, or skip hooks.
12. Stop. Do not start the next task — the orchestrator will do that.

### Preflight (per-crate, baseline-tolerant)

The workspace currently has **~104 pre-existing `clippy::must_use_candidate`
and related baseline errors concentrated in `stygian-browser`** (session.rs,
recorder.rs, page.rs, mcp.rs, etc.) and other crates. These are NOT
introduced by Phase 13 tasks. A separate baseline-cleanup subagent task is
tracked in PROGRESS.md and will be dispatched at a natural break point.

Until that runs, the preflight is **per-crate and baseline-tolerant**:

```bash
# 1. Workspace build + tests must always pass
cargo build --workspace --all-features && \
  cargo test --workspace --all-features

# 2. Per-crate clippy: count errors before and after. The task is gated
#    on "no new errors introduced" — not on zero total errors.
CARGO_TERM_COLOR=never cargo clippy -p TASK_CRATE \
  --all-features --all-targets -- -D warnings 2>&1 | tee /tmp/clippy.txt
ERRORS=$(grep -c "^error" /tmp/clippy.txt || true)
# Subagent must report ERRORS in its final report. The orchestrator
# compares against the tracked baseline for the touched crate.
```

**Baseline error counts (tracked, 2026-06-17):**
- `stygian-browser`: ~104 (pre-Phase 13)
- `stygian-charon`: 0
- `stygian-proxy`: 0
- `stygian-plugin`: 0
- `stygian-graph`: 0

If a task introduces *new* errors beyond its crate's baseline, the
subagent must fix them. Total errors should be ≤ baseline. Touched-code
clippy-clean status (no new errors in newly added files/modules) is the
strict gate.

When the baseline-cleanup subagent lands and the workspace clippy is
green, the orchestrator will revert to the original strict
`--workspace -D warnings` preflight.

### Rules (from AGENTS.md)

- Rust edition 2024, stable toolchain (1.94.0).
- Hexagonal Architecture: domain core isolated from infrastructure; all
  external interactions go through port traits.
- Workspace structure: domain (business logic), ports (trait definitions),
  adapters (implementations), application (orchestration).
- Domain layer NEVER imports from adapters; use dependency inversion via
  ports.
- Use Rust 1.94.0 features: async closures, trait upcasting,
  `LazyCell`/`LazyLock`, let chains.
- Use native `async fn` in traits for the plugin interface.
- All error types must use `thiserror`; `anyhow` is reserved for CLI entry
  points only.
- No `.unwrap()` or `.expect()` in library code; use exhaustive error
  handling.
- Library versions: tokio 1.49, reqwest 0.13, serde 1.0, sqlx 0.8.
- Feature-gate optional dependencies: `redis` behind `redis`, object
  storage behind `object-storage`, etc.
- Idempotence: all operations must be safely retryable with idempotency
  keys.
- Security-first: authorization checks at repository level, fail-secure by
  default.
- Documentation: every public trait and method must have a doc comment with
  an example.
- Every new adapter MUST implement the corresponding port trait AND
  `ScrapingService` where appropriate.
- Cross-platform: handle path separators, case sensitivity, and encoding
  differences across OS targets.

### Architecture: Hexagonal

- Domain layer must have zero I/O dependencies.
- All external interactions go through port traits.
- Adapters implement port traits and live in `adapters/`.
- New capabilities require a new port trait before an adapter.
- Depend inward: adapters → ports ← domain.
