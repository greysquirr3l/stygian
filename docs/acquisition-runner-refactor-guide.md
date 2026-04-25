# Acquisition Runner Refactor Guide (T59-T63)

## Audience and Purpose

This document explains the recently completed acquisition-runner refactor for contributors who were not involved in implementation.

It focuses on:

- what changed and why
- where changes landed by crate
- compatibility guarantees
- how to use the new behavior
- how to validate and extend it safely

## Executive Summary

The refactor introduced an opinionated acquisition runner that improves hard-target scraping ergonomics without changing workspace layout or breaking existing graph behavior.

The completed tasks were:

- T59: acquisition runner core in stygian-browser
- T60: deterministic charon runtime-policy mapping
- T61: high-level MCP tool browser_acquire_and_extract
- T62: optional, additive stygian-graph bridge
- T63: runner-first docs and compatibility checks

The key outcome is a clear default execution path for difficult targets while preserving opt-in boundaries for downstream users.

## Why This Refactor Was Done

Before this refactor, users often had to understand and manually combine lower-level controls early, even for common acquisition workflows. The new runner-first path compresses intent-to-outcome and standardizes failure reporting.

Design goals that drove implementation:

- keep crate layout unchanged
- keep stygian-graph behavior stable by default
- make new graph behavior additive and opt-in
- provide deterministic failure shapes for automation
- preserve advanced knobs as secondary paths

## Architecture Decision Record (Short Form)

### Decision 1: Put orchestration in stygian-browser

The acquisition ladder belongs close to browser session lifecycle and extraction primitives. This keeps orchestration thin and avoids duplicating runtime behavior across crates.

### Decision 2: Keep charon mapping pure and deterministic

Runtime policy mapping was isolated as pure conversion logic. This prevents hidden coupling and makes behavior predictable for tests and downstream tools.

### Decision 3: Expose one high-level MCP entry point

browser_acquire_and_extract is the operator-friendly wrapper around the runner. It enables one-call acquisition plus extraction with stable diagnostics.

### Decision 4: Keep graph bridge opt-in

No existing graph pipeline changes behavior unless a browser node explicitly opts in with an acquisition block and the acquisition-runner feature is enabled.

## What Changed by Crate

## stygian-browser

Primary additions and behavior:

- acquisition runner core added
- deterministic strategy ladder execution
- terminal result object for success and failure paths
- stable failure bundle and diagnostics fields

Notable implementation details:

- timeout and setup-failure paths are represented as terminal structured results
- sticky browser retries can pin by host via browser-pool context acquisition

Main touched areas:

- crates/stygian-browser/src/acquisition.rs
- crates/stygian-browser/src/mcp.rs
- crates/stygian-browser/tests/mcp_integration.rs

## stygian-charon

Primary additions and behavior:

- runtime-policy to acquisition mapping layer
- deterministic mapping helpers and explicit defaulting
- risk/clamp handling to avoid undefined transitions

Main touched areas:

- crates/stygian-charon/src/acquisition.rs
- crates/stygian-charon/src/lib.rs
- crates/stygian-charon/src/types.rs

## stygian-graph

Primary additions and behavior:

- optional bridge to run browser nodes through acquisition runner
- guarded by acquisition-runner feature
- browser nodes only route to the bridge when the node has an acquisition block

Compatibility behavior:

- if acquisition block is absent, prior browser-node skip semantics remain
- existing pipelines remain non-breaking by default

Main touched areas:

- crates/stygian-graph/Cargo.toml
- crates/stygian-graph/src/mcp.rs

## Documentation and examples

Primary additions and behavior:

- MCP docs updated to reflect pipeline_run browser opt-in behavior
- browser acquisition example added
- production security baseline added for MCP operations

Main touched areas:

- book/src/mcp/graph-tools.md
- book/src/mcp/overview.md

## New or Updated Runtime Contracts

## Runner modes

Supported high-level modes:

- fast
- resilient
- hostile
- investigate

These are now the documented and tested operator-facing strategy selectors.

## MCP tool contract

High-level tool:

- browser_acquire_and_extract

Contract characteristics:

- validates mode strings explicitly
- emits compact diagnostics for failure analysis
- preserves deterministic output shape for downstream automation

Diagnostics fields include:

- attempted
- timed_out
- failure_count
- failures

## Graph bridge contract

Bridge activation requires both:

- acquisition-runner feature in stygian-graph build
- node-level acquisition opt-in block in pipeline TOML

Without both conditions, the browser node is skipped — behavior remains legacy and non-breaking.
If the feature is not compiled in but a node carries an `acquisition` block, the block is ignored
and the node is still recorded in the `skipped` list, not `errors`.

## Migration Guidance for Developers

Most users do not need migration changes unless they want runner behavior in graph pipelines.

## If you use MCP directly

Recommended path:

- adopt browser_acquire_and_extract for runner-first workflows
- keep lower-level browser tools for advanced or debugging flows

## If you use stygian-graph pipelines

To opt into acquisition runner on browser nodes:

1. enable acquisition-runner feature
2. add node-level acquisition block
3. validate pipeline behavior in CI for both legacy and opt-in paths

Example opt-in block:

```toml
[nodes.params.acquisition]
enabled = true
mode = "resilient"
wait_for_selector = "main"
total_timeout_secs = 45
```

## Compatibility Guarantees and Boundaries

Guaranteed by this refactor:

- additive graph integration only
- legacy graph behavior preserved by default
- no crate-layout changes
- deterministic runner/MCP failure structures

Not guaranteed:

- automatic behavioral changes to existing graph browser nodes
- blanket migration of all low-level browser workflows to runner-first APIs

## Verification and CI Expectations

Recommended validation sequence:

1. cargo build --workspace --all-features
2. cargo test --workspace --all-features
3. cargo clippy --workspace --all-features -- -D warnings

For graph users, add matrix checks where practical:

- legacy path (without acquisition-runner feature)
- opt-in bridge path (with acquisition-runner and acquisition block)

## Extending This Refactor Safely

Follow these guardrails for future changes:

- keep runner outputs deterministic and machine-consumable
- preserve additive-only behavior in stygian-graph
- keep policy mapping pure in stygian-charon
- avoid coupling MCP protocol concerns into runtime internals
- preserve no-unwrap/no-expect policy in library code

When adding new runner modes or diagnostics:

- update MCP schema/docs and integration tests together
- add at least one failure-path test and one success-path test
- document compatibility impact explicitly

## Quick File Map

- Refactor plan: plan-refactor.toml
- Progress status: PROGRESS.md
- Browser runner core: crates/stygian-browser/src/acquisition.rs
- MCP high-level tool: crates/stygian-browser/src/mcp.rs
- Charon mapping: crates/stygian-charon/src/acquisition.rs
- Optional graph bridge: crates/stygian-graph/src/mcp.rs
- Graph MCP docs: book/src/mcp/graph-tools.md
- MCP security baseline docs: book/src/mcp/overview.md

## Known Operational Benefits After Refactor

- clearer default path for hard-target acquisition
- fewer manual low-level tool combinations for common cases
- more stable diagnostics for automation and incident triage
- safer downstream adoption due to explicit opt-in boundaries

## Open Follow-Ups (Optional)

Potential future improvements that are compatible with this design:

- expand runner mode guidance with target-type playbooks
- add more pipeline examples that compare legacy vs opt-in bridge behavior
- add docs page dedicated to diagnostics interpretation and failure triage
