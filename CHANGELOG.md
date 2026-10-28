# Changelog

All notable changes to FlyMCPRust are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may
break the API).

## [0.1.1] - 2026-10-28

### Changed

- Higher concurrency defaults, with memory bounded in bytes: request bodies
  held at once may total 64 MiB (`HttpConfig::max_body_bytes_in_flight`,
  new), reserved frame by frame as they arrive; `max_concurrent_calls`
  1024 (was 64), `HttpConfig::max_in_flight` 4096 (was 256),
  `HttpConfig::max_connections` 4096 (was 1024). Servers whose tools wait
  on other services were capped by the old defaults.
- MCP Apps app-only tools (`visibility = ["app"]`) are listed and callable
  for every client: some hosts render interfaces without declaring the
  extension.

### Fixed

- `#[tool]`, `#[resource]` and `#[prompt]` handlers with a parameter named
  like the function (`fn user(user: User)`) did not compile.

### Added

- `examples/apps-demo`: a runnable MCP Apps server, tested in the official
  MCP Apps test host.

## [0.1.0] - 2026-10-26

First release: a high-performance MCP server SDK for Rust, targeting
protocol revision 2026-07-28.

### Protocol

- Stateless 2026-07-28: `server/discover`, per-request `_meta`, version
  negotiation, `resultType`, caching hints (`ttlMs`, `cacheScope`).
- Tools, resources and resource templates, prompts, completion.
- Multi round-trip requests (elicitation; sampling and roots, deprecated)
  with signed, expiring `requestState`.
- Progress notifications, logging (deprecated, SEP-2577), SSE response
  streaming with cancellation on disconnect.
- Clients of 2025-11-25 and 2025-06-18: `initialize`, signed stateless
  sessions, input requests as server-to-client requests.
- MCP Apps extension (`io.modelcontextprotocol/ui`): `ui://` interfaces,
  CSP, tool visibility.
- Official conformance suite: 147/147 checks for 2026-07-28, 76/78 for
  2025-11-25 (resource subscriptions are not implemented).

### Transports

- stdio, and Streamable HTTP as a `tower::Service` plus a built-in hyper
  server (HTTP/1.1).
- OAuth 2.1 resource server on HTTP: bearer tokens, Protected Resource
  Metadata (RFC 9728), `401`/`403` challenges, per-handler scopes, and a
  built-in JWT verifier with JWKS discovery (feature `oauth`).

### Developer experience

- `#[tool]`, `#[resource]` and `#[prompt]` macros: handlers are plain
  functions; schemas and descriptions come from the signature and doc
  comments; misuse is a compile error at the right span.
- Fix-it hints in build errors and runtime warnings; in-process
  `testing::TestClient`.

### Security

- No `unsafe`, no panics on the request path, sanitized client errors,
  secure default for every limit, Origin and Host allowlists.
- Three recorded security reviews (see `docs/SECURITY.md`) and five fuzz
  targets.

[0.1.1]: https://github.com/charlesmmorais/flymcprust/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/charlesmmorais/flymcprust/releases/tag/v0.1.0
