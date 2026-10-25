# Architecture

This document records the architectural decisions behind SDKMCPRust, the
alternatives that were considered and rejected, and the reasoning behind each
choice. It is a living document: when a decision changes, update the relevant
section and note the change in the decision log at the end.

## 1. Goals

SDKMCPRust is a Rust SDK for building Model Context Protocol (MCP) servers.
It aims to be:

- **The most ergonomic** Rust MCP SDK: FastMCP-style tools declared as plain
  (async) functions, with schemas derived from the signature.
- **Safe by construction**: no `unsafe`, no panics on the request path, secure
  defaults for every limit and for the HTTP transport.
- **Fast**: minimal allocations and copies on the hot path, measured with
  benchmarks from day one.
- **The closest to the specification**: protocol types are generated from the
  official schema, and conformance is measured with the official test suite.

## 2. Source of truth

| Item | Value |
| --- | --- |
| Target protocol revision | `2026-07-28` |
| Future compatibility target | `2025-11-25` (legacy, handshake-based) |
| Spec repository | `modelcontextprotocol/modelcontextprotocol` |
| Pinned spec commit | `271ecc9accafdd9b83a3c869fa67c22953b2af80` |
| Machine-readable schema | `schema/2026-07-28/schema.json` (JSON Schema 2020-12, 155 definitions) |
| Human-authored schema | `schema/2026-07-28/schema.ts` (source of truth; carries `@deprecated` tags) |

When this document and the specification disagree, the specification wins.

### 2.1 What the 2026-07-28 revision means for a server SDK

The points below drive most of the design. Each one cites the relevant part of
the specification.

1. **Stateless protocol.** There is no `initialize` / `notifications/initialized`
   handshake and no protocol session. Every request carries
   `io.modelcontextprotocol/protocolVersion` and
   `io.modelcontextprotocol/clientCapabilities` (both required) and, optionally,
   `io.modelcontextprotocol/clientInfo` and `io.modelcontextprotocol/logLevel`
   in `params._meta`. A request missing a required field is rejected with
   `-32602` (HTTP `400`).
2. **`server/discover` is mandatory.** It returns `supportedVersions`,
   `capabilities`, optional `instructions`, and caching hints.
3. **Every result carries `resultType`.** Core values are `"complete"` and
   `"input_required"` (multi round-trip requests). Extensions may add values,
   such as `"task"` for the Tasks extension.
4. **Caching hints are required** (`ttlMs >= 0`, `cacheScope` of `"public"` or
   `"private"`) on `server/discover`, `tools/list`, `prompts/list`,
   `resources/list`, `resources/templates/list` and `resources/read`.
5. **Servers SHOULD identify themselves** with
   `io.modelcontextprotocol/serverInfo` in every result's `_meta`.
6. **Version negotiation is per request.** An unsupported version yields
   `UnsupportedProtocolVersionError` (`-32022`, with `data.supported` and
   `data.requested`).
7. **Error code policy.** `-32000..=-32019` is legacy and must not be used by
   new implementations. `-32020..=-32099` is reserved for the specification:
   `-32020` HeaderMismatch, `-32021` MissingRequiredClientCapability, `-32022`
   UnsupportedProtocolVersion. Resource-not-found is now `-32602`.
8. **Streamable HTTP is one POST endpoint.** There is no GET stream, no
   `Mcp-Session-Id` and no `Last-Event-ID` resumption. Requests must carry
   `MCP-Protocol-Version`, `Mcp-Method`, and (for `tools/call`,
   `resources/read`, `prompts/get`) `Mcp-Name`. Values may use the
   `=?base64?…?=` sentinel encoding. Tool parameters annotated with
   `x-mcp-header` are mirrored into `Mcp-Param-{Name}` headers, and the server
   must validate every mirrored value against the body (`-32020`, HTTP `400`).
   An unknown method is HTTP `404` with `-32601`. GET or DELETE is `405`. An
   accepted notification is `202`. Closing a response stream cancels the
   request.
9. **stdio** uses newline-delimited JSON-RPC. The server must never write
   anything but MCP messages to stdout, and must never write requests.
   Cancellation uses `notifications/cancelled`. EOF on stdin is the shutdown
   signal.
10. **Multi Round-Trip Requests (MRTR)** replace server-to-client requests.
    `tools/call`, `prompts/get` and `resources/read` may return an
    `InputRequiredResult` with `inputRequests` and/or an opaque `requestState`.
    The `requestState` is attacker-controlled input and must be
    integrity-protected when it matters.
