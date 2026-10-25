# SDKMCPRust

> Working title. A Rust SDK for building [Model Context Protocol](https://modelcontextprotocol.io)
> servers, with the ergonomics of FastMCP and the guarantees of Rust.

**Status:** early development (phase 1). Not published yet.

- Targets MCP spec revision **2026-07-28** (stateless: no `initialize`, no sessions).
- Tools are plain (async) functions; JSON Schemas come from the signature.
- No `unsafe`, no panics on the request path, secure defaults.
- Protocol types are generated from the official schema.

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
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for design decisions.

## Benchmarks

Numbers will be recorded here at each milestone.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
