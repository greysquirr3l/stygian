//! Fuzz target for `stygian_charon::har::parse_har_transactions`.
//!
//! The HAR parser takes untrusted JSON input from third-party HTTP
//! Archive recordings (browser DevTools exports, proxy captures,
//! anti-bot telemetry). It enforces size and entry-count limits to
//! prevent resource-exhaustion, so this fuzz target's job is to
//! surface panics, infinite loops, or out-of-bounds reads at the
//! boundary between "shape looks plausible" and "limits kick in".
//!
//! Build with `cargo +nightly fuzz run parse_har --fuzz-dir crates/stygian-charon/fuzz`.
//! See `.github/workflows/fuzz.yml` for the nightly CI integration.

#![no_main]

use libfuzzer_sys::fuzz_target;

use stygian_charon::har::parse_har_transactions;

fuzz_target!(|data: &[u8]| {
    // The parser is `fn parse_har_transactions(har_json: &str) -> Result<ParsedHar, HarError>`,
    // so we wrap the fuzzer's &[u8] in a `&str` via `std::str::from_utf8` to mirror
    // how a real caller would feed the parser (UTF-8 JSON). Non-UTF-8 inputs are
    // short-circuited by `from_utf8` and uninteresting; panics / OOM / hangs in
    // the actual parser are the bugs we want to find.
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_har_transactions(s);
    }
});