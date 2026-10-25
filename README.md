# SDKMCPRust

> Working title. A Rust SDK for building [Model Context Protocol](https://modelcontextprotocol.io)
> servers, with the ergonomics of FastMCP and the guarantees of Rust.

**Status:** phase 2 (tools, resources, prompts, completion and multi
round-trip requests over stdio and Streamable HTTP). Not published yet.

- **Spec-first.** Targets MCP revision **2026-07-28**: stateless, with no
  `initialize` and no sessions. Protocol types are generated from the official
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
sdkmcprust = { git = "…", features = ["stdio"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use sdkmcprust::prelude::*;

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

## Features

| Feature | Default | Description |
| --- | --- | --- |
| `macros` | yes | The `#[tool]`, `#[resource]` and `#[prompt]` attributes. |
| `stdio` | no | stdio transport. |
| `http` | no | Streamable HTTP transport (`tower::Service` plus a hyper server). |
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
| Progress and logging notifications, response streaming | Planned |
| Subscriptions, Tasks, Apps, OAuth | Planned; the architecture already reserves their place ([ARCHITECTURE](docs/ARCHITECTURE.md) §12) |
| 2025-11-25 (legacy) clients | Planned (transport-edge adapter). Today they get an error naming the supported versions |

### Official conformance suite

[`examples/conformance-server`](examples/conformance-server) exposes the
fixtures of `@modelcontextprotocol/conformance`, and CI runs every 2026-07-28
server scenario against it. Known gaps are listed in
[`conformance-baseline.yml`](conformance-baseline.yml), and CI fails on any
other failure or on a stale entry.

Results with `@modelcontextprotocol/conformance@0.2.0-alpha.12` on 2026-10-25:
**144 checks pass and 3 fail**, with no warnings. The 3 failures need progress
notifications, logging notifications and response streaming, which are not
built yet.

| Scenario | Result |
| --- | --- |
| `server-stateless` | 23/25 (the 2 failures need logging and streaming fixtures) |
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
| `tools-call-with-progress` | 1/2 (progress notifications) |

To reproduce locally, run `cargo run -p conformance-server`, then:

```bash
npx @modelcontextprotocol/conformance@0.2.0-alpha.12 server --url http://127.0.0.1:3000/mcp --spec-version 2026-07-28 --suite all --expected-failures conformance-baseline.yml
```

## Try it with the MCP Inspector

The example server lives in [`examples/calculadora`](examples/calculadora).
The MCP Inspector v2 needs `--protocol-era modern` to use the stateless
protocol; its default is the legacy `initialize` handshake.

```bash
cargo build -p calculadora
```

**stdio, CLI:**

```bash
npx @modelcontextprotocol/inspector --cli target/debug/calculadora --protocol-era modern --method tools/list
```

```bash
npx @modelcontextprotocol/inspector --cli target/debug/calculadora --protocol-era modern --method tools/call --tool-name divide --tool-args-json '{"a":10,"b":4}'
```

**Streamable HTTP:** start the server, then point the Inspector at it.

```bash
cargo run -p calculadora -- --http 127.0.0.1:8000
```

```bash
npx @modelcontextprotocol/inspector --cli --transport http --server-url http://127.0.0.1:8000/mcp --protocol-era modern --method tools/list
```

**Web UI:** run `npx @modelcontextprotocol/inspector`. Add a server with
command `target/debug/calculadora` (stdio) or URL `http://127.0.0.1:8000/mcp`
(Streamable HTTP), set its protocol era to **modern**, and connect.

On Windows, use `target\debug\calculadora.exe`.

## Testing your server

Enable `test-client` in `[dev-dependencies]` to drive a server in process
through the real wire format:

```rust
use sdkmcprust::testing::TestClient;

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
Streamable HTTP/1.1, JSON responses) written with SDKMCPRust, `rmcp` 3.7.0
and the official Python SDK `mcp` 2.3.0, under one load generator. Full
tables, method and caveats: [bench/results/RESULTS.md](bench/results/RESULTS.md).

At 64 concurrent clients (laptop, load generator on the same machine,
2026-10-25):

| Scenario | SDKMCPRust req/s | rmcp req/s | Python req/s | CPU per request vs rmcp | vs Python |
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

`cargo bench -p sdkmcprust-core` (criterion). Phase 1 baseline, measured on
2026-10-25 on an Intel Core i5-12450HX laptop (16 GB, Windows 11) with Rust
1.99.1. Laptop measurements vary by about ±20% between runs because of
thermal throttling. Phase 2 was compared with this code by interleaving runs
of both versions on the same machine: the `tools/call` path showed no
measurable change (MRTR digests are computed only when input is requested).

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