11. **Tasks moved out of core** into the `io.modelcontextprotocol/tasks`
    extension (`resultType: "task"`, `tasks/get`, `tasks/update`,
    `tasks/cancel`).
12. **Deprecated features** (SEP-2577): Roots, Sampling and Logging. A server
    must not emit `notifications/message` for requests without
    `io.modelcontextprotocol/logLevel`. HTTP+SSE remains deprecated.
13. **Tool lists** must not vary per connection and SHOULD be returned in a
    deterministic order.
14. **JSON Schema** defaults to the 2020-12 dialect. Implementations must not
    automatically dereference network `$ref`s, and SHOULD bound
    composition-keyword validation cost.

### 2.2 Divergences from the original project brief

These points were absent from, or differed from, the initial brief. In every
case the specification takes precedence.

- `resultType`, `ttlMs` and `cacheScope` are mandatory protocol fields; the
  brief did not mention them.
- `Mcp-Param-{Name}` (via `x-mcp-header`) also requires server-side validation.
  It goes beyond the three headers the brief listed.
- **Extensions carry no version number.** An extension evolves through flags
  in its settings object; a breaking change gets a new identifier (for example
  `…/tasks-v2`). The extension trait is therefore keyed by identifier plus
  settings, not by a version.
- `schema.json` carries no deprecation markers. Only the JSDoc in `schema.ts`
  does, so code generation reads both files.

## 3. Build on `rmcp` or build our own

`rmcp` (v3.7.0, October 2026) is the official Rust SDK. It implements the
2026-07-28 revision fully, including dual-era support, MRTR, OAuth and tier-1
conformance.

| Option | Pros | Cons |
| --- | --- | --- |
| **A. Own stack** (chosen) | Generated spec types; a `tower::Service` core; full control of the hot path; no session-era baggage in the core. | We must reach conformance ourselves, and add OAuth and dual-era later. |
| B. Build on `rmcp` | Conformance and OAuth available immediately. | Peer/session architecture rather than a `tower::Service`. Hand-written types mixing every revision. No control over allocations. Fast-moving semver (3.0 to 3.7 in three months). Our value would shrink to macro sugar. |
| C. Own core, reuse `rmcp::model` | Skips code generation. | Gives up "spec as code" and couples our public API to `rmcp`'s types and release cycle. |

**Decision: A.** The 2026-07-28 revision removed the hardest stateful parts of
the protocol (handshake and sessions), so this is the cheapest moment to build
a clean core. `rmcp` stays a **dev-dependency only**: we use it for
interoperability tests (its client against our server) and for comparative
benchmarks. Progress is measured with the official
`modelcontextprotocol/conformance` suite in CI.

## 4. Workspace layout

```
crates/
  sdkmcprust-spec/     Generated protocol types, one module per revision.
  sdkmcprust-core/     Runtime: JSON-RPC, limits, routing, registry,
                       extractors, errors, tower::Service, transports.
  sdkmcprust-macros/   The #[tool], #[resource] and #[prompt] macros.
  sdkmcprust/          Facade crate that re-exports the public API.
xtask/                 Spec fetching and code generation.
examples/calculadora/  Example server.
fuzz/                  cargo-fuzz targets (outside the workspace).
spec/<revision>/       Pinned schema files plus a SHA-256 lock.
```

| Crate | Depends on | Notes |
| --- | --- | --- |
| `sdkmcprust-spec` | `serde`, `serde_json` | No I/O and no async runtime, so it is WASM-ready. Module `v2026_07_28`; `v2025_11_25` will follow. |
| `sdkmcprust-core` | spec, `tower`, `futures`, `tokio` (sync/time), `tracing`, `schemars` | Features: `stdio`, `http` (`http`, `http-body-util`, `hyper`, `hyper-util`), `test-client`. |
| `sdkmcprust-macros` | `syn`, `quote`, `proc-macro2`, `darling` | Emits paths through the facade (`::sdkmcprust::__private`). |
| `sdkmcprust` | core, macros | Features: `macros` (default), `stdio`, `http`, `auto-register`. |

**Naming.** The working title is `SDKMCPRust`. Crate names use lowercase
(`sdkmcprust-*`) because crates.io is case-insensitive and the ecosystem
convention is lowercase. The final name is still to be decided.

**Alternatives considered.**

- *A separate transport crate.* Transports live in `-core` behind features for
  now. They can be split out later without breaking the facade.
