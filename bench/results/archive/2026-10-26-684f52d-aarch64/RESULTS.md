# Benchmark results

Date: 2026-10-26, commit `684f52d`. Machine: Neoverse-N2, 4 logical cores (aarch64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.12.3, uvicorn 0.54.0). All servers speak MCP 2026-07-28, stateless; over HTTP/1.1 with JSON responses bound to 127.0.0.1; release builds for Rust.

## Startup, memory and size

| Server | Transport | Startup to first response | Idle memory (RSS) | Artifact |
| --- | --- | ---: | ---: | ---: |
| flymcprust | http | 4 ms | 4.0 MiB | 4.6 MiB (bench-flymcprust) |
| rmcp | http | 4 ms | 4.3 MiB | 4.2 MiB (bench-rmcp) |
| flymcprust (1 thread) | http | 4 ms | 4.0 MiB | 4.6 MiB (bench-flymcprust) |
| rmcp (1 thread) | http | 4 ms | 4.2 MiB | 4.2 MiB (bench-rmcp) |
| python mcp (1 process) | http | 1,116 ms | 71.2 MiB | 87.7 MiB (.venv) |
| flymcprust + OAuth (JWT RS256) | http | 4 ms | 4.2 MiB | 4.6 MiB (bench-flymcprust) |
| flymcprust | stdio | 2 ms | 3.9 MiB | 4.6 MiB (bench-flymcprust) |
| rmcp | stdio | 2 ms | 3.7 MiB | 4.2 MiB (bench-rmcp) |
| python mcp (1 process) | stdio | 556 ms | 63.6 MiB | 87.7 MiB (.venv) |

## Streamable HTTP

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 23,858 | 135,360 | 151,786 | 163,903 | 134,622 | 1% | 41 µs | 696 µs | 13 ms | 5.6 MiB |
| rmcp | 9,881 | 31,737 | 31,385 | 29,268 | 26,180 | 2% | 99 µs | 3,173 µs | 61 ms | 12.0 MiB |
| flymcprust (1 thread) | 25,220 | 96,553 | 94,352 | 87,702 | 82,880 | 1% | 39 µs | 725 µs | 11 ms | 5.0 MiB |
| rmcp (1 thread) | 13,378 | 22,253 | 20,489 | 17,498 | 15,923 | 2% | 73 µs | 3,553 µs | 49 ms | 9.6 MiB |
| python mcp (1 process) | 1,644 | 1,750 | 1,756 | 1,693 | 1,673 | 3% | 605 µs | 42,207 µs | 571 ms | 71.2 MiB |
| flymcprust + OAuth (JWT RS256) | 13,027 | 40,878 | 42,073 | 42,417 | 39,600 | 0% | 75 µs | 2,469 µs | 47 ms | 5.5 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 20,673 | 106,155 | 110,787 | 117,005 | 106,557 | 2% | 47 µs | 955 µs | 18 ms | 16.3 MiB |
| rmcp | 8,852 | 30,581 | 31,442 | 28,605 | 25,143 | 1% | 111 µs | 3,207 µs | 63 ms | 69.8 MiB |
| flymcprust (1 thread) | 22,104 | 77,933 | 77,317 | 73,054 | 69,169 | 2% | 44 µs | 886 µs | 13 ms | 12.7 MiB |
| rmcp (1 thread) | 11,596 | 19,027 | 17,604 | 14,934 | 13,997 | 0% | 84 µs | 4,199 µs | 57 ms | 68.5 MiB |
| python mcp (1 process) | 1,278 | 1,401 | 1,360 | 1,258 | 1,059 | 1% | 778 µs | 74,559 µs | 752 ms | 77.9 MiB |
| flymcprust + OAuth (JWT RS256) | 12,134 | 38,652 | 40,141 | 40,107 | 37,478 | 1% | 81 µs | 2,573 µs | 50 ms | 14.3 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 18,370 | 63,507 | 63,273 | 65,623 | 61,244 | 4% | 53 µs | 1,795 µs | 30 ms | 16.8 MiB |
| rmcp | 7,447 | 22,209 | 22,056 | 18,952 | 16,977 | 3% | 132 µs | 4,715 µs | 87 ms | 69.8 MiB |
| flymcprust (1 thread) | 19,333 | 47,077 | 45,898 | 42,618 | 39,958 | 10% | 51 µs | 1,533 µs | 22 ms | 13.2 MiB |
| rmcp (1 thread) | 9,578 | 14,796 | 13,880 | 11,801 | 10,876 | 0% | 102 µs | 5,407 µs | 72 ms | 68.1 MiB |
| python mcp (1 process) | 1,167 | 1,278 | 1,241 | 1,133 | 974 | 2% | 853 µs | 80,063 µs | 823 ms | 103.1 MiB |
| flymcprust + OAuth (JWT RS256) | 11,146 | 31,177 | 31,950 | 32,577 | 29,951 | 1% | 88 µs | 3,309 µs | 62 ms | 14.8 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 5,415 | 14,203 | 13,220 | 14,691 | 14,275 | 13% | 182 µs | 8,319 µs | 146 ms | 24.1 MiB |
| rmcp | 4,514 | 13,158 | 13,166 | 10,932 | 10,264 | 0% | 219 µs | 7,603 µs | 149 ms | 87.9 MiB |
| flymcprust (1 thread) | 5,564 | 7,670 | 7,522 | 7,302 | 6,998 | 1% | 177 µs | 9,375 µs | 133 ms | 17.8 MiB |
| rmcp (1 thread) | 5,013 | 7,563 | 7,075 | 5,841 | 5,454 | 1% | 196 µs | 10,647 µs | 141 ms | 77.2 MiB |
| python mcp (1 process) | 815 | 816 | 796 | 765 | 697 | 2% | 1,215 µs | 110,719 µs | 1,274 ms | 123.0 MiB |
| flymcprust + OAuth (JWT RS256) | 4,365 | 11,476 | 11,018 | 11,058 | 10,917 | 2% | 226 µs | 9,495 µs | 181 ms | 21.6 MiB |

