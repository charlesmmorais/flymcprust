# Benchmark results

Date: 2026-10-26, commit `47f1c60`. Machine: Neoverse-N2, 4 logical cores (aarch64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.12.3, uvicorn 0.54.0). All servers speak MCP 2026-07-28, stateless; over HTTP/1.1 with JSON responses bound to 127.0.0.1; release builds for Rust.

## Startup, memory and size

| Server | Transport | Startup to first response | Idle memory (RSS) | Artifact |
| --- | --- | ---: | ---: | ---: |
| flymcprust | http | 4 ms | 4.6 MiB | 6.8 MiB (bench-flymcprust) |
| rmcp | http | 4 ms | 5.4 MiB | 7.9 MiB (bench-rmcp) |
| flymcprust (1 thread) | http | 4 ms | 4.6 MiB | 6.8 MiB (bench-flymcprust) |
| rmcp (1 thread) | http | 4 ms | 5.3 MiB | 7.9 MiB (bench-rmcp) |
| python mcp (1 process) | http | 1,141 ms | 71.2 MiB | 87.7 MiB (.venv) |
| flymcprust + OAuth (JWT RS256) | http | 4 ms | 4.9 MiB | 6.8 MiB (bench-flymcprust) |
| flymcprust | stdio | 2 ms | 4.6 MiB | 6.8 MiB (bench-flymcprust) |
| rmcp | stdio | 2 ms | 4.8 MiB | 7.9 MiB (bench-rmcp) |
| python mcp (1 process) | stdio | 550 ms | 63.6 MiB | 87.7 MiB (.venv) |

## Streamable HTTP

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 23,833 | 136,139 | 154,210 | 161,705 | 152,246 | 10% | 41 µs | 676 µs | 13 ms | 6.1 MiB |
| rmcp | 12,416 | 38,215 | 37,491 | 34,100 | 30,702 | 2% | 79 µs | 2,739 µs | 52 ms | 13.1 MiB |
| flymcprust (1 thread) | 25,035 | 92,674 | 91,763 | 85,541 | 79,210 | 2% | 39 µs | 759 µs | 11 ms | 5.6 MiB |
| rmcp (1 thread) | 14,880 | 25,618 | 23,416 | 18,205 | 17,007 | 1% | 66 µs | 3,147 µs | 43 ms | 10.7 MiB |
| python mcp (1 process) | 1,557 | 1,701 | 1,672 | 1,636 | 1,607 | 0% | 639 µs | 39,295 µs | 600 ms | 71.2 MiB |
| flymcprust + OAuth (JWT RS256) | 12,863 | 40,363 | 42,206 | 42,414 | 39,618 | 1% | 76 µs | 2,439 µs | 47 ms | 6.2 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 20,780 | 112,327 | 123,088 | 130,289 | 121,108 | 3% | 47 µs | 879 µs | 16 ms | 14.7 MiB |
| rmcp | 10,434 | 38,258 | 39,255 | 34,836 | 30,540 | 1% | 95 µs | 2,589 µs | 50 ms | 53.3 MiB |
| flymcprust (1 thread) | 21,832 | 72,081 | 69,765 | 65,615 | 63,254 | 2% | 44 µs | 1,051 µs | 14 ms | 22.9 MiB |
| rmcp (1 thread) | 13,834 | 26,926 | 22,551 | 18,851 | 16,332 | 6% | 71 µs | 3,297 µs | 44 ms | 61.7 MiB |
| python mcp (1 process) | 1,266 | 1,371 | 1,345 | 1,255 | 1,135 | 1% | 784 µs | 76,159 µs | 758 ms | 78.9 MiB |
| flymcprust + OAuth (JWT RS256) | 11,862 | 38,070 | 39,399 | 39,138 | 36,506 | 1% | 82 µs | 2,613 µs | 51 ms | 16.2 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 18,000 | 68,430 | 69,326 | 69,841 | 63,824 | 3% | 54 µs | 1,568 µs | 28 ms | 14.7 MiB |
| rmcp | 8,290 | 28,748 | 29,900 | 25,474 | 21,992 | 1% | 119 µs | 3,485 µs | 66 ms | 53.3 MiB |
| flymcprust (1 thread) | 19,067 | 43,531 | 41,294 | 40,696 | 39,564 | 3% | 51 µs | 1,702 µs | 24 ms | 23.5 MiB |
| rmcp (1 thread) | 11,281 | 19,046 | 17,041 | 14,310 | 12,920 | 1% | 87 µs | 4,427 µs | 59 ms | 75.1 MiB |
| python mcp (1 process) | 1,153 | 1,277 | 1,253 | 1,142 | 1,032 | 1% | 863 µs | 80,447 µs | 815 ms | 105.2 MiB |
| flymcprust + OAuth (JWT RS256) | 10,733 | 30,493 | 31,125 | 34,008 | 29,328 | 1% | 91 µs | 3,391 µs | 63 ms | 16.2 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 5,176 | 13,229 | 12,904 | 14,616 | 13,953 | 17% | 191 µs | 8,311 µs | 150 ms | 22.7 MiB |
| rmcp | 4,892 | 15,156 | 15,298 | 12,862 | 11,582 | 1% | 202 µs | 6,587 µs | 130 ms | 83.5 MiB |
| flymcprust (1 thread) | 5,453 | 7,409 | 7,133 | 7,134 | 6,716 | 3% | 180 µs | 9,855 µs | 140 ms | 27.7 MiB |
| rmcp (1 thread) | 5,153 | 7,835 | 7,255 | 6,310 | 5,642 | 0% | 191 µs | 10,271 µs | 138 ms | 75.1 MiB |
| python mcp (1 process) | 804 | 823 | 810 | 760 | 694 | 1% | 1,237 µs | 108,927 µs | 1,248 ms | 121.3 MiB |
| flymcprust + OAuth (JWT RS256) | 4,204 | 10,719 | 10,965 | 10,704 | 10,562 | 2% | 235 µs | 9,407 µs | 182 ms | 23.1 MiB |

