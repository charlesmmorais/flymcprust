# Real-world benchmark results

Date: 2026-10-26, commit `47f1c60`. Machine: Neoverse-N2, 4 logical cores (aarch64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator (and the simulated upstream) to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

FlyMCPRust and rmcp 3.7.0 serve the same 203 tools over Streamable HTTP/1.1 (MCP 2026-07-28, stateless, JSON responses). `+ JWT` targets require an RS256 access token on every request, checked by the same verifier in both servers. `default limits` is FlyMCPRust without raising its secure defaults (64 concurrent tool calls, 256 requests in flight, 1,024 connections).

## Startup and idle memory (203 tools)

| Server | Startup to first response | Idle memory (RSS) |
| --- | ---: | ---: |
| flymcprust | 7 ms | 7.2 MiB |
| rmcp | 7 ms | 8.3 MiB |
| flymcprust + JWT | 7 ms | 7.4 MiB |
| rmcp + JWT | 7 ms | 8.6 MiB |
| flymcprust (default limits) | 7 ms | 7.2 MiB |

## `tools_list_200`

`tools/list` of 200 tools (~100 KB response).

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 53,121 | 22% | 1,187 µs | 2,209 µs | 37 ms | 8.2 MiB | 0 |
| rmcp | 64 | 6,538 | 3% | 10,079 µs | 15,591 µs | 305 ms | 25.2 MiB | 0 |
| flymcprust + JWT | 64 | 22,806 | 2% | 3,153 µs | 4,093 µs | 88 ms | 8.5 MiB | 0 |
| rmcp + JWT | 64 | 5,771 | 1% | 11,447 µs | 17,471 µs | 345 ms | 24.0 MiB | 0 |
| flymcprust (default limits) | 64 | 47,688 | 17% | 1,325 µs | 2,471 µs | 42 ms | 8.8 MiB | 0 |

## `call_last_of_200`

Calling the last of 200 tools.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 112,639 | 2% | 558 µs | 957 µs | 17 ms | 8.4 MiB | 0 |
| rmcp | 64 | 28,133 | 0% | 2,307 µs | 3,669 µs | 70 ms | 24.3 MiB | 0 |
| flymcprust + JWT | 64 | 39,902 | 1% | 1,616 µs | 2,609 µs | 50 ms | 8.5 MiB | 0 |
| rmcp + JWT | 64 | 18,986 | 8% | 3,423 µs | 5,819 µs | 105 ms | 22.6 MiB | 0 |
| flymcprust (default limits) | 64 | 123,733 | 3% | 505 µs | 849 µs | 16 ms | 8.9 MiB | 0 |

## `call_order_complex`

A nested order (~10 KB, 40 lines, enums) deserialized and summarized.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 24,859 | 3% | 2,605 µs | 4,259 µs | 79 ms | 8.5 MiB | 0 |
| rmcp | 64 | 6,838 | 1% | 9,495 µs | 16,527 µs | 288 ms | 25.6 MiB | 0 |
| flymcprust + JWT | 64 | 20,134 | 1% | 3,255 µs | 5,147 µs | 99 ms | 8.7 MiB | 0 |
| rmcp + JWT | 64 | 6,089 | 1% | 10,711 µs | 18,015 µs | 325 ms | 25.5 MiB | 0 |
| flymcprust (default limits) | 64 | 30,558 | 1% | 2,125 µs | 3,409 µs | 65 ms | 8.7 MiB | 0 |

## `call_add_many_clients`

A trivial tool, with 1,024 and 4,096 keep-alive connections.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 1,024 | 112,126 | 0% | 3,787 µs | 67,903 µs | 18 ms | 24.2 MiB | 0 |
| flymcprust | 4,096 | 108,254 | 1% | 3,909 µs | 369,663 µs | 18 ms | 74.8 MiB | 0 |
| rmcp | 1,024 | 20,212 | 1% | 10,927 µs | 295,935 µs | 98 ms | 174.4 MiB | 0 |
| rmcp | 4,096 | 18,680 | 3% | 12,127 µs | 1,362,943 µs | 106 ms | 563.8 MiB | 0 |
| flymcprust + JWT | 1,024 | 38,174 | 1% | 14,207 µs | 67,327 µs | 53 ms | 23.6 MiB | 0 |
| flymcprust + JWT | 4,096 | 37,033 | 1% | 14,663 µs | 355,839 µs | 55 ms | 74.5 MiB | 0 |
| rmcp + JWT | 1,024 | 14,629 | 0% | 16,943 µs | 300,799 µs | 136 ms | 165.5 MiB | 0 |
| rmcp + JWT | 4,096 | 14,149 | 3% | 17,631 µs | 1,381,375 µs | 141 ms | 553.5 MiB | 0 |
| flymcprust (default limits) | 1,024 | 122,035 | 3% | 3,441 µs | 69,375 µs | 16 ms | 23.8 MiB | 0 |
| flymcprust (default limits) | 4,096 | 120,967 | 4% | 3,465 µs | 68,479 µs | 17 ms | 24.0 MiB | 35 |

## `call_add_no_keepalive`

A trivial tool, with a new TCP connection for every request.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 40,298 | 2% | 884 µs | 1,674 µs | 45 ms | 46.9 MiB | 0 |
| rmcp | 64 | 19,525 | 6% | 3,175 µs | 5,127 µs | 102 ms | 364.8 MiB | 0 |
| flymcprust + JWT | 64 | 30,146 | 0% | 1,925 µs | 4,099 µs | 65 ms | 46.3 MiB | 0 |
| rmcp + JWT | 64 | 13,069 | 3% | 4,839 µs | 8,035 µs | 153 ms | 398.8 MiB | 0 |
| flymcprust (default limits) | 64 | 39,758 | 0% | 872 µs | 1,627 µs | 46 ms | 17.4 MiB | 0 |

## `call_fetch_upstream`

Each call waits for an HTTP upstream that answers after 20 ms.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 2,987 | 0% | 21,423 µs | 22,463 µs | 60 ms | 47.0 MiB | 0 |
| flymcprust | 256 | 11,733 | 0% | 21,791 µs | 23,439 µs | 52 ms | 47.3 MiB | 0 |
| flymcprust | 1,024 | 38,403 | 7% | 26,431 µs | 35,039 µs | 51 ms | 72.9 MiB | 0 |
| rmcp | 64 | 2,918 | 0% | 21,871 µs | 23,679 µs | 144 ms | 24.4 MiB | 0 |
| rmcp | 256 | 11,221 | 1% | 22,767 µs | 25,119 µs | 134 ms | 55.1 MiB | 0 |
| rmcp | 1,024 | 12,962 | 3% | 78,143 µs | 224,255 µs | 150 ms | 184.7 MiB | 0 |
| flymcprust + JWT | 64 | 2,987 | 0% | 21,375 µs | 22,527 µs | 97 ms | 44.6 MiB | 0 |
| flymcprust + JWT | 256 | 11,744 | 0% | 21,775 µs | 23,311 µs | 88 ms | 44.9 MiB | 0 |
| flymcprust + JWT | 1,024 | 22,254 | 4% | 35,743 µs | 102,399 µs | 89 ms | 74.8 MiB | 0 |
| rmcp + JWT | 64 | 2,913 | 0% | 21,903 µs | 23,695 µs | 179 ms | 22.6 MiB | 0 |
| rmcp + JWT | 256 | 11,107 | 3% | 22,991 µs | 25,135 µs | 166 ms | 54.5 MiB | 0 |
| rmcp + JWT | 1,024 | 10,702 | 3% | 103,359 µs | 266,751 µs | 183 ms | 167.4 MiB | 0 |
| flymcprust (default limits) | 64 | 2,989 | 0% | 21,391 µs | 22,463 µs | 56 ms | 17.5 MiB | 0 |
| flymcprust (default limits) | 256 | 2,998 | 0% | 85,375 µs | 87,999 µs | 69 ms | 18.6 MiB | 0 |
| flymcprust (default limits) | 1,024 | 819 | 7% | 257,407 µs | 632,831 µs | 1,205 ms | 32.5 MiB | 314473 |

## `mix`

50% trivial calls, 20% orders, 20% upstream calls, 10% `tools/list` of 200 tools.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 14,313 | 0% | 268 µs | 22,175 µs | 55 ms | 67.3 MiB | 0 |
| flymcprust | 1,024 | 37,362 | 2% | 10,495 µs | 121,215 µs | 51 ms | 72.9 MiB | 0 |
| rmcp | 64 | 9,369 | 4% | 2,473 µs | 26,927 µs | 206 ms | 176.8 MiB | 0 |
| rmcp | 1,024 | 8,832 | 1% | 122,111 µs | 413,439 µs | 221 ms | 186.3 MiB | 0 |
| flymcprust + JWT | 64 | 14,213 | 0% | 354 µs | 22,015 µs | 87 ms | 69.7 MiB | 0 |
| flymcprust + JWT | 1,024 | 22,631 | 2% | 50,527 µs | 135,935 µs | 88 ms | 76.5 MiB | 0 |
| rmcp + JWT | 64 | 8,412 | 1% | 3,163 µs | 28,607 µs | 230 ms | 161.6 MiB | 0 |
| rmcp + JWT | 1,024 | 7,554 | 0% | 128,575 µs | 459,007 µs | 259 ms | 175.6 MiB | 0 |
| flymcprust (default limits) | 64 | 14,358 | 0% | 258 µs | 22,143 µs | 52 ms | 27.7 MiB | 0 |
| flymcprust (default limits) | 1,024 | 4,413 | 5% | 45,599 µs | 498,431 µs | 256 ms | 34.8 MiB | 315045 |

## `mix_without_upstream`

The same mix with the upstream calls replaced by trivial calls.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 46,844 | 2% | 1,363 µs | 2,547 µs | 40 ms | 67.3 MiB | 0 |
| flymcprust | 1,024 | 43,955 | 4% | 11,735 µs | 67,839 µs | 43 ms | 72.9 MiB | 0 |
| rmcp | 64 | 11,170 | 2% | 5,807 µs | 9,703 µs | 176 ms | 180.0 MiB | 0 |
| rmcp | 1,024 | 9,059 | 4% | 131,839 µs | 326,399 µs | 216 ms | 192.1 MiB | 0 |
| flymcprust + JWT | 64 | 26,964 | 1% | 2,381 µs | 4,077 µs | 74 ms | 71.0 MiB | 0 |
| flymcprust + JWT | 1,024 | 25,832 | 2% | 22,175 µs | 85,951 µs | 77 ms | 76.5 MiB | 0 |
| rmcp + JWT | 64 | 9,476 | 1% | 6,859 µs | 11,295 µs | 207 ms | 171.3 MiB | 0 |
| rmcp + JWT | 1,024 | 7,861 | 2% | 140,159 µs | 361,727 µs | 249 ms | 184.1 MiB | 0 |
| flymcprust (default limits) | 64 | 48,368 | 1% | 1,318 µs | 2,513 µs | 40 ms | 33.4 MiB | 0 |
| flymcprust (default limits) | 1,024 | 48,236 | 13% | 10,311 µs | 68,351 µs | 41 ms | 33.1 MiB | 0 |

## FlyMCPRust relative to rmcp

Throughput: how many times more requests per second. CPU: how many times less CPU time per request. Memory: how many times less peak memory.

| Scenario | Clients | Throughput | CPU | Memory | Throughput, both with JWT | CPU, both with JWT |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `tools_list_200` | 64 | 8.12× | 8.20× | 3.09× | 3.95× | 3.94× |
| `call_last_of_200` | 64 | 4.00× | 4.03× | 2.89× | 2.10× | 2.10× |
| `call_order_complex` | 64 | 3.64× | 3.67× | 3.00× | 3.31× | 3.27× |
| `call_add_many_clients` | 1,024 | 5.55× | 5.53× | 7.20× | 2.61× | 2.59× |
| `call_add_many_clients` | 4,096 | 5.80× | 5.71× | 7.54× | 2.62× | 2.58× |
| `call_add_no_keepalive` | 64 | 2.06× | 2.24× | 7.78× | 2.31× | 2.37× |
| `call_fetch_upstream` | 64 | 1.02× | 2.41× | 0.52× | 1.03× | 1.83× |
| `call_fetch_upstream` | 256 | 1.05× | 2.59× | 1.16× | 1.06× | 1.88× |
| `call_fetch_upstream` | 1,024 | 2.96× | 2.94× | 2.54× | 2.08× | 2.06× |
| `mix` | 64 | 1.53× | 3.75× | 2.63× | 1.69× | 2.64× |
| `mix` | 1,024 | 4.23× | 4.31× | 2.56× | 3.00× | 2.94× |
| `mix_without_upstream` | 64 | 4.19× | 4.35× | 2.67× | 2.85× | 2.79× |
| `mix_without_upstream` | 1,024 | 4.85× | 4.97× | 2.63× | 3.29× | 3.22× |

Errors across all runs: 629553.
