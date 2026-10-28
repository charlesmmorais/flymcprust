# Benchmark results

Date: 2026-10-26, commit `47f1c60`. Machine: AMD EPYC 9V74 80-Core Processor, 4 logical cores (x86_64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.12.3, uvicorn 0.54.0). All servers speak MCP 2026-07-28, stateless; over HTTP/1.1 with JSON responses bound to 127.0.0.1; release builds for Rust.

## Startup, memory and size

| Server | Transport | Startup to first response | Idle memory (RSS) | Artifact |
| --- | --- | ---: | ---: | ---: |
| flymcprust | http | 4 ms | 5.9 MiB | 7.4 MiB (bench-flymcprust) |
| rmcp | http | 4 ms | 6.7 MiB | 8.4 MiB (bench-rmcp) |
| flymcprust (1 thread) | http | 4 ms | 5.7 MiB | 7.4 MiB (bench-flymcprust) |
| rmcp (1 thread) | http | 4 ms | 6.4 MiB | 8.4 MiB (bench-rmcp) |
| python mcp (1 process) | http | 1,213 ms | 73.2 MiB | 90.3 MiB (.venv) |
| flymcprust + OAuth (JWT RS256) | http | 4 ms | 5.9 MiB | 7.4 MiB (bench-flymcprust) |
| flymcprust | stdio | 3 ms | 5.4 MiB | 7.4 MiB (bench-flymcprust) |
| rmcp | stdio | 3 ms | 5.6 MiB | 8.4 MiB (bench-rmcp) |
| python mcp (1 process) | stdio | 589 ms | 65.6 MiB | 90.3 MiB (.venv) |

## Streamable HTTP

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 19,189 | 85,497 | 86,552 | 85,791 | 87,801 | 6% | 50 µs | 1,304 µs | 23 ms | 7.2 MiB |
| rmcp | 9,267 | 34,428 | 33,184 | 29,016 | 26,177 | 2% | 105 µs | 3,033 µs | 60 ms | 14.0 MiB |
| flymcprust (1 thread) | 19,400 | 69,221 | 69,770 | 67,443 | 62,584 | 1% | 49 µs | 1,318 µs | 14 ms | 6.6 MiB |
| rmcp (1 thread) | 12,308 | 30,122 | 28,198 | 26,723 | 25,074 | 1% | 78 µs | 2,673 µs | 35 ms | 11.7 MiB |
| python mcp (1 process) | 2,004 | 2,180 | 2,223 | 2,181 | 2,126 | 1% | 492 µs | 32,175 µs | 451 ms | 73.4 MiB |
| flymcprust + OAuth (JWT RS256) | 10,671 | 21,346 | 21,422 | 21,573 | 20,947 | 0% | 90 µs | 4,527 µs | 93 ms | 7.2 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 17,164 | 68,392 | 66,276 | 66,900 | 67,716 | 1% | 56 µs | 1,634 µs | 30 ms | 16.1 MiB |
| rmcp | 9,151 | 33,668 | 33,194 | 29,485 | 26,253 | 2% | 106 µs | 3,033 µs | 60 ms | 57.0 MiB |
| flymcprust (1 thread) | 17,574 | 58,534 | 58,660 | 57,479 | 53,579 | 1% | 54 µs | 1,674 µs | 17 ms | 15.1 MiB |
| rmcp (1 thread) | 11,679 | 29,677 | 28,078 | 26,236 | 25,218 | 1% | 82 µs | 2,739 µs | 36 ms | 67.1 MiB |
| python mcp (1 process) | 1,573 | 1,814 | 1,742 | 1,558 | 1,311 | 1% | 633 µs | 71,871 µs | 604 ms | 79.9 MiB |
| flymcprust + OAuth (JWT RS256) | 9,860 | 19,233 | 19,337 | 19,347 | 19,656 | 0% | 98 µs | 5,003 µs | 103 ms | 15.2 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 15,237 | 44,731 | 44,799 | 43,592 | 43,875 | 0% | 63 µs | 2,367 µs | 44 ms | 16.7 MiB |
| rmcp | 7,542 | 23,595 | 23,848 | 21,717 | 19,631 | 1% | 129 µs | 4,319 µs | 83 ms | 52.5 MiB |
| flymcprust (1 thread) | 15,596 | 40,808 | 40,674 | 39,691 | 37,657 | 1% | 61 µs | 1,699 µs | 25 ms | 15.5 MiB |
| rmcp (1 thread) | 9,625 | 19,485 | 19,030 | 18,055 | 17,498 | 0% | 100 µs | 4,037 µs | 53 ms | 80.0 MiB |
| python mcp (1 process) | 1,418 | 1,632 | 1,556 | 1,293 | 1,076 | 3% | 701 µs | 77,439 µs | 673 ms | 108.2 MiB |
| flymcprust + OAuth (JWT RS256) | 9,171 | 16,884 | 17,117 | 17,247 | 17,996 | 0% | 105 µs | 5,651 µs | 117 ms | 16.4 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | req/s c=1024 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 4,996 | 7,950 | 7,653 | 7,744 | 7,901 | 6% | 194 µs | 13,535 µs | 261 ms | 24.7 MiB |
| rmcp | 4,418 | 8,622 | 8,546 | 7,740 | 7,396 | 1% | 221 µs | 11,719 µs | 234 ms | 92.3 MiB |
| flymcprust (1 thread) | 5,054 | 5,971 | 6,754 | 6,406 | 6,347 | 15% | 191 µs | 10,103 µs | 148 ms | 19.7 MiB |
| rmcp (1 thread) | 4,767 | 7,379 | 7,144 | 6,634 | 6,323 | 0% | 210 µs | 10,287 µs | 140 ms | 80.0 MiB |
| python mcp (1 process) | 894 | 935 | 896 | 869 | 768 | 8% | 1,103 µs | 124,927 µs | 1,157 ms | 122.8 MiB |
| flymcprust + OAuth (JWT RS256) | 3,994 | 5,975 | 6,145 | 6,237 | 6,160 | 1% | 246 µs | 16,655 µs | 325 ms | 23.1 MiB |

