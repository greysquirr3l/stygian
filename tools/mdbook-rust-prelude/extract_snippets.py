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
                        block_lang = lang
                        block_lines = [line]
                        block_start = i
                        in_block = True
                elif in_block and line.startswith('```'):
                    snippets.append({
                        'path': rel,
                        'start': block_start,
                        'lang': block_lang,
                        'lines': block_lines,
                    })
                    block_lang = ''
                    block_lines = []
                    in_block = False
                elif in_block:
                    block_lines.append(line)
    return snippets


def prelude_for(body):
    """Add `pub use stygian_*::*;` for any workspace crate referenced."""
    used = set()
    for c in WORKSPACE_CRATES:
        if re.search(rf'\b{c}::', body):
            used.add(c)
    if re.search(r'\banyhow::', body):
        used.add('anyhow')
    lines = []
    for c in sorted(used):
        lines.append(f'pub use {c}::*;')
    return lines


def needs_async(body):
    return '.await' in body or 'async fn' in body


def uses_try_operator(body):
    """Detect `?` at end-of-line OR `.await?` chained expressions in
    the snippet body. If the snippet doesn't define its own `fn` to
    anchor `?` to, the wrapper needs to return Result so `?` works."""
    has_own_fn = bool(re.search(r'\bfn\s+\w', body))
    if has_own_fn:
        return False
    # Trailing `?` on its own line (the common case in docs).
    if re.search(r'\?\s*$', body, re.MULTILINE):
        return True
    # `.await?` chained form.
    if re.search(r'\.await\s*\?', body):
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
        ret = ' -> Result<(), Box<dyn std::error::Error>>'
    elif async_:
        attr = '#[tokio::test]'
        ret = ''
    elif try_:
        attr = '#[test]'
        ret = ' -> Result<(), Box<dyn std::error::Error>>'
    else:
        attr = '#[test]'
        ret = ''

    indented_body = '\n'.join(
        ('    ' + line) if line else '' for line in body.split('\n')
    )

    return f'''{comment}
{prelude}

{attr}
fn {test_name}(){ret} {{
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