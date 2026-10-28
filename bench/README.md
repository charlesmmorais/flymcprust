# Comparative benchmarks

The same MCP server written with three SDKs, driven by one load generator:

| Server | SDK | Code |
| --- | --- | --- |
| `flymcprust` | This repository | [servers/flymcprust](servers/flymcprust/src/main.rs) |
| `rmcp` | Official Rust SDK, `rmcp` 3.7.0 | [servers/rmcp](servers/rmcp/src/main.rs) |
| `python mcp` | Official Python SDK, `mcp` 2.3.0 (`MCPServer` API) | [servers/python](servers/python/server.py) |

Each server has the same three tools (`add`, `echo`, `stats` with structured
output) and speaks MCP 2026-07-28, stateless, over Streamable HTTP/1.1 with
JSON responses (no SSE) bound to `127.0.0.1`, and over stdio. A sixth
target is the FlyMCPRust server requiring OAuth: every request carries an
RS256 JWT ([`fixtures/token.txt`](fixtures/token.txt), verified against
[`fixtures/jwks.json`](fixtures/jwks.json)). The Rust servers use the
same release profile (`lto = "thin"`, `codegen-units = 1`) and hyper's
HTTP/1.1 server.

Results, with raw data next to each, from GitHub Actions runners
(2026-10-26, commit `47f1c60`): x86-64
([basic](results/2026-10-26-47f1c60-x86_64/basic/RESULTS.md),
[real-world](results/2026-10-26-47f1c60-x86_64/real/RESULTS.md)) and ARM64
([basic](results/2026-10-26-47f1c60-aarch64/basic/RESULTS.md),
[real-world](results/2026-10-26-47f1c60-aarch64/real/RESULTS.md)); earlier
runs in [results/archive](results/archive).

## Run it

Requirements: Rust (stable) and [uv](https://docs.astral.sh/uv/).

```bash
cargo build --release --manifest-path bench/Cargo.toml
```

```bash
uv sync --project bench/servers/python
```

```bash
cargo run --release --manifest-path bench/Cargo.toml -p mcp-load -- suite --repeat 3
```

On Linux, keep the server and the load generator on separate CPUs (here
for 4 CPUs):

```bash
taskset -c 2-3 ./bench/target/release/mcp-load suite --repeat 3 --server-cpus 0-1
```

```bash
python bench/report.py
```

`suite --quick` runs a shorter version, and `--only TEXT` keeps the
targets whose name or transport contains `TEXT` (for example `stdio`). `mcp-load run 127.0.0.1:8001
call_add 64 10` measures a server you started yourself.

## Method

- **Closed loop.** Each simulated client owns one keep-alive connection and
  sends its next request as soon as the previous response arrives.
  Concurrency levels: 1, 16, 64, 256 and 1024 clients over HTTP; over stdio
  (one pipe) 1, 16 and 64 requests in flight.
- **Repetitions.** `--repeat N` measures every point N times; the report
  shows the median and the spread ((max − min) / median of the throughput).
- **CPU isolation.** `--server-cpus` pins the server with `taskset`; the
  load generator runs on the remaining CPUs. The GitHub workflow does this
  on an x86-64 and an ARM64 runner.
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

- The load generator shares the machine with the server. With
  `--server-cpus` they no longer share CPUs, but still share caches and
  memory bandwidth; absolute requests per second remain machine-specific, so
  compare servers to each other, and prefer the CPU-per-request figures.
- Shared CI runners are noisy neighbours: check the spread column.
- On Windows, `sysinfo` occasionally misses a CPU-time window; such points
  report no CPU figure rather than zero.
- Laptops throttle with heat. For publishable numbers use a quiet machine,
  run the suite twice and check the runs agree.
- The FlyMCPRust benchmark server raises `max_in_flight` and
  `max_connections` to 4096 for the 1024-client level; rmcp and uvicorn have
  no such limits by default.
- These are protocol-overhead benchmarks: the tools do almost nothing. In a
  real server, tool work (database, network) adds the same cost to every
  SDK.

## Real-world suite

`mcp-load real` compares FlyMCPRust and rmcp under workloads closer to
production: 200 tools with different schemas (generated for both SDKs by
[`gen_tools.py`](gen_tools.py)), a nested ~10 KB payload, 1,024 and 4,096
connections, a new connection per request, tools that call an HTTP
upstream answering after 20 ms (simulated by the load generator), and a
mixed load. Each runs with and without an RS256 JWT on every request,
checked in both servers by the same verifier, and FlyMCPRust also runs with
its default limits. Report: `python bench/report_real.py`.

The rmcp servers keep their tool router in the handler. rmcp's
`#[tool_router(server_handler)]` shortcut rebuilds the router on every
request; the runs before commit `47f1c60` used it, which inflated
FlyMCPRust's lead (see the archived results).

## Independent runs

These numbers come from this repository's own harness. To check them, run
the suite on your hardware (the commands above), or start the
[Benchmarks workflow](../.github/workflows/bench.yml) in a fork, and open an
issue with the generated `RESULTS.md` and `results.json`. Results that
disagree are as welcome as results that agree.
