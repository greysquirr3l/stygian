//! trybuild UI tests — verify that the `#[derive(Extract)]` proc-macro emits
//! the correct `compile_error!` diagnostics for invalid inputs.
//!
//! These tests require the `extract` feature and are compiled against the
//! `stygian-browser` library with that feature enabled.
//!
//! Run with `TRYBUILD=overwrite` on first execution (or after changing error
//! messages) to regenerate the `.stderr` snapshot files.

/// Verify that applying `#[derive(Extract)]` to an enum and applying it to a
/// struct with a field missing `#[selector(...)]` both produce the expected
/// compiler diagnostics.
#[test]
fn ui_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/extract_enum.rs");
    t.compile_fail("tests/ui/extract_missing_selector.rs");
}
