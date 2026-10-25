# SDKMCPRust

> Working title. A Rust SDK for building [Model Context Protocol](https://modelcontextprotocol.io)
> servers, with the ergonomics of FastMCP and the guarantees of Rust.

**Status:** phase 1 (tools over stdio and Streamable HTTP). Not published yet.

- **Spec-first.** Targets MCP revision **2026-07-28**: stateless, with no
  `initialize` and no sessions. Protocol types are generated from the official
  `schema.json`/`schema.ts`, and deprecations become `#[deprecated]`.
- **Ergonomic.** Tools are plain (async) functions. Input and output JSON
  Schemas, descriptions and annotations come from the signature and doc
  comments.
- **Safe.** No `unsafe`, no panics on the request path, and secure defaults:
  message size, JSON depth, tool timeout, concurrency, Origin/Host checks,
  localhost binding. Internal errors are never exposed to clients.
- **Fast.** Parsing borrows the request buffer, tool arguments deserialize
  directly into their struct, and list responses are pre-serialized.
- **Composable.** The server is a `tower::Service`, and so is the HTTP
  transport, which mounts in axum.

## Quick start

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

## Features

| Feature | Default | Description |
| --- | --- | --- |
| `macros` | yes | The `#[tool]` attribute. |
| `stdio` | no | stdio transport. |
| `http` | no | Streamable HTTP transport (`tower::Service` plus a hyper server). |
| `auto-register` | no | `ServerBuilder::auto_tools()`. |
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
| Resources, prompts, completion, subscriptions, MRTR, Tasks, Apps, OAuth | Planned; the architecture already reserves their place ([ARCHITECTURE](docs/ARCHITECTURE.md) §12) |
| 2025-11-25 (legacy) clients | Planned (transport-edge adapter). Today they get an error naming the supported versions |

### Official conformance suite

[`examples/conformance-server`](examples/conformance-server) exposes the
fixtures of `@modelcontextprotocol/conformance`, and CI runs every 2026-07-28
server scenario against it. Known gaps are listed in
[`conformance-baseline.yml`](conformance-baseline.yml), and CI fails on any
other failure or on a stale entry.

Results with `@modelcontextprotocol/conformance@0.2.0-alpha.12` on 2026-10-25:
**108 checks pass and 26 fail**. Every failure is in a feature not built yet
(resources, prompts, completion, progress, logging, MRTR).

| Scenario | Result |
| --- | --- |
| `server-stateless` | 23/25 (the 2 failures need logging/MRTR fixtures) |
| `tools-list`, `tools-call-{simple-text,image,audio,embedded-resource,mixed-content,error}` | all pass |
| `json-schema-2020-12` | 8/8 |
| `http-header-validation` | 14/14 |
| `http-custom-header-server-validation` | 10/10 |
| `dns-rebinding-protection` | 2/2 |
| `caching` | 4/7 (the 3 failures are prompts/resources lists) |

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

`cargo bench -p sdkmcprust-core` (criterion). Phase 1 baseline, measured on
2026-10-25 on an Intel Core i5-12450HX laptop (16 GB, Windows 11) with Rust
1.99.1. Laptop measurements vary by about ±20% between runs because of
thermal throttling.

| Benchmark | Median time | Notes |
| --- | --- | --- |
| `parse/tools_call_add` | 0.60 µs | JSON-RPC envelope plus limit checks (~300-byte message) |
| `parse/tools_list` | 0.71 µs | |
| `parse/tools_call_64KiB` | 48.9 µs | 1.25 GiB/s, including the depth pre-scan |
| `dispatch/tools_list` | 1.50 µs | Bytes in → bytes out, catalog of 32 tools |
| `dispatch/tools_call_add` | 2.76 µs | Bytes in → bytes out: parse, `_meta` validation, argument deserialization, call, serialization |
| `dispatch/tools_call_64KiB` | 115 µs | Echo of a 64 KiB string |
| `serialize/call_result` | 0.10 µs | Response envelope |

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