- *A separate test-client crate.* A `test-client` feature is enough and keeps
  the internal types private.

## 5. Spec as code

`cargo xtask fetch-spec <revision>` downloads `schema.json` and `schema.ts` at
the pinned commit into `spec/<revision>/`, together with a SHA-256 lock file.
`cargo xtask codegen` turns them into Rust under
`crates/sdkmcprust-spec/src/v<revision>/`. The generated code is committed, so
users never need network access to build. CI runs `cargo xtask codegen --check`
and fails if the committed output is stale.

The generator works in three steps:

1. **Parse** `$defs` into an intermediate representation (IR): structs,
   enums, aliases and open maps.
2. **Apply overrides** from `xtask/overrides/<revision>.toml`:
   - `anyOf` unions with a constant discriminator become internally tagged
     enums (`type` for content blocks, `method` for requests and
     notifications).
   - Protocol `_meta` keys (`io.modelcontextprotocol/*`) become typed fields
     with `#[serde(rename)]`.
   - Unknown `_meta` keys are preserved in a flattened map.
3. **Annotate deprecations** by scanning the JSDoc in `schema.ts` for
   `@deprecated` before each `interface`/`type`/field. Matching items get
   `#[deprecated(note = "…")]`.

The output is formatted with `prettyplease`.

**Rejected: `typify`.** It maps the spec's `anyOf` unions to
`#[serde(untagged)]` enums, which are slow and produce poor error messages. It
also needs patches for slash-containing `_meta` keys, and it cannot see
deprecations.

## 6. Runtime core

### 6.1 Request pipeline

```
bytes ─► size limit ─► depth pre-scan ─► JSON-RPC envelope parse
      ─► _meta validation + version negotiation ─► router (by method)
      ─► handler ─► result + resultType + _meta.serverInfo ─► serialize
```

- The server is a `tower::Service<McpRequest, Response = McpResponse>`.
  `McpRequest` holds the parsed message and a `TransportContext` (an
  `http::Extensions` bag for auth claims, headers and similar, populated by the
  transport). Timeouts, concurrency limits and tracing are ordinary tower
  layers, so users can add their own middleware.
- **Envelope rules.** `id` must be a string or an integer, never null. JSON-RPC
  batches are rejected because the 2026-07-28 revision does not define them.
- **Version negotiation.** The supported versions come from the server builder
  (initially `["2026-07-28"]`). Anything else yields `-32022`.
- **Capability gating.** A handler that needs a client capability the client
  did not declare returns `-32021`.

### 6.2 Hot path

- The size limit is checked before parsing. Nesting depth is checked with a
  single byte scan that tracks string and escape state, before `serde`
  recursion.
- `serde_json::from_slice` borrows `&RawValue` for `params.arguments`. Tool
  arguments deserialize directly into the macro-generated argument struct,
  with no intermediate `serde_json::Value`.
- **Immutable registry.** After `build()`, tools sit in an `Arc<[ToolEntry]>`
  sorted by name (deterministic order) with a name index. The `tools/list`
  body is serialized once at build time.
- Results serialize straight into the output buffer.

### 6.3 Limits (secure defaults, all configurable)

| Limit | Default |
| --- | --- |
| Maximum message size (HTTP body / stdio line) | 4 MiB |
| Maximum JSON nesting depth | 64 |
| Per-tool timeout | 30 s |
| Maximum concurrent requests | 64 |
| HTTP bind address | `127.0.0.1` |
| Allowed `Origin`s | localhost origins only (absent `Origin` accepted) |

### 6.4 Errors

The design follows two rules: never panic on the request path, and never leak
internal details to the client.

- Protocol errors use the specification's codes, with sanitized messages.
- `ToolError::user(msg)` becomes a tool result with `isError: true` and the
  given message, so the model can self-correct (spec: tool execution errors).
- Any other error type (`E: std::error::Error`), as well as panics caught at
  the tool boundary, is logged through `tracing` with full detail. The client
  receives a generic `isError` message.
- SDK-local failures (timeouts, overload) use generic messages and never codes
  from the reserved ranges.

### 6.5 Tool outputs

The trait `IntoToolOutput` is implemented for:

| Return type | Produces |
| --- | --- |
| `String`, `&str`, primitives | Text content |
| `Json<T: Serialize + JsonSchema>` | `structuredContent`, a serialized `TextContent` block (backward compatibility), and an `outputSchema` derived from `T` |
| `CallToolResult` | Raw result, for full control |
| `Result<T, E>` | `T` as above; `E` mapped as in §6.4 |

