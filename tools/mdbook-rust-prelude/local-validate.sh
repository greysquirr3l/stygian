#!/usr/bin/env bash
# Local validation of mdbook snippets against real workspace APIs.
#
# Extracts every ```rust fence from book/src/*.md into per-fence
# integration tests in crates/stygian-book-tests/tests/, then runs
# `cargo test -p stygian-book-tests`. This replaces mdbook-test as
# the snippet compile-validation strategy.
#
# Usage:
#   ./tools/mdbook-rust-prelude/local-validate.sh
#
# After running, see the failure list with:
#   cargo test -p stygian-book-tests --no-run 2>&1 | grep "could not compile"

set -euo pipefail

cd "$(dirname "$0")/../.."

echo "==> Extracting snippets from book/src into integration tests..."
python3 tools/mdbook-rust-prelude/extract_snippets.py

echo ""
echo "==> Compiling stygian-book-tests (no run)..."
cargo build -p stygian-book-tests --tests 2>&1 | tail -20

echo ""
echo "==> Running stygian-book-tests..."
cargo test -p stygian-book-tests 2>&1 | tail -30

echo ""
echo "==> Summary:"
echo "    Total snippet tests: $(ls crates/stygian-book-tests/tests/snip_*.rs 2>/dev/null | wc -l | tr -d ' ')"
echo ""
echo "If any test files fail to compile, edit the corresponding book/src/*.md"
echo "file (and re-run this script) to fix the snippet."