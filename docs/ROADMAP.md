# Roadmap

Positioning: **a fast, secure and ergonomic Rust SDK for building MCP
servers.** Every item is judged by three criteria: it keeps the macro API
simple, it adds no cost to the request hot path (measured), and it keeps the
defaults secure. Feature parity with the official `rmcp` SDK is not a goal:
`rmcp` also covers clients and the whole protocol surface. This SDK focuses on
servers and competes on simplicity, efficiency and security.

## Done

| Phase | Content |
| --- | --- |
| 1 | Spec-generated types (2026-07-28), stateless runtime, stdio and Streamable HTTP, `#[tool]`, test client, benchmarks, fuzzing, official conformance suite in CI |
| 2 | `#[resource]` (URI templates), `#[prompt]`, completion, multi round-trip requests with signed `requestState` |
| Security review | 9 findings fixed with regression tests ([SECURITY.md](SECURITY.md)) |
| 3 | Comparative benchmarks against `rmcp` and the official Python SDK ([bench/results/RESULTS.md](../bench/results/RESULTS.md)) |

## Next, in order

### 3. Verifiable performance (done; keep running)

Claims of efficiency need numbers anyone can reproduce. A separate workspace
(`bench/`) holds:

- the **same server** written with SDKMCPRust, `rmcp` and the official Python
  SDK (`mcp`, FastMCP-style API), all speaking 2026-07-28 over Streamable
  HTTP: a trivial tool, a 64 KiB echo, structured output and `tools/list`;
- a **load generator** in Rust that adds as little client overhead as
  possible;
- metrics: latency p50/p95/p99, requests per second at several
  concurrency levels, memory (RSS) idle and under load, CPU time per request,
  startup time, artifact size;
- one command to run everything, pinned versions, JSON results.

Results are published with the hardware, OS and versions. Laptop runs are
indicative only; the numbers that matter come from a quiet, dedicated machine.

First results (laptop, 64 clients): 1.6–1.9× the throughput of `rmcp` with
2.3–2.6× less CPU per request on small messages, parity on 64 KiB payloads,
and 22–39× the throughput of the Python SDK. On a Linux CI runner: 2.4–2.6×
rmcp's throughput with 3.2–4.1× less CPU on small messages. Next steps: add the stdio transport, and look for the remaining cost on
large payloads (several `serde_json` passes over the same message).

### 4. Compatibility with 2025-11-25 clients

Many clients (including the MCP Inspector by default) still speak
2025-11-25 (`initialize`, sessions). A server that only speaks 2026-07-28
cannot be used with them, which blocks adoption more than any missing
feature. Plan: an adapter at the transport edge that answers `initialize`
and translates legacy requests onto the same stateless core, so handlers and
macros do not change.

### 5. Progress, logging and response streaming

SSE responses for `notifications/progress` and `notifications/message`
(gated by `_meta.../logLevel`), with `ctx.progress(..)` and `ctx.log(..)`.
Closes the last 3 conformance failures. The plain JSON path must stay as fast
as today.

### 6. Authentication (OAuth 2.1 resource server)

Bearer-token validation as a tower layer, protected resource metadata, and
`requestState` bound to the authenticated principal (the remaining
known limitation of the security model).

### 7. First release

Final crate name, docs.rs, `0.1.0` on crates.io, a "zero to Claude in five
minutes" guide.

## Not planned

- A client SDK: `rmcp` covers it; the focus here is servers.
- Tasks, MCP Apps and subscriptions wait until there is demand; the
  architecture keeps room for them (ARCHITECTURE §12).