The internal `ToolOutput` enum is `#[non_exhaustive]`, so `InputRequired`
(MRTR) and extension results such as Tasks can be added without breaking
changes.

### 6.6 Extensions

Extensions implement a trait with these methods:

- `id()` returns the identifier, for example `io.modelcontextprotocol/tasks`.
- `server_settings()` returns the settings object advertised in
  `capabilities.extensions`.
- `routes()` returns additional methods.
- `result_types()` returns additional `resultType` values.
- An optional result-interception hook, for example to turn a long tool call
  into a task.

An extension is active for a request only if the client listed it in that
request's capabilities. Otherwise the core falls back to core behavior, as the
spec requires. Phase 1 ships the trait and an empty registry.

## 7. Transports

### 7.1 stdio

- **Bounded line reader.** A line longer than the message limit is rejected
  and the reader resynchronizes at the next newline.
- **Bounded concurrency.** Requests are processed concurrently, up to the
  concurrency limit.
- **Single writer task** fed by an mpsc channel. Every outgoing message is
  serialized to a single line, and nothing else is ever written to stdout.
  Diagnostics go to stderr through `tracing`.
- **Cancellation.** `notifications/cancelled` maps request ids to
  cancellation tokens. A cancelled request produces no response.
- **Shutdown.** EOF stops accepting new requests, then waits for in-flight work
  up to a deadline.

### 7.2 Streamable HTTP (stateless)

The transport is implemented as `tower::Service<http::Request<B>>` on top of
`http` and `http-body`. A convenience `serve()` uses `hyper-util`. Axum users
can mount it with `Router::nest_service("/mcp", svc)`.

**Rejected: building on axum.** It would add a mandatory heavy dependency and
couple our release cycle to axum's.

Request handling, in order:

1. Only POST is accepted. GET and DELETE get `405`.
2. `Origin` is checked against the allowlist; an invalid origin gets `403`.
   `Host` is also checked against the allowlist when bound to loopback, as
   defense in depth against DNS rebinding.
3. `Content-Type: application/json` is required, and `Accept` must list both
   `application/json` and `text/event-stream`.
4. The body is read with `http-body-util::Limited`; an oversized body gets
   `413`.
5. Header and body validation:
   - `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` are decoded (base64
     sentinel) and compared with the body. A missing or mismatching header gets
     `400` with `-32020`.
   - Recognized `Mcp-Param-*` headers are validated the same way.
   - `Mcp-Session-Id` and `Last-Event-ID` are ignored.
6. Status mapping:

   | Outcome | HTTP status |
   | --- | --- |
   | Notification accepted | `202` |
   | Unknown method | `404` |
   | `-32602` for missing `_meta`, `-32020`, `-32021`, `-32022` | `400` |
   | Any other response | `200` |

7. Phase 1 responds with `application/json`. The SSE response path (for
   progress notifications) is designed in: when the response stream is
   dropped, the handler future is dropped too, which is the cancellation
   semantics the spec requires.

### 7.3 Future: legacy (2025-11-25) adapter

Legacy support reintroduces state: the `initialize` handshake, plus
`Mcp-Session-Id` over HTTP. It lives entirely at the transport edge. A legacy
adapter recognizes `initialize`, stores the negotiated version and
capabilities per stdio process or HTTP session, and synthesizes the per-request
context the stateless core expects. The core never learns about sessions. Era
detection follows the spec's compatibility matrix.

## 8. The `#[tool]` macro

```rust
/// Adds two numbers.
///
/// # Arguments
/// * `a` - First operand.
/// * `b` - Second operand.
#[tool(read_only, idempotent)]
async fn add(a: f64, b: f64) -> f64 {
    a + b
}

Server::builder("calc", "0.1.0")
    .tool::<add>()          // explicit registration
    // .auto_tools()        // feature = "auto-register"
    .build()?
    .serve_stdio()
    .await?;
```

- **Argument struct.** The macro generates a hidden argument struct deriving
  `Deserialize` and `schemars::JsonSchema`. schemars 1.x emits the 2020-12
  dialect. `Ctx` and `State<T>` parameters are recognized and excluded from
  the schema.
- **Same-name marker type.** The macro emits a braced unit struct with the
  function's name (`struct add {}`). A braced struct only occupies the type
  namespace, so the function `add` remains callable as before, and the struct
  implements the `Tool` trait used by `.tool::<add>()`.