## stdio

### `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 13,939 | 83,568 | 149,155 | 0% | 70 µs | 779 µs | 12 ms | 6.1 MiB |
| rmcp | 12,717 | 15,588 | 15,515 | 0% | 75 µs | 4,623 µs | 96 ms | 6.8 MiB |
| python mcp (1 process) | 1,229 | 1,236 | 1,158 | 2% | 809 µs | 85,311 µs | 996 ms | 67.3 MiB |

### `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 13,294 | 77,369 | 128,320 | 0% | 74 µs | 827 µs | 14 ms | 6.2 MiB |
| rmcp | 12,983 | 16,066 | 16,100 | 0% | 75 µs | 4,315 µs | 96 ms | 6.9 MiB |
| python mcp (1 process) | 1,109 | 1,063 | 1,054 | 2% | 896 µs | 89,343 µs | 1,120 ms | 67.6 MiB |

### `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 11,831 | 52,592 | 74,189 | 1% | 83 µs | 1,447 µs | 25 ms | 6.5 MiB |
| rmcp | 10,424 | 14,633 | 14,550 | 0% | 94 µs | 4,871 µs | 111 ms | 7.5 MiB |
| python mcp (1 process) | 1,055 | 1,028 | 999 | 0% | 941 µs | 94,719 µs | 1,176 ms | 68.0 MiB |

### `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | spread c=64 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 2,695 | 4,549 | 4,532 | 1% | 368 µs | 16,199 µs | 364 ms | 9.6 MiB |
| rmcp | 2,180 | 3,035 | 3,020 | 1% | 457 µs | 23,055 µs | 476 ms | 7.8 MiB |
| python mcp (1 process) | 773 | 737 | 738 | 3% | 1,287 µs | 113,343 µs | 1,632 ms | 74.4 MiB |

## FlyMCPRust relative to the others over HTTP (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 2.6× | 2.6× | 38.9× | 19.8× |
| `call_add` | 2.0× | 2.0× | 38.1× | 20.2× |
| `call_stats_100` | 1.9× | 1.9× | 28.8× | 15.2× |
| `call_echo_64KiB` | 0.9× | 0.9× | 8.5× | 4.4× |

## Cost of OAuth (c=64)

The same FlyMCPRust server with every request carrying an RS256 JWT access token, verified (signature, issuer, audience, expiry, scopes) on each request.

| Scenario | req/s without | req/s with | CPU per 1k req without | CPU per 1k req with |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 86,552 | 21,422 | 23 ms | 93 ms |
| `call_add` | 66,276 | 19,337 | 30 ms | 103 ms |
| `call_stats_100` | 44,799 | 17,117 | 44 ms | 117 ms |
| `call_echo_64KiB` | 7,653 | 6,145 | 261 ms | 325 ms |

## FlyMCPRust relative to the others over stdio (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 9.6× | 8.3× | 128.8× | 85.8× |
| `call_add` | 8.0× | 6.9× | 121.7× | 81.3× |
| `call_stats_100` | 5.1× | 4.5× | 74.3× | 47.3× |
| `call_echo_64KiB` | 1.5× | 1.3× | 6.1× | 4.5× |

Errors across all runs: 0.
