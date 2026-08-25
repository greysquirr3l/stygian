#!/usr/bin/env python3
"""Extract Rust code blocks from the mdbook into individual integration
tests in crates/stygian-book-tests/tests/.

Each block becomes a `#[test] fn snip_...()` whose body runs the snippet.
"""
import os
import re
from pathlib import Path

WORKSPACE_CRATES = (
    'stygian_graph', 'stygian_browser', 'stygian_proxy',
    'stygian_charon', 'stygian_mcp', 'stygian_plugin',
    'stygian_extract_derive',
)

ROOT = Path('/Users/ncampbell/Projects/rust/stygian')
BOOK = ROOT / 'book/src'
OUT = ROOT / 'crates/stygian-book-tests/tests'
OUT.mkdir(parents=True, exist_ok=True)


def slugify(s):
    return re.sub(r'[^a-z0-9]+', '_', s.lower()).strip('_')


def extract():
    snippets = []
    for md_path in sorted(BOOK.rglob('*.md')):
        rel = md_path.relative_to(BOOK)
        with open(md_path) as f:
            in_block = False
            block_lang = ''
            block_lines = []
            block_start = 0
            for i, line in enumerate(f.read().split('\n'), start=1):
                if not in_block and line.startswith('```'):
                    lang = line[3:].strip()
                    if lang.startswith('rust'):
                        # Skip blocks the doc author marked `no_run` —
                        # these are intentionally not executable in
                        # isolation (typically example APIs that need
                        # infrastructure not present in the test crate).
                        # `rust,ignore` is just an mdbook renderer
                        # directive (don't render/test in the book) and
                        # we still try to compile those.
                        block_skipped = 'no_run' in lang
                        block_lang = lang
                        block_lines = [line]
                        block_start = i
                        in_block = True
                elif in_block and line.startswith('```'):
                    if not block_skipped:
                        snippets.append({
                            'path': rel,
                            'start': block_start,
                            'lang': block_lang,
                            'lines': block_lines,
                        })
                    block_lang = ''
                    block_lines = []
                    in_block = False
                    block_skipped = False
                elif in_block:
                    block_lines.append(line)
    return snippets


def prelude_for(body):
    """Add `pub use <this_crate>::<mod>::*;` for any workspace crate
    referenced, where `<mod>` is the wrapper module that scopes the
    crate-local `Result` alias.

    Skip emitting a `use` line for a crate when the snippet already
    has a `use stygian_<crate>::...;` (or `use crate::...;` re-export)
    inside its body, to avoid the `unused_imports` lint.

    Test snippets can also use `pub use stygian_book_tests::*;` to get
    every crate accessible at root, but that brings in the local
    `Result` aliases. The wrapper-module form keeps `Result` scoped to
    the module, so `Result<(), Box<dyn std::error::Error>>` resolves
    to `std::result::Result` (the 2-generic std type).
    """
    used = set()
    for c in WORKSPACE_CRATES:
        if re.search(rf'\b{c}::', body):
            used.add(c)
    if re.search(r'\banyhow::', body):
        used.add('anyhow')
    mod_names = {
        'stygian_graph': 'graph',
        'stygian_browser': 'browser',
        'stygian_proxy': 'proxy',
        'stygian_charon': 'charon',
        'stygian_mcp': 'mcp',
        'stygian_plugin': 'plugin',
    }
    # Skip a crate if the snippet already has an explicit `use` of it
    # at the top level. The `use ...::{...}` form counts too. We don't
    # try to be clever about nested scopes.
    def snippet_uses(crate_name):
        # Match either `use stygian_graph::...;` or `use crate::...::*;
        # re-exports the same crate.
        return bool(re.search(
            rf'\buse\b\s+(?:crate::|stygian_book_tests::)?\s*'
            rf'{re.escape(crate_name)}\b',
            body,
        ))
    lines = []
    for c in sorted(used):
        if snippet_uses(c):
            continue
        mod_name = mod_names.get(c)
        if mod_name:
            lines.append(f'pub use stygian_book_tests::{mod_name}::*;')
    return lines


def _strip_comments(body):
    """Strip // line comments and /* ... */ block comments from a
    snippet body. Doc snippets often show intended code as a
    commented-out hint, and we don't want those hints to flip the
    detection of async/try-operator/etc.

    Caveat: `//` inside a string literal or URL (e.g. `https://`)
    is NOT a comment. We only strip `//` when it is preceded by
    whitespace OR start-of-line, AND not preceded by `:`.
    """
    # Remove block comments first (they can span lines).
    no_block = re.sub(r'/\*.*?\*/', '', body, flags=re.DOTALL)
    # Line comments: `//` not inside a string, not preceded by `:` (URL).
    # Naive but effective for mdbook snippets: strip from ` // ` or
    # leading `//` only (requires a whitespace or line-start before).
    # We approximate by stripping ` // ...` through end-of-line.
    lines = no_block.split('\n')
    cleaned = []
    for line in lines:
        # Find ` // ` (space-slash-slash-space) or leading `//`.
        idx = -1
        i = 0
        while i < len(line) - 1:
            if line[i] == '/' and line[i + 1] == '/' and (
                i == 0 or line[i - 1] in ' \t'
            ):
                idx = i
                break
            # Skip string literals.
            if line[i] in ('"', "'"):
                quote = line[i]
                i += 1
                while i < len(line) and line[i] != quote:
                    if line[i] == '\\':
                        i += 1
                    i += 1
            i += 1
        if idx >= 0:
            cleaned.append(line[:idx].rstrip())
        else:
            cleaned.append(line)
    return '\n'.join(cleaned)