- **Descriptions.** The tool description is the doc comment up to its
  first heading; later sections (`# Errors`, `# Examples`) stay rustdoc-only.
  Parameter descriptions come from doc comments placed directly on the
  parameters (the macro strips them before the compiler validates the
  function) or from the `# Arguments` section. Documenting a name that is not
  a JSON parameter is a compile error.
- **Forwarded attributes.** `#[serde(..)]` and `#[schemars(..)]` on a
  parameter are copied to the generated field (defaults, validation ranges,
  renames). Any other parameter attribute is a compile error.
- **Annotations.** `read_only`, `idempotent`, `destructive` and `open_world`
  map to the `…Hint` fields of `ToolAnnotations`. Also supported: `name = "…"`
  and `title = "…"`.
- **Output schema.** The `outputSchema` is derived from the return type when
  it is `Json<T>` (or `Result<Json<T>, E>`).
- **Compile errors.** Errors are reported with `syn::Error` and precise spans,
  and covered by `trybuild` UI tests.

**Registration.** Explicit registration always works. The opt-in
`auto-register` feature uses `inventory`, so `.auto_tools()` collects every
`#[tool]` in the binary.

**Rejected: auto-registration by default.** Implicit global registration is
surprising and depends on linker support on every platform.

`inventory` relies on link sections. The spike confirmed it is compatible
with `#![forbid(unsafe_code)]`, both in `sdkmcprust-core` (which declares the
registry) and in user crates (where `#[tool]` submits entries), so no
exception to the safety rules is needed.

**Alternatives considered for the API shape.**

- *Methods on a struct with a `Parameters<T>` wrapper* (the `rmcp` style).
  Rejected: it needs boilerplate for every tool.
- *Replacing the function with a struct of the same name.* Rejected: it makes
  the function no longer directly callable.

## 9. Safety rules

- `#![forbid(unsafe_code)]` in every crate. Any exception must be justified in
  this document.
- `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic` and
  `clippy::indexing_slicing` are set to `deny` outside tests.
- No panic may escape a tool. Tool futures run under `catch_unwind`, which
  becomes a generic error.
- `requestState` (MRTR, future work) will be sealed with HMAC/AEAD, a short
  expiry and request binding, as the spec requires.
- Network `$ref`s are never dereferenced.
- **Supply chain.** `cargo-deny` checks licenses, advisories, bans and sources;
  `cargo-audit` runs in CI. Spec downloads are verified against a SHA-256
  lock.
- **Fuzzing.** The `fuzz/` directory holds two targets. `parse_message`
  covers the envelope parser and the depth scanner. `handle_message` covers
  full request handling: `_meta` validation, dispatch and error mapping. CI
  runs each one for 60 seconds on nightly.
- **Documented exception.** The fuzz harness cannot use
  `#![forbid(unsafe_code)]`, because `libfuzzer-sys`'s `fuzz_target!` expands
  to `unsafe` FFI glue. It is test-only code outside the workspace and is
  never shipped.

## 10. Performance

- `criterion` benchmarks from the first version, covering three areas:
  - message parsing (small, typical and large payloads);
  - dispatch (`tools/list`, `tools/call`);
  - result serialization.
- Comparative benchmarks against `rmcp` (as a dev-dependency) back up the
  "fastest" claim.
- Results are recorded in the README at each milestone, with machine and
  toolchain details.

## 11. Toolchain and policy

- Edition 2024, resolver 3.
- **MSRV 1.88**, matching `rmcp` and `darling`, and sufficient for schemars 1.2,
  tokio 1.54 and criterion 0.8. A dedicated CI job enforces it.
- License: `MIT OR Apache-2.0`.

## 12. Out of scope for phase 1 (the architecture must allow them)

| Feature | Where it plugs in |
| --- | --- |
| OAuth | A tower layer on the HTTP service populates `TransportContext`. Cache scope becomes `private` per authorization context. |
| 2025-11-25 compatibility | §7.3 legacy adapter, plus a `spec::v2025_11_25` module. |
| Tasks, MCP Apps | The extension trait (§6.6). Apps uses the `io.modelcontextprotocol/ui` identifier. |
| Subscriptions (`subscriptions/listen`) | A long-lived response stream. Uses the SSE path on HTTP and `subscriptionId` tagging on stdio. |
| WASM target | `spec` is already I/O-free. `core` gates `tokio` behind features. |

## 13. Decision log

