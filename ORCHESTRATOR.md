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

### Preflight (strict workspace, pedantic + nursery + perf)

The workspace clippy baseline is at **zero errors**. T95 closed the
remaining ~255 pre-existing baseline errors and the strict clippy
sweep closed an additional ~293 pedantic/nursery/perf lints. Preflight
is now **strict workspace** — no per-crate carve-out, no baseline
tolerance. Every subagent must clear all three gates:

```bash
# 1. Workspace build + tests must always pass with all features.
cargo build --workspace --all-features && \
  cargo test --workspace --all-features

# 2. Strict workspace clippy with the full pedantic + nursery + perf
#    profile. `--all-features` is REQUIRED — feature-gated code paths
#    (e.g. `#[cfg(feature = "metrics")]` impls) can carry lints that
#    only fire when the feature is enabled. The canonical command is
#    also exposed as `cargo l` in `.cargo/config.toml`.
CARGO_TERM_COLOR=never cargo clippy --workspace --all-features \
  --all-targets -- \
  -W clippy::all -W clippy::pedantic -W clippy::nursery \
  -W clippy::cargo -W clippy::perf \
  -A clippy::module_name_repetitions \
  -A clippy::must_use_candidate \
  -A clippy::missing_errors_doc \
  -A clippy::missing_panics_doc \
  -A clippy::struct_excessive_bools \
  -A clippy::multiple_crate_versions \
  -D clippy::unwrap_used \
  -D clippy::expect_used \
  -D clippy::panic \
  -D clippy::indexing_slicing \
  -D clippy::cast_ptr_alignment \
  -D clippy::suspicious \
  -D warnings 2>&1 | tee /tmp/clippy.txt
ERRORS=$(grep -c "^error" /tmp/clippy.txt || true)
# ERRORS must be 0. If higher, fix and rerun.
```

**Subagent policy:**

- The touched crate is identified from the task file's `## Feature
  flag` section and interpolated as `TASK_CRATE` in the subagent
  prompt. The subagent uses the strict workspace clippy command
  above (NOT a per-crate relaxed variant).
- If the subagent reports a clippy baseline error that is clearly
  unrelated to its work, it must still fix it as part of its task —
  the strict-workspace policy does not allow deferring baseline work.
- All test groups must pass (live `#[ignore]`-gated tests are exempt).
- The `cargo l` alias in `.cargo/config.toml` is the canonical
  shorthand for the strict clippy command above. Use it locally; the
  full command is required in CI for clarity.

**Why `--all-features` matters:**
A feature-gated `impl` block (e.g. `#[cfg(feature = "metrics")] impl
MetricsCollector { fn foo(&self) { Self::bar(self); } }`) will only
be compiled when the feature is enabled. Without `--all-features`,
clippy never sees that code, so `clippy::use_self` and similar lints
can hide in the gated path until CI runs the feature build. The
canonical `cargo l` alias always passes `--all-features`; do not
remove it.

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