def needs_async(body):
    """Detect whether the snippet is async-context. Looks for:
    - `.await` / `.await?` / `await?` chained
    - `async fn` / `async move`
    - any identifier that names an async function the doc is calling
      (best-effort: look for call sites that look like async methods).

    Comments (// and /* */) are stripped first so commented-out
    `// let x = foo().await?;` hint lines don't trigger detection.
    """
    stripped = _strip_comments(body)
    if '.await' in stripped:
        return True
    if 'async fn' in stripped or 'async move' in stripped or 'async {' in stripped:
        return True
    # Detect chained `await?` (common in docs without leading dot)
    if re.search(r'await\?', stripped):
        return True
    return False


def uses_try_operator(body):
    """Detect `?` at end-of-line OR `.await?` chained expressions in
    the snippet body. If the snippet doesn't define its own `fn` to
    anchor `?` to, the wrapper needs to return Result so `?` works."""
    stripped = _strip_comments(body)
    has_own_fn = bool(re.search(r'\bfn\s+\w', stripped))
    if has_own_fn:
        return False
    # Trailing `?` on its own line (the common case in docs).
    if re.search(r'\?\s*$', stripped, re.MULTILINE):
        return True
    # `.await?` chained form.
    if re.search(r'\.await\s*\?', stripped):
        return True
    return False


def has_own_main(body):
    """Return True if the snippet is a self-contained program with its
    own entry point (fn main, #[tokio::main], etc.)."""
    return bool(
        re.search(r'\bfn\s+main\b', body)
        or re.search(r'#\[tokio::main\]', body)
        or re.search(r'\basync\s+fn\s+main\b', body)
    )


def emit_snippet_test(snippet, shared_prelude_lines):
    body = '\n'.join(snippet['lines'][1:])
    rel = snippet['path']
    test_name = f'snip_{slugify(str(rel))}_l{snippet["start"]}'
    snippet_lang = snippet['lang']
    comment = (
        f'// Source: book/src/{rel}, line {snippet["start"]}\n'
        f'// Extracted from mdbook fence: `{snippet_lang}`\n'
    )

    prelude_lines = shared_prelude_lines + prelude_for(body)
    prelude = '\n'.join(prelude_lines)

    # Snippets with their own `fn main` are already complete programs.
    # We can't run them inside `#[test] fn ...()` (duplicate `main`), so
    # we just emit them at module scope. They'll be compiled but not
    # executed — the goal is to validate the types resolve, which is
    # what compile gives us.
    if has_own_main(body):
        indented_body = '\n'.join(
            ('    ' + line) if line else '' for line in body.split('\n')
        )
        return f'''{comment}
{prelude}

#[allow(dead_code)]
fn {test_name}_program() {{
{indented_body}
}}

#[test]
fn {test_name}() {{
    // Program-style snippet; compile-check only (call the function).
    let _ = {test_name}_program;
}}
'''

    async_ = needs_async(body)
    try_ = uses_try_operator(body)

    if async_ and try_:
        attr = '#[tokio::test]'
        kw = 'async'
        ret = ' -> Result<(), Box<dyn std::error::Error>>'
    elif async_:
        attr = '#[tokio::test]'
        kw = 'async'
        ret = ''
    elif try_:
        attr = '#[test]'
        kw = ''
        ret = ' -> Result<(), Box<dyn std::error::Error>>'
    else:
        attr = '#[test]'
        kw = ''
        ret = ''

    indented_body = '\n'.join(
        ('    ' + line) if line else '' for line in body.split('\n')
    )

    return f'''{comment}
{prelude}

{attr}
{kw} fn {test_name}(){ret} {{
{indented_body}
}}
'''


def main():
    snippets = extract()
    print(f'Found {len(snippets)} Rust code blocks.')

    shared_prelude = []  # lib.rs already re-exports workspace crates

    by_file = {}
    for s in snippets:
        by_file.setdefault(s['path'], []).append(s)

    for f in OUT.glob('snip_*.rs'):
        f.unlink()

    written = 0
    for rel, items in sorted(by_file.items()):
        slug = slugify(str(rel))
        out_path = OUT / f'snip_{slug}.rs'
        with open(out_path, 'w') as f:
            f.write('// Auto-generated by tools/mdbook-rust-prelude/extract_snippets.py.\n')
            f.write('// DO NOT EDIT BY HAND.\n\n')
            for s in items:
                f.write(emit_snippet_test(s, shared_prelude))
                written += 1
        print(f'  {rel}: {len(items)} snippets -> {out_path.name}')

    print(f'Wrote {written} tests across {len(by_file)} files.')


if __name__ == '__main__':
    main()