| Date | Decision |
| --- | --- |
| 2026-10-25 | Own stack instead of building on `rmcp`; explicit registration plus opt-in `inventory` auto-registration; HTTP on `http` + `hyper-util`; custom code generator reading `schema.json` and `schema.ts`. |
| 2026-10-25 | `McpError` carries an `ErrorKind` so HTTP can choose the status without guessing: malformed requests are `400`, unknown methods `404`, failed-but-valid requests (unknown tool) `200`. Answering `400` for an unknown tool could make a dual-era client fall back to the legacy protocol. |
| 2026-10-25 | `x-mcp-header` (SEP-2243) is supported in phase 1: annotations are validated at build time, and `Mcp-Param-*` headers are checked against the arguments. |
| 2026-10-25 | Generated schemas drop schemars' non-standard numeric `format` values (`uint`, `double`, …). They are not JSON Schema 2020-12 formats, clients warn about them (the MCP Inspector does), and they cost model context. |
| 2026-10-25 | Tools can return protocol-level errors through `ToolError::protocol` and `ToolError::missing_client_capability`. The spec requires `-32021` (HTTP `400`), not an `isError` result, when a request needs an undeclared client capability. |
| 2026-10-25 | Conformance is measured with `@modelcontextprotocol/conformance@0.2.0-alpha.12`, the first release with 2026-07-28 server scenarios (`latest` still targets 2025-11-25), against `examples/conformance-server` with a per-check baseline. |
| 2026-10-25 | Phase 2 public API (chosen by the owner): resources and prompts are declared with `#[resource]` / `#[prompt]` macros consistent with `#[tool]`, with `ResourceDef` / `PromptDef` for registration by hand. |
| 2026-10-25 | One error type, `ToolError`, for all handlers. For resources and prompts it maps to JSON-RPC errors: `user`/`invalid_argument` → `-32602` with the message, `not_found` → `-32602` with `data.uri` (the 2026-07-28 resource-not-found), internal/panic/timeout → `-32603` with a generic message. |
| 2026-10-25 | URI templates support the RFC 6570 subset `{var}`, `{+var}`, `{/var}` and trailing `{?a,b}`. Matching is deterministic and linear (a variable ends at the first occurrence of the next literal, `{+var}` at the last), so ambiguous templates (adjacent variables, two `{+var}`) are rejected at build time, and URIs over 8 KiB never match. |
| 2026-10-25 | `resources/read` defaults to `ttlMs: 0` and `cacheScope: "private"` (safe for per-caller data); lists keep the server-wide hints. Static resources are matched before templates; templates in name order, first match wins. |
| 2026-10-25 | Completion providers are synchronous (`fn(&CompletionRequest) -> Completion`), attached with `#[complete(path)]` or `#[complete(values = [..])]`. Results are truncated to 100 values with `total`/`hasMore`. Methods for features the server does not offer (`resources/*`, `prompts/*`, `completion/complete`) answer `-32601`, matching the advertised capabilities. |
| 2026-10-25 | MRTR uses the *replay* model (chosen by the owner): `ctx.elicit(key, msg).await?` returns the answer if the client sent it (this round or an earlier one), otherwise records the request and ends the round with an internal "input required" error that the runtime turns into `InputRequiredResult`. Handlers rerun from the start each round, so code before an input call must be idempotent. Questions awaited together (`tokio::join!`) share a round. This replaces the planned `ToolOutput::InputRequired`: an error kind propagates with `?` through any return type. |
| 2026-10-25 | `requestState` is always emitted and carries the answers of earlier rounds, so the server stays stateless. It is HMAC-SHA256 signed (`hmac` + `sha2`, chosen by the owner) and bound to the method, the target (tool/prompt name or resource URI), a digest of the arguments (whitespace- and key-order-insensitive) and an expiry (default 10 min). Failed verification is `-32602`. The key is random per process unless `request_state_key` (at least 32 bytes) is set, which multi-instance deployments need. Binding to the authenticated principal (a spec SHOULD) waits for OAuth. |
| 2026-10-25 | Answers in the current round's `inputResponses` need no state: they are client input like arguments. Unknown keys are ignored; a missing answer is asked again; an answer that does not decode is `-32602`. Input requests check the client capabilities first (`-32021` otherwise). Sampling and roots helpers are `#[deprecated]` like the features (SEP-2577). |
| 2026-10-25 | Over HTTP, `initialize` skips header validation and gets `404` + `-32601` with a message naming the supported versions. The spec recommends that diagnostic for legacy clients and requires `404` for unimplemented methods. |
