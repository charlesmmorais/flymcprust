# Benchmark results

Date: 2026-10-26, commit `684f52d`. Machine: AMD EPYC 9V45 96-Core Processor, 4 logical cores (x86_64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.12.3, uvicorn 0.54.0). All servers speak MCP 2026-07-28, stateless; over HTTP/1.1 with JSON responses bound to 127.0.0.1; release builds for Rust.

## Startup, memory and size

| Server | Transport | Startup to first response | Idle memory (RSS) | Artifact |
| --- | --- | ---: | ---: | ---: |
| flymcprust | http | 4 ms | 5.1 MiB | 4.8 MiB (bench-flymcprust) |
| rmcp | http | 4 ms | 5.4 MiB | 4.3 MiB (bench-rmcp) |
| flymcprust (1 thread) | http | 3 ms | 5.0 MiB | 4.8 MiB (bench-flymcprust) |
| rmcp (1 thread) | http | 4 ms | 5.4 MiB | 4.3 MiB (bench-rmcp) |
| python mcp (1 process) | http | 796 ms | 73.0 MiB | 90.3 MiB (.venv) |
| flymcprust + OAuth (JWT RS256) | http | 4 ms | 5.5 MiB | 4.8 MiB (bench-flymcprust) |
| flymcprust | stdio | 2 ms | 5.2 MiB | 4.8 MiB (bench-flymcprust) |
| rmcp | stdio | 2 ms | 4.8 MiB | 4.3 MiB (bench-rmcp) |
| python mcp (1 process) | stdio | 345 ms | 65.7 MiB | 90.3 MiB (.venv) |

## Streamable HTTP

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 32,267 | 162,474 | 165,345 | 170,259 | 164,632 | 4% | 30 µs | 692 µs | 12 ms | 6.6 MiB |
| rmcp | 19,016 | 56,926 | 54,108 | 51,350 | 47,985 | 5% | 51 µs | 1,847 µs | 37 ms | 13.2 MiB |
| flymcprust (1 thread) | 33,592 | 108,332 | 110,660 | 108,188 | 103,950 | 2% | 29 µs | 785 µs | 9 ms | 5.9 MiB |
| rmcp (1 thread) | 23,780 | 40,091 | 37,940 | 37,681 | 36,614 | 3% | 41 µs | 1,963 µs | 26 ms | 10.7 MiB |
| python mcp (1 process) | 3,690 | 3,913 | 4,066 | 4,084 | 3,840 | 3% | 263 µs | 38,079 µs | 246 ms | 73.0 MiB |
| flymcprust + OAuth (JWT RS256) | 19,642 | 46,198 | 46,936 | 47,567 | 46,043 | 2% | 50 µs | 2,213 µs | 43 ms | 6.8 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 30,908 | 141,480 | 146,494 | 145,178 | 141,286 | 2% | 32 µs | 740 µs | 13 ms | 17.6 MiB |
| rmcp | 16,136 | 45,271 | 44,731 | 45,097 | 43,539 | 1% | 61 µs | 2,251 µs | 44 ms | 44.7 MiB |
| flymcprust (1 thread) | 32,257 | 93,308 | 94,094 | 93,687 | 90,659 | 4% | 30 µs | 740 µs | 11 ms | 14.4 MiB |
| rmcp (1 thread) | 21,125 | 32,575 | 33,146 | 32,341 | 31,440 | 0% | 46 µs | 2,273 µs | 30 ms | 67.2 MiB |
| python mcp (1 process) | 2,879 | 3,089 | 3,068 | 2,607 | 1,805 | 2% | 342 µs | 49,151 µs | 344 ms | 80.6 MiB |
| flymcprust + OAuth (JWT RS256) | 19,059 | 44,697 | 44,964 | 45,347 | 43,948 | 0% | 51 µs | 2,293 µs | 44 ms | 16.6 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 25,993 | 91,245 | 91,940 | 91,619 | 88,632 | 2% | 37 µs | 1,153 µs | 22 ms | 17.7 MiB |
| rmcp | 14,750 | 35,411 | 36,370 | 35,873 | 34,174 | 5% | 65 µs | 2,901 µs | 55 ms | 46.2 MiB |
| flymcprust (1 thread) | 28,547 | 69,728 | 68,892 | 67,134 | 65,188 | 2% | 34 µs | 1,000 µs | 14 ms | 14.7 MiB |
| rmcp (1 thread) | 17,801 | 26,238 | 25,794 | 24,253 | 23,086 | 2% | 55 µs | 2,945 µs | 39 ms | 67.3 MiB |
| python mcp (1 process) | 2,573 | 2,820 | 2,716 | 2,306 | 1,767 | 4% | 385 µs | 54,239 µs | 386 ms | 107.9 MiB |
| flymcprust + OAuth (JWT RS256) | 17,970 | 39,202 | 40,151 | 40,610 | 38,891 | 2% | 55 µs | 2,645 µs | 50 ms | 16.9 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 10,153 | 17,120 | 18,118 | 17,980 | 17,310 | 2% | 96 µs | 5,775 µs | 110 ms | 26.0 MiB |
| rmcp | 9,116 | 17,079 | 16,759 | 14,884 | 14,048 | 1% | 108 µs | 5,999 µs | 119 ms | 78.0 MiB |
| flymcprust (1 thread) | 10,292 | 13,492 | 12,883 | 11,950 | 12,128 | 2% | 95 µs | 5,247 µs | 78 ms | 18.3 MiB |
| rmcp (1 thread) | 10,745 | 14,340 | 13,517 | 12,688 | 11,856 | 3% | 91 µs | 5,559 µs | 74 ms | 67.6 MiB |
| python mcp (1 process) | 1,702 | 1,731 | 1,679 | 1,501 | 1,266 | 4% | 585 µs | 74,687 µs | 622 ms | 123.2 MiB |
| flymcprust + OAuth (JWT RS256) | 8,450 | 14,190 | 13,973 | 13,780 | 13,689 | 3% | 116 µs | 7,331 µs | 143 ms | 23.1 MiB |