## stdio

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 21,268 | 108,079 | 146,209 | 1% | 45 µs | 735 µs | 12 ms | 5.3 MiB |
| rmcp | 17,926 | 23,753 | 23,767 | 1% | 54 µs | 2,953 µs | 60 ms | 5.8 MiB |
| python mcp (1 process) | 1,335 | 1,349 | 1,298 | 2% | 743 µs | 68,479 µs | 796 ms | 65.2 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 19,812 | 98,572 | 131,984 | 0% | 49 µs | 836 µs | 13 ms | 5.4 MiB |
| rmcp | 18,599 | 25,124 | 25,103 | 1% | 53 µs | 2,821 µs | 59 ms | 5.9 MiB |
| python mcp (1 process) | 1,182 | 1,176 | 1,153 | 10% | 840 µs | 75,839 µs | 918 ms | 65.4 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 17,999 | 66,313 | 74,416 | 0% | 54 µs | 1,491 µs | 24 ms | 5.6 MiB |
| rmcp | 14,611 | 25,059 | 24,912 | 1% | 68 µs | 2,981 µs | 66 ms | 6.4 MiB |
| python mcp (1 process) | 1,103 | 1,118 | 1,091 | 2% | 894 µs | 62,303 µs | 973 ms | 65.8 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 3,870 | 8,353 | 8,433 | 0% | 256 µs | 9,999 µs | 184 ms | 14.6 MiB |
| rmcp | 3,193 | 4,630 | 4,623 | 0% | 311 µs | 14,439 µs | 287 ms | 6.8 MiB |
| python mcp (1 process) | 906 | 898 | 887 | 1% | 1,093 µs | 75,711 µs | 1,228 ms | 72.1 MiB |

## FlyMCPRust relative to the others over HTTP (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 4.1× | 4.1× | 92.2× | 47.0× |
| `call_add` | 3.1× | 3.1× | 91.5× | 47.1× |
| `call_stats_100` | 2.3× | 2.3× | 55.3× | 29.1× |
| `call_echo_64KiB` | 0.8× | 0.9× | 15.9× | 8.3× |

## Cost of OAuth (c=64)

The same FlyMCPRust server with every request carrying an RS256 JWT access token, verified (signature, issuer, audience, expiry, scopes) on each request.

| Scenario | req/s without | req/s with | CPU per 1k req without | CPU per 1k req with |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 154,210 | 42,206 | 13 ms | 47 ms |
| `call_add` | 123,088 | 39,399 | 16 ms | 51 ms |
| `call_stats_100` | 69,326 | 31,125 | 28 ms | 63 ms |
| `call_echo_64KiB` | 12,904 | 10,965 | 150 ms | 182 ms |

## FlyMCPRust relative to the others over stdio (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 6.2× | 5.1× | 112.6× | 67.1× |
| `call_add` | 5.3× | 4.4× | 114.4× | 68.4× |
| `call_stats_100` | 3.0× | 2.7× | 68.2× | 40.4× |
| `call_echo_64KiB` | 1.8× | 1.6× | 9.5× | 6.7× |

Errors across all runs: 0.
