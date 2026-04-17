# T52 - Proxy Capability Model and Protocol-Aware Routing

> Depends on: none

## Goal

Add a proxy capability model to stygian-proxy and route requests based on protocol and capability requirements rather than only generic availability.

## Why

Different proxy providers support different feature sets (HTTPS CONNECT, SOCKS5 UDP, geography stability). Matching request requirements to proxy capability improves success rate and reduces wasted retries.

## Scope

- Extend proxy metadata model with capability flags.
- Add selection filters by required capability.
- Introduce protocol-aware routing paths for:
  - h1/h2 over TCP
  - h3 over UDP where supported

## Required Capabilities

- Capability fields (initial):
  - `supports_https_connect`
  - `supports_socks5_udp`
  - `supports_http3_tunnel` (future-compatible)
  - optional geo confidence fields
- Selection API additions:
  - request required capability set
  - return typed error when no suitable proxy exists

## Tests

- Unit test: capability filtering in candidate selection.
- Unit test: fallback behavior when no compatible proxy exists.
- Integration test: mixed pool with protocol-specific selection.

## Preflight

```bash
cargo build --workspace --all-features
cargo test -p stygian-proxy --all-features
cargo clippy -p stygian-proxy --all-features -- -D warnings
```

## Exit Criteria

- [ ] Capability model added to proxy entities
- [ ] Selector honors capability requirements
- [ ] Protocol-aware routing pathways implemented
- [ ] Tests cover positive and no-candidate flows