## stdio

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 23,092 | 145,323 | 261,753 | 1% | 43 µs | 417 µs | 7 ms | 5.9 MiB |
| rmcp | 20,531 | 23,718 | 23,816 | 0% | 48 µs | 2,931 µs | 62 ms | 6.0 MiB |
| python mcp (1 process) | 2,656 | 2,662 | 2,630 | 2% | 374 µs | 39,071 µs | 435 ms | 67.5 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 22,298 | 138,994 | 235,138 | 1% | 44 µs | 441 µs | 7 ms | 5.9 MiB |
| rmcp | 20,292 | 25,352 | 25,307 | 1% | 48 µs | 2,777 µs | 61 ms | 6.1 MiB |
| python mcp (1 process) | 2,420 | 2,371 | 2,319 | 0% | 410 µs | 55,455 µs | 510 ms | 67.8 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 19,006 | 96,730 | 134,121 | 1% | 51 µs | 807 µs | 14 ms | 6.2 MiB |
| rmcp | 16,779 | 22,237 | 22,368 | 1% | 58 µs | 3,501 µs | 69 ms | 6.7 MiB |
| python mcp (1 process) | 2,294 | 2,244 | 2,148 | 3% | 434 µs | 50,687 µs | 546 ms | 68.1 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 5,291 | 10,962 | 10,989 | 1% | 182 µs | 6,919 µs | 149 ms | 11.7 MiB |
| rmcp | 3,251 | 4,493 | 4,500 | 1% | 305 µs | 15,295 µs | 301 ms | 7.0 MiB |
| python mcp (1 process) | 1,758 | 1,709 | 1,684 | 2% | 566 µs | 62,879 µs | 718 ms | 74.5 MiB |

## FlyMCPRust relative to the others over HTTP (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 3.1× | 3.1× | 40.7× | 20.7× |
| `call_add` | 3.3× | 3.3× | 47.8× | 25.6× |
| `call_stats_100` | 2.5× | 2.5× | 33.9× | 17.9× |
| `call_echo_64KiB` | 1.1× | 1.1× | 10.8× | 5.7× |

## Cost of OAuth (c=64)

The same FlyMCPRust server with every request carrying an RS256 JWT access token, verified (signature, issuer, audience, expiry, scopes) on each request.

| Scenario | req/s without | req/s with | CPU per 1k req without | CPU per 1k req with |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 165,345 | 46,936 | 12 ms | 43 ms |
| `call_add` | 146,494 | 44,964 | 13 ms | 44 ms |
| `call_stats_100` | 91,940 | 40,151 | 22 ms | 50 ms |
| `call_echo_64KiB` | 18,118 | 13,973 | 110 ms | 143 ms |

## FlyMCPRust relative to the others over stdio (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 11.0× | 9.5× | 99.5× | 66.1× |
| `call_add` | 9.3× | 8.2× | 101.4× | 68.9× |
| `call_stats_100` | 6.0× | 5.1× | 62.4× | 40.4× |
| `call_echo_64KiB` | 2.4× | 2.0× | 6.5× | 4.8× |

Errors across all runs: 0.
