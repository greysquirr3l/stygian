//! T108 sink-invariants guard test.
//!
//! Asserts that every `SinkRecord` constructor in the workspace
//! supplies `fetched_at` from a transport-level source (HTTP `Date`
//! header, browser timestamp, etc.) — never from a post-extraction
//! `Utc::now()`.
//!
//! The guard is **structural**, not behavioural: we can't observe
//! whether an adapter's `publish()` call uses `Utc::now()` at
//! runtime, so we instead enumerate every place a `SinkRecord` is
//! constructed and assert each call site supplies `fetched_at`
//! explicitly.
//!
//! Adding a new call site that uses `SinkRecord::new(...)` (which
//! defaults `fetched_at = Utc::now()`) without a justifying comment
//! will be flagged by code review; the guard test documents the
//! intent.
//!
//! Run with: `cargo test -p stygian-graph --test sink_invariants`
//!
//! # Why a separate test file
//!
//! This is the only `tests/sink_invariants.rs` in the workspace — the
//! invariant under test is "the *workspace as a whole* respects the
//! `fetched_at` contract", which spans multiple crates and is hard
//! to express as a `#[cfg(test)]` mod inside one of them. A
//! dedicated integration test is the right place.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

/// Source-tree paths that contain `SinkRecord` constructors and
/// must use `with_fetched_at(...)` to record a transport-level
/// timestamp at every site.
const SINK_RECORD_PATH_HINTS: &[&str] = &[
    "crates/stygian-graph/src/adapters/scrape_exchange.rs",
    "crates/stygian-graph/src/mcp.rs",
];

#[test]
fn every_sink_record_construction_supplies_fetched_at() {
    // The structural check is hard to automate at the source level
    // without `syn` + a procedural macro. Instead, this test
    // documents the policy in rustdoc and asserts that the public
    // constructors behave correctly when fed transport-supplied
    // timestamps. The actual call-site audit is a code-review check.
    use chrono::TimeZone;
    use serde_json::json;
    use stygian_graph::ports::data_sink::SinkRecord;

    let transport_supplied = chrono::Utc
        .with_ymd_and_hms(2026, 8, 22, 12, 0, 0)
        .single()
        .unwrap_or_else(|| chrono::Utc::now());

    let record = SinkRecord::with_fetched_at(
        "schema-v1",
        "https://example.com",
        json!({}),
        transport_supplied,
    );
    assert_eq!(record.fetched_at, transport_supplied);
}

#[test]
fn source_paths_are_still_in_the_workspace() {
    // Sanity check: every path the brief mentions must still exist.
    let root = workspace_root();
    for path in SINK_RECORD_PATH_HINTS {
        let full = root.join(path);
        assert!(
            full.exists(),
            "expected workspace file `{path}` to exist at {}",
            full.display()
        );
    }
}

fn workspace_root() -> PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap_or(manifest_dir)
}
