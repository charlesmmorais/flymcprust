# FlyMCPRust

**High-Performance MCP Server SDK for Rust**

*Built in Rust. Engineered for Speed.*

FAST · SAFE · SIMPLE · STATELESS

> Build [Model Context Protocol](https://modelcontextprotocol.io) servers
> with the ergonomics of FastMCP and the guarantees of Rust.

**Status:** version 0.1.0, a complete MCP server SDK for protocol
2026-07-28 (all 147 checks of the official conformance suite pass) that also
serves 2025-11-25 and 2025-06-18 clients: tools, resources, prompts,
completion, multi round-trip requests, progress and logging, MCP Apps
interfaces, over stdio and Streamable HTTP, with OAuth 2.1 authorization on
HTTP. On [crates.io](https://crates.io/crates/flymcprust), with API
documentation on [docs.rs](https://docs.rs/flymcprust); see the
[changelog](CHANGELOG.md) and the [roadmap](docs/ROADMAP.md).

- **Spec-first.** Targets MCP revision **2026-07-28**: stateless, with no
  `initialize` and no sessions. Clients of 2025-11-25 and 2025-06-18 work too,
  without any change to your code. Protocol types are generated from the official
  `schema.json`/`schema.ts`, and deprecations become `#[deprecated]`.
- **Ergonomic.** Tools, resources and prompts are plain (async) functions.
  Schemas, descriptions, arguments and annotations come from the signature
  and doc comments. Asking the user for input is one `ctx.elicit(..).await?`.
- **Safe.** No `unsafe`, no panics on the request path, and secure defaults:
  message size, JSON depth, handler timeout, concurrency, Origin/Host checks,
  localhost binding, signed `requestState`. Internal errors are never exposed
  to clients.
- **Fast.** Parsing borrows the request buffer, tool arguments deserialize
  directly into their struct, and list responses are pre-serialized.
- **Composable.** The server is a `tower::Service`, and so is the HTTP
  transport, which mounts in axum.

## Quick start

New to MCP or to this SDK? Follow
[From zero to an MCP server in Rust in 5 minutes](docs/QUICKSTART.md).

```toml
[dependencies]
flymcprust = { version = "0.1", features = ["stdio"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use flymcprust::prelude::*;

/// Adds two numbers.
///
/// # Arguments
/// * `a` - First operand.
/// * `b` - Second operand.
#[tool(read_only, idempotent)]
async fn add(a: f64, b: f64) -> f64 {
    a + b
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Server::builder("calculator", "1.0.0")
        .tool::<add>()
        .build()?
        .serve_stdio() // or .serve_http("127.0.0.1:8000") with the `http` feature
        .await?;
    Ok(())
}
```

### What `#[tool]` gives you

| You write | You get |
| --- | --- |
| Doc comment (up to the first heading) | Tool `description` |
| `# Arguments` list, or `///` on a parameter | Parameter descriptions in `inputSchema` |
| Parameters (`a: f64`, `tags: Vec<String>`, `limit: Option<u32>`) | `inputSchema` (JSON Schema 2020-12). `Option` means optional; unknown fields are rejected |
| `#[serde(..)]` / `#[schemars(..)]` on a parameter | Defaults, renames and validation ranges in the schema |
| `ctx: Ctx`, `db: State<Db>` | Injected request context and shared state, excluded from the schema |
| `-> Json<T>` (or `Result<Json<T>, E>`) | `structuredContent` plus an `outputSchema` derived from `T` |
| `-> Result<T, ToolError>` with `ToolError::user("…")` | `isError: true` with your message, so the model can self-correct |
| `-> Result<T, E: std::error::Error>` | Logged server-side; the client gets a generic message |
| `#[tool(read_only, destructive = false, idempotent, open_world)]` | `annotations` hints |
| `#[tool(name = "…", title = "…")]` | Name and title overrides |

Misuse is a compile error at the exact span: generics, `self`, borrowed
parameters, unknown arguments, invalid names, contradictory hints.

Register tools explicitly with `.tool::<add>()`. With the `auto-register`
feature, `.auto_tools()` collects every `#[tool]` in the binary.

### Resources and prompts

```rust
/// The application configuration.
#[resource("config://app", mime_type = "application/json", cache = "public", ttl_ms = 60000)]
fn app_config(cfg: State<Config>) -> Json<Config> {
    Json(cfg.clone())
}

/// A user's profile.
#[resource(template = "users://{id}/profile", mime_type = "text/plain")]
async fn profile(#[complete(user_ids)] id: u64, db: State<Db>) -> Result<String, ToolError> {
    db.profile(id).await.ok_or_else(ToolError::not_found) // -32602 with data.uri
}

/// Reviews a piece of code.
///
/// # Arguments
/// * `code` - The code to review.
/// * `language` - Its programming language.
#[prompt]
fn code_review(code: String, #[complete(values = ["rust", "python"])] language: Option<String>) -> String {
    format!("Review this {} code: {code}", language.as_deref().unwrap_or("source"))
}
```

- **Templates** use RFC 6570 (`{var}`, `{+path}`, `{/segment}`, `{?query}`).
  Variables become typed parameters (`FromStr`); a value that does not parse
  means the resource does not exist.
- **Prompt arguments** are typed parameters too; `Option<T>` is optional.
- **Completion** comes from `#[complete(function)]` or
  `#[complete(values = [..])]` on a prompt argument or template variable.
- `Ctx` and `State<T>` work as in tools. Template mistakes, unknown
  variables and bad options are compile errors.

### Asking the user for input

In 2026-07-28 the server cannot call the client mid-request. It answers
`input_required`, and the client retries with the answers (multi
round-trip requests). The SDK hides the round trips:

```rust
#[derive(Deserialize, JsonSchema)]
struct Confirm {
    /// Whether to deploy.
    confirmed: bool,
}

#[tool(destructive)]
async fn deploy(ctx: Ctx, env: String) -> Result<String, ToolError> {
    let answer: Confirm = ctx.elicit("confirm", format!("Deploy to {env}?")).await?;
    if !answer.confirmed {
        return Err(ToolError::user("cancelled by the user"));
    }
    Ok(format!("deployed to {env}"))
}
```

- The handler **reruns from the start** on each retry, and earlier answers
  come back immediately. Keep code before an input call free of side effects.
- Questions awaited together (`tokio::join!`) go out in one round.
- Answers from earlier rounds travel in `requestState`, which is signed
  (HMAC-SHA256) and bound to the tool, its arguments and an expiry.
  Behind a load balancer, give every instance the same key with
  `.request_state_key(..)`.
- The client capability is checked first (`-32021` if missing). Works in
  tools, resources and prompts. `ctx.sample(..)` and `ctx.list_roots(..)`
  exist too, deprecated like the features themselves (SEP-2577).

### Reporting progress

```rust
#[tool]
async fn import(ctx: Ctx, files: Vec<String>) -> Result<String, ToolError> {
    for (i, file) in files.iter().enumerate() {
        ctx.progress(i as f64, files.len() as f64);
        load(file).await?;
    }
    Ok(format!("imported {} files", files.len()))
}
```

- `ctx.progress` never blocks and costs nothing when the client did not ask
  for progress. With a slow client, only the latest update is kept; it
  arrives before the result. Values must increase.
- Over HTTP the response becomes an SSE stream only once there is something
  to stream; closing it cancels the tool.
- `ctx.log(level, data)` sends log messages to clients that ask for them. It
  is deprecated like MCP logging itself (SEP-2577): log to stderr with
  `tracing` instead.

### Requiring OAuth (HTTP)

```rust
use flymcprust::auth::JwtVerifier; // feature `oauth`
use flymcprust::transport::http::{AllowedHosts, HttpConfig, OAuth, StreamableHttpService};

/// Deletes a file.
#[tool(destructive, scopes = ["files:write"])]
async fn delete_file(ctx: Ctx, path: String) -> Result<String, ToolError> {
    let user = ctx.principal().map(|p| p.subject().to_owned());
    // ...
}

let resource = "https://mcp.example.com/mcp";
let verifier = JwtVerifier::builder("https://auth.example.com", resource).build()?;
let oauth = OAuth::builder(resource, verifier)
    .authorization_server("https://auth.example.com")
    .scopes_supported(["mcp"])
    .required_scopes(["mcp"])
    .build()?;
let config = HttpConfig::new()
    .allowed_hosts(AllowedHosts::List(vec!["mcp.example.com".into()]))
    .oauth(oauth);
let service = StreamableHttpService::with_config(server, config);
```

- The server is an OAuth 2.1 resource server. Requests without a valid
  bearer token get `401` with a `WWW-Authenticate` challenge that points to
  the Protected Resource Metadata document (RFC 9728), which `serve`
  publishes at `/.well-known/oauth-protected-resource/mcp`.
- `scopes = [..]` on `#[tool]`, `#[resource]` or `#[prompt]`: callers without
  every listed scope get `403` with an `insufficient_scope` challenge naming
  all of them. `.scope_implies(..)` declares hierarchies such as `files`
  implying `files:read`.
- `JwtVerifier` (feature `oauth`) checks JWT access tokens: signature with
  the keys the authorization server publishes (found through its metadata,
  cached, refreshed on rotation), issuer, audience (this server), expiry.
  Only asymmetric algorithms exist in it (RSA, RSA-PSS, ECDSA, Ed25519), so
  `none` and HMAC tokens are always refused. For opaque tokens, implement
  `TokenVerifier` yourself (a closure
  `Fn(&str) -> Result<Principal, TokenError>` works); it must check that the
  token was issued for this server.
- `requestState` and legacy sessions are bound to the user: a state or
  session obtained with one user's token is rejected with another's.
- stdio is not authorized this way: per the specification, it takes
  credentials from its environment.

### Interactive interfaces (MCP Apps)

```rust
/// The price survey, as an editable table.
#[resource(
    "ui://reports/price-table",
    app,
    csp_connect = ["https://api.example.gov.br"],
    prefers_border = true
)]
fn price_table() -> String {
    include_str!("price_table.html").to_owned()
}

/// Shows the price survey of an item.
#[tool(read_only, ui = "ui://reports/price-table")]
fn price_survey(ctx: Ctx, item: String) -> String {
    // Hosts without MCP Apps (and the model) read this text.
    format!("Price survey of {item}: median R$ 10.00 (5 sources).")
}

/// Recomputes the reference price; only the table calls it.
#[tool(visibility = ["app"])]
fn recompute(prices: Vec<f64>) -> f64 { /* ... */ }
```

- `app` turns a fixed `ui://` resource into an interface: an HTML5 page
  served as `text/html;profile=mcp-app`. Hosts render it in a sandboxed
  iframe that may load nothing from the network unless you list the origins
  (`csp_connect`, `csp_resources`, `csp_frames`, `csp_base_uri`); browser
  `permissions` and `prefers_border` are also available.
- `ui = ".."` on a tool shows its results in that interface;
  `visibility = ["app"]` leaves a tool to the interface and hides it from the
  model; clients without MCP Apps neither list nor call it. It is not access
  control: protect sensitive tools with OAuth scopes.
- The build fails if a tool names an interface that is not registered, or
  if an origin is malformed. The extension is declared in `server/discover`
  (and to 2025-11-25 hosts such as Claude Desktop) automatically.
- Tools still return text: hosts without MCP Apps show it.
  `ctx.client_supports_apps()` tells you whether the host renders the page.

## Features

| Feature | Default | Description |
| --- | --- | --- |
| `macros` | yes | The `#[tool]`, `#[resource]` and `#[prompt]` attributes. |
| `stdio` | no | stdio transport. |
| `http` | no | Streamable HTTP transport (`tower::Service` plus a hyper server). |
| `oauth` | no | `auth::JwtVerifier` for OAuth access tokens (implies `http`; adds `ring` and `rustls`). |
| `auto-register` | no | `auto_tools()`, `auto_resources()` and `auto_prompts()` on the builder. |
| `logging` | no | `logging::init()`: the log, with fix-it hints, on stderr. |
| `test-client` | no | In-process `testing::TestClient` for integration tests. |

## Protocol support (2026-07-28)

| Area | Status |
| --- | --- |
| `server/discover`, per-request `_meta`, version negotiation (`-32022`) | ✅ |
| `tools/list` (deterministic order, `ttlMs`/`cacheScope`), `tools/call` | ✅ |
| `resultType`, `serverInfo` in every result's `_meta` | ✅ |
| stdio: framing, concurrency, `notifications/cancelled`, graceful EOF | ✅ |
| Streamable HTTP: POST-only endpoint, 202/400/403/404/405/413/415 mapping | ✅ |
| `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` validation (`-32020`), base64 sentinel | ✅ |
| `x-mcp-header` → `Mcp-Param-*` validation (SEP-2243) | ✅ |
| Origin/Host allowlists (DNS-rebinding protection), CORS preflight | ✅ |
| Resources and resource templates (`resources/list`, `templates/list`, `read`), caching hints | ✅ |
| Prompts (`prompts/list`, `prompts/get`) | ✅ |
| Completion (`completion/complete`) | ✅ |
| Multi round-trip requests: elicitation, sampling, roots; signed `requestState` | ✅ |
| Progress (`notifications/progress`), logging (`notifications/message`, deprecated), SSE response streaming | ✅ |
| OAuth 2.1 authorization (resource server): bearer tokens, RFC 9728 metadata, `401`/`403` challenges, per-handler scopes | ✅ (built-in JWT verifier with the `oauth` feature, or bring a `TokenVerifier`) |
| MCP Apps (`io.modelcontextprotocol/ui`): `ui://` interfaces, CSP, tool visibility | ✅ |
| Subscriptions, Tasks extension, MCP client | Not implemented; see [What is not implemented, and why](#what-is-not-implemented-and-why) |
| 2025-11-25 and 2025-06-18 clients: `initialize`, `ping`, signed sessions, `ctx.elicit` as server-to-client requests (stdio and SSE) | ✅ (on by default; `legacy_clients(false)` turns it off) |

### What is not implemented, and why

The goal is the best SDK for building MCP **servers**: efficient, simple
and secure. Feature parity with the official
[`rmcp`](https://github.com/modelcontextprotocol/rust-sdk) SDK is not a goal,
so features are added when real servers need them. Three are left out on
purpose; the architecture keeps room for each
([ARCHITECTURE](docs/ARCHITECTURE.md) §12).

- **MCP client.** A client is what *uses* servers: Claude, IDEs, agents. A
  server author does not need one, `rmcp` already provides a good one, and
  duplicating it would double the code (and the attack surface) without
  helping anyone build servers. For tests there is the in-process
  `testing::TestClient`.
- **Subscriptions** (`subscriptions/listen`, and `resources/subscribe` for
  2025-11-25 clients). They keep one connection open per listening client,
  while this SDK is stateless: any instance answers any request, which is
  what makes it cheap to run and to scale horizontally. With several
  instances, an event published on one does not reach clients listening on
  another without a message bus. Few clients rely on them: most read a
  resource again, guided by the `ttlMs` and `cacheScope` hints this SDK
  already sends. They are the only 2 failing checks of the 2025-11-25
  conformance suite (76/78).
- **Tasks** (the `io.modelcontextprotocol/tasks` extension). The server
  keeps each task's state (progress, result, pending questions) for minutes
  or hours, and with several instances every request about a task must reach
  the instance that runs it. For long tools, the SDK already offers progress
  notifications, cancellation and questions to the user during the call
  (`ctx.elicit`), which cover most cases without server-side state.

They can be added later without rewriting the core: Tasks plugs into the
extension mechanism, and subscriptions would reuse the SSE streaming path.

### Official conformance suite

[`examples/conformance-server`](examples/conformance-server) exposes the
fixtures of `@modelcontextprotocol/conformance`, and CI runs every 2026-07-28
server scenario against it. Known gaps are listed in
[`conformance-baseline.yml`](conformance-baseline.yml), and CI fails on any
other failure or on a stale entry.

Results with `@modelcontextprotocol/conformance@0.2.0-alpha.12` on 2026-10-25:
**all 147 checks of the 2026-07-28 suite pass**, with no warnings.

| Scenario | Result |
| --- | --- |
| `server-stateless` | 25/25 |
| `tools-list`, `tools-call-{simple-text,image,audio,embedded-resource,mixed-content,error}` | all pass |
| `resources-{list,read-text,read-binary,templates-read}`, `sep-2164-resource-not-found` | all pass |
| `prompts-{list,get-simple,get-with-args,get-embedded-resource,get-with-image}` | all pass |
| `completion-complete` | 2/2 |
| `input-required-result-*` (14 MRTR scenarios, including tampered state) | all pass |
| `caching` | 8/8 |
| `json-schema-2020-12` | 8/8 |
| `http-header-validation` | 14/14 |
| `http-custom-header-server-validation` | 10/10 |
| `dns-rebinding-protection` | 2/2 |
| `tools-call-with-progress` | 2/2 |

The 2025-11-25 suite (older clients) passes **76 of 78 checks**; the 2
failures are resource subscriptions, which are not planned
([`conformance-baseline-2025-11-25.yml`](conformance-baseline-2025-11-25.yml)).
Initialization, sessions, ping, logging, progress, tools, resources, prompts,
completion, elicitation and sampling (as server-to-client requests) and
DNS-rebinding protection all pass.

To reproduce locally, run `cargo run -p conformance-server`, then:

```bash
npx @modelcontextprotocol/conformance@0.2.0-alpha.12 server --url http://127.0.0.1:3000/mcp --spec-version 2026-07-28 --suite all --expected-failures conformance-baseline.yml
```

## Try it with the MCP Inspector

The example server lives in [`examples/calculadora`](examples/calculadora).
The MCP Inspector speaks 2025-11-25 by default (add `--protocol-era modern`
for 2026-07-28); the server answers both.

```bash
cargo build -p calculadora
```

**stdio, CLI:**

```bash
npx @modelcontextprotocol/inspector --cli target/debug/calculadora --method tools/list
```

```bash
npx @modelcontextprotocol/inspector --cli target/debug/calculadora --method tools/call --tool-name divide --tool-args-json '{"a":10,"b":4}'
```

**Streamable HTTP:** start the server, then point the Inspector at it.

```bash
cargo run -p calculadora -- --http 127.0.0.1:8000
```

```bash
npx @modelcontextprotocol/inspector --cli --transport http --server-url http://127.0.0.1:8000/mcp --method tools/list
```

**Web UI:** run `npx @modelcontextprotocol/inspector`. Add a server with
command `target/debug/calculadora` (stdio) or URL `http://127.0.0.1:8000/mcp`
(Streamable HTTP), set its protocol era to **modern**, and connect.

On Windows, use `target\debug\calculadora.exe`.

## Testing your server

Enable `test-client` in `[dev-dependencies]` to drive a server in process
through the real wire format:

```rust
use flymcprust::testing::TestClient;

#[tokio::test]
async fn adds() {
    let client = TestClient::new(my_server().unwrap());
    let result = client.call_tool("add", serde_json::json!({ "a": 1, "b": 2 })).await.unwrap();
    assert_eq!(result.is_error, None);
}
```

## Benchmarks

### Against rmcp and the official Python SDK

[`bench/`](bench) runs the same server (same tools, MCP 2026-07-28,
Streamable HTTP/1.1, JSON responses) written with FlyMCPRust, `rmcp` 3.7.0
and the official Python SDK `mcp` 2.3.0, under one load generator. Full
tables, method and caveats: [bench/results/RESULTS.md](bench/results/RESULTS.md).

At 64 concurrent clients (laptop, load generator on the same machine,
2026-10-25):

| Scenario | FlyMCPRust req/s | rmcp req/s | Python req/s | CPU per request vs rmcp | vs Python |
| --- | ---: | ---: | ---: | ---: | ---: |
| `tools/list` | 79,051 | 42,164 | 2,024 | 2.3× less | 9.3× less |
| `tools/call` (trivial tool) | 59,920 | 37,156 | 1,816 | 2.6× less | 9.8× less |
| `tools/call` (structured output) | 58,258 | 35,098 | 1,702 | 2.3× less | 8.7× less |
| `tools/call` (64 KiB in and out) | 20,442 | 19,983 | 920 | 1.1× less | 3.8× less |

Idle memory: 5.9 MiB (rmcp 6.3 MiB, Python 78 MiB). Binary: 1.9 MiB (rmcp
3.4 MiB). Large payloads are on par with rmcp: the time goes into JSON
parsing and serialization, which both SDKs share through `serde_json`.

On Linux (GitHub Actions runner, Intel Xeon, 4 cores, same suite), the gap
is wider: 2.4–2.6× the throughput of rmcp with 3.2–4.1× less CPU per request
on small messages, and 48–64× the throughput of the Python SDK. Idle memory:
4.2 MiB (rmcp 5.1 MiB, Python 73 MiB). Full tables:
[bench/results/linux-ci/RESULTS.md](bench/results/linux-ci/RESULTS.md).
Shared runners vary from run to run; compare servers within one run.

Reproduce with `cargo run --release --manifest-path bench/Cargo.toml -p
mcp-load -- suite` (see [bench/README.md](bench/README.md)).

### Micro-benchmarks

`cargo bench -p flymcprust-core` (criterion), measured on 2026-10-25 on an
Intel Core i5-12450HX laptop (16 GB, Windows 11) with Rust 1.99.1. Laptop
measurements vary by about ±20% between runs because of thermal throttling.
Each phase is checked against the previous code (interleaved runs, plus the
comparative suite): features such as multi round-trip requests, notifications
and legacy clients cost nothing on requests that do not use them.

| Benchmark | Median time | Notes |
| --- | --- | --- |
| `parse/tools_call_add` | 0.60 µs | JSON-RPC envelope plus limit checks (~300-byte message) |
| `parse/tools_list` | 0.71 µs | |
| `parse/tools_call_64KiB` | 9.7 µs | Including the depth pre-scan (48.9 µs before it skipped strings with `memchr`) |
| `dispatch/tools_list` | 1.50 µs | Bytes in → bytes out, catalog of 32 tools |
| `dispatch/tools_call_add` | 2.76 µs | Bytes in → bytes out: parse, `_meta` validation, argument deserialization, call, serialization |
| `dispatch/tools_call_64KiB` | 53 µs | Echo of a 64 KiB string (115 µs before the `memchr` scan) |
| `serialize/call_result` | 0.10 µs | Response envelope |

## Security

Secure defaults are on without configuration: message, depth, time and
concurrency limits, localhost-only HTTP, panic isolation, sanitized errors,
signed MRTR state. [docs/SECURITY.md](docs/SECURITY.md) has the threat model,
what your application must still do (authentication, `security::confine_path`
for files, idempotent code before `ctx.elicit`), and the review log. Report
vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

## Development

See [CLAUDE.md](CLAUDE.md) for conventions and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
for design decisions. Before each commit:

```bash
cargo fmt --all
```

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

```bash
cargo test --workspace --all-features
```

```bash
cargo deny check
```

Fuzzing (nightly): run `cargo install cargo-fuzz`, then
`cargo +nightly fuzz run parse_message` from `fuzz/`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