## stdio

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 20,298 | 105,639 | 141,935 | 0% | 47 µs | 762 µs | 12 ms | 4.7 MiB |
| rmcp | 15,162 | 22,952 | 22,990 | 1% | 64 µs | 3,055 µs | 68 ms | 5.0 MiB |
| python mcp (1 process) | 1,349 | 1,358 | 1,294 | 1% | 735 µs | 70,719 µs | 799 ms | 65.2 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 18,898 | 96,526 | 130,036 | 0% | 51 µs | 853 µs | 14 ms | 4.7 MiB |
| rmcp | 15,362 | 23,892 | 23,883 | 0% | 64 µs | 2,981 µs | 66 ms | 5.0 MiB |
| python mcp (1 process) | 1,194 | 1,174 | 1,140 | 1% | 830 µs | 62,655 µs | 928 ms | 65.4 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 16,892 | 62,219 | 71,301 | 1% | 58 µs | 1,544 µs | 25 ms | 5.0 MiB |
| rmcp | 12,744 | 22,967 | 22,717 | 0% | 79 µs | 3,285 µs | 75 ms | 5.6 MiB |
| python mcp (1 process) | 1,112 | 1,094 | 1,055 | 0% | 892 µs | 63,871 µs | 1,005 ms | 65.8 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 3,749 | 8,202 | 8,192 | 1% | 265 µs | 10,551 µs | 190 ms | 11.3 MiB |
| rmcp | 3,028 | 4,348 | 4,339 | 1% | 328 µs | 15,375 µs | 311 ms | 5.8 MiB |
| python mcp (1 process) | 905 | 890 | 865 | 1% | 1,097 µs | 77,759 µs | 1,260 ms | 72.1 MiB |

## FlyMCPRust relative to the others over HTTP (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 4.8× | 4.7× | 86.4× | 44.2× |
| `call_add` | 3.5× | 3.6× | 81.4× | 42.6× |
| `call_stats_100` | 2.9× | 2.9× | 51.0× | 27.2× |
| `call_echo_64KiB` | 1.0× | 1.0× | 16.6× | 8.7× |

## Cost of OAuth (c=64)

The same FlyMCPRust server with every request carrying an RS256 JWT access token, verified (signature, issuer, audience, expiry, scopes) on each request.

| Scenario | req/s without | req/s with | CPU per 1k req without | CPU per 1k req with |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 151,786 | 42,073 | 13 ms | 47 ms |
| `call_add` | 110,787 | 40,141 | 18 ms | 50 ms |
| `call_stats_100` | 63,273 | 31,950 | 30 ms | 62 ms |
| `call_echo_64KiB` | 13,220 | 11,018 | 146 ms | 181 ms |

## FlyMCPRust relative to the others over stdio (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 6.2× | 5.6× | 109.7× | 65.3× |
| `call_add` | 5.4× | 4.9× | 114.1× | 68.5× |
| `call_stats_100` | 3.1× | 3.0× | 67.6× | 40.2× |
| `call_echo_64KiB` | 1.9× | 1.6× | 9.5× | 6.6× |

Errors across all runs: 0.
