//! trybuild UI tests — verify that the `#[derive(Extract)]` proc-macro emits
//! the expected compiler diagnostics.
//!
//! These tests require the `extract` feature and are compiled against the
//! `stygian-browser` library with that feature enabled.
//!
//! Run with `TRYBUILD=overwrite` on first execution (or after changing expected
//! error messages) to regenerate the reference snapshots.

/// Runs trybuild UI tests to verify the proc-macro emits expected compiler diagnostics.
#[test]
fn ui_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/extract_enum.rs");
    t.compile_fail("tests/ui/extract_missing_selector.rs");
}
