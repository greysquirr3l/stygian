# stygian docs site

Astro + Starlight site for stygian, replacing the former mdBook. Content lives in
`src/content/docs/`; sidebar order is set in `astro.config.mjs`.

- Develop: `pnpm install && pnpm dev`
- Build: `pnpm build` (output in `dist/`)
- Deploy: `pnpm run deploy` builds, rsyncs into the `greysquirr3l.github.io` clone
  (`deploy-docs` block in `package.json`), pushes, and polls the live URL.
  `pnpm run deploy:dry` stops before the push.
- Rust snippets are compile-checked by `tools/mdbook-rust-prelude/extract_snippets.py`,
  which reads the fences in `src/content/docs/`.

If the live URL 404s with "Site not found", Pages is enabled on the source repo and
shadows the user-pages route: `gh api -X DELETE repos/greysquirr3l/stygian/pages`.
The site is deployed by script only; CI builds it but does not publish.
