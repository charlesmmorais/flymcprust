# Comparative benchmarks

The same MCP server written with three SDKs, driven by one load generator:

| Server | SDK | Code |
| --- | --- | --- |
| `flymcprust` | This repository | [servers/flymcprust](servers/flymcprust/src/main.rs) |
| `rmcp` | Official Rust SDK, `rmcp` 3.7.0 | [servers/rmcp](servers/rmcp/src/main.rs) |
| `python mcp` | Official Python SDK, `mcp` 2.3.0 (`MCPServer` API) | [servers/python](servers/python/server.py) |

Each server has the same three tools (`add`, `echo`, `stats` with structured
output) and speaks MCP 2026-07-28 over Streamable HTTP/1.1 with JSON
responses (no SSE), stateless, bound to `127.0.0.1`. The Rust servers use the
same release profile (`lto = "thin"`, `codegen-units = 1`) and hyper's
HTTP/1.1 server.

Results: [results/RESULTS.md](results/RESULTS.md) (Windows laptop) and
[results/linux-ci/RESULTS.md](results/linux-ci/RESULTS.md) (Linux GitHub
Actions runner), with raw data next to each.

## Run it

Requirements: Rust (stable) and [uv](https://docs.astral.sh/uv/).

```bash
cargo build --release --manifest-path bench/Cargo.toml
```

```bash
uv sync --project bench/servers/python
```

```bash
cargo run --release --manifest-path bench/Cargo.toml -p mcp-load -- suite
```

```bash
python bench/report.py
```

`suite --quick` runs a shorter version. `mcp-load run 127.0.0.1:8001
call_add 64 10` measures a server you started yourself.

## Method

- **Closed loop.** Each simulated client owns one keep-alive connection and
  sends its next request as soon as the previous response arrives.
  Concurrency levels: 1, 16, 64 and 256 clients.
- **Scenarios.** `tools_list`; `call_add` (trivial tool); `call_stats_100`
  (100 numbers in, structured output); `call_echo_64KiB` (large payload in
  and out).
- **Latency.** HDR histogram per client, recorded after a warm-up, merged.
  Every response must be HTTP 200 with a body, or it counts as an error.
- **Server cost.** CPU time and peak memory (RSS) of the server process tree
  (on Windows a venv's `python.exe` launches the real interpreter as a
  child), sampled every 100 ms during the measured window. CPU per request is
  the most portable efficiency figure: it barely depends on the load
  generator.
- **Startup.** From process spawn to the first successful `server/discover`.
- **Artifact.** The release binary for Rust; the virtual environment
  (interpreter link and dependencies) for Python.
- **Threads.** Rust servers run with Tokio's default (one worker per core)
  and with `TOKIO_WORKER_THREADS=1`. The Python server is one process, as
  `mcp.run()` starts it; Python scales with more processes (`uvicorn
  --workers`), which multiplies its memory.

## Caveats

- The load generator shares the machine with the server. At high
  throughput it competes for CPU, so absolute requests per second are lower
  bounds; compare servers to each other, and prefer the CPU-per-request
  figures.
- Laptops throttle with heat. For publishable numbers use a quiet machine,
  run the suite twice and check the runs agree.
- These are protocol-overhead benchmarks: the tools do almost nothing. In a
  real server, tool work (database, network) adds the same cost to every
  SDK.
