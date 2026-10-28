# Real-world benchmark results

Date: 2026-10-26, commit `47f1c60`. Machine: AMD EPYC 9V74 80-Core Processor, 4 logical cores (x86_64), 16 GiB, Linux (Ubuntu 24.04); the server is pinned to CPUs 0-1 and the load generator (and the simulated upstream) to the others. Each point: 2 s warm-up, 8 s measured, median of 3 runs.

FlyMCPRust and rmcp 3.7.0 serve the same 203 tools over Streamable HTTP/1.1 (MCP 2026-07-28, stateless, JSON responses). `+ JWT` targets require an RS256 access token on every request, checked by the same verifier in both servers. `default limits` is FlyMCPRust without raising its secure defaults (64 concurrent tool calls, 256 requests in flight, 1,024 connections).

## Startup and idle memory (203 tools)

| Server | Startup to first response | Idle memory (RSS) |
| --- | ---: | ---: |
| flymcprust | 7 ms | 8.3 MiB |
| rmcp | 8 ms | 9.6 MiB |
| flymcprust + JWT | 7 ms | 8.8 MiB |
| rmcp + JWT | 7 ms | 10.0 MiB |
| flymcprust (default limits) | 7 ms | 8.4 MiB |

## `tools_list_200`

`tools/list` of 200 tools (~100 KB response).

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 29,670 | 17% | 2,191 µs | 3,983 µs | 67 ms | 9.5 MiB | 0 |
| rmcp | 64 | 4,916 | 3% | 13,415 µs | 21,071 µs | 406 ms | 25.7 MiB | 0 |
| flymcprust + JWT | 64 | 14,317 | 1% | 4,647 µs | 7,035 µs | 140 ms | 10.0 MiB | 0 |
| rmcp + JWT | 64 | 4,382 | 0% | 15,039 µs | 22,911 µs | 455 ms | 26.2 MiB | 0 |
| flymcprust (default limits) | 64 | 28,833 | 4% | 2,263 µs | 3,827 µs | 69 ms | 9.5 MiB | 0 |

## `call_last_of_200`

Calling the last of 200 tools.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 64,372 | 2% | 1,014 µs | 1,692 µs | 31 ms | 9.7 MiB | 0 |
| rmcp | 64 | 25,722 | 1% | 2,547 µs | 3,995 µs | 77 ms | 24.8 MiB | 0 |
| flymcprust + JWT | 64 | 19,452 | 0% | 3,469 µs | 4,983 µs | 103 ms | 9.7 MiB | 0 |
| rmcp + JWT | 64 | 15,749 | 1% | 4,111 µs | 6,807 µs | 127 ms | 25.3 MiB | 0 |
| flymcprust (default limits) | 64 | 63,173 | 1% | 1,029 µs | 1,742 µs | 31 ms | 9.5 MiB | 0 |

## `call_order_complex`

A nested order (~10 KB, 40 lines, enums) deserialized and summarized.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 17,521 | 0% | 3,861 µs | 5,543 µs | 114 ms | 9.8 MiB | 0 |
| rmcp | 64 | 5,209 | 1% | 12,495 µs | 20,799 µs | 381 ms | 26.2 MiB | 0 |
| flymcprust + JWT | 64 | 10,887 | 0% | 6,219 µs | 8,855 µs | 183 ms | 10.0 MiB | 0 |
| rmcp + JWT | 64 | 4,572 | 1% | 14,183 µs | 23,775 µs | 435 ms | 26.7 MiB | 0 |
| flymcprust (default limits) | 64 | 17,568 | 0% | 3,775 µs | 5,539 µs | 114 ms | 9.7 MiB | 0 |

## `call_add_many_clients`

A trivial tool, with 1,024 and 4,096 keep-alive connections.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 1,024 | 64,236 | 2% | 7,159 µs | 75,519 µs | 31 ms | 25.4 MiB | 0 |
| flymcprust | 4,096 | 61,980 | 1% | 7,447 µs | 370,175 µs | 32 ms | 75.9 MiB | 0 |
| rmcp | 1,024 | 18,530 | 2% | 12,415 µs | 297,727 µs | 108 ms | 168.7 MiB | 0 |
| rmcp | 4,096 | 17,735 | 2% | 13,087 µs | 1,362,943 µs | 113 ms | 545.6 MiB | 0 |
| flymcprust + JWT | 1,024 | 19,535 | 1% | 44,383 µs | 80,703 µs | 103 ms | 24.5 MiB | 0 |
| flymcprust + JWT | 4,096 | 18,788 | 0% | 42,015 µs | 413,695 µs | 107 ms | 72.3 MiB | 0 |
| rmcp + JWT | 1,024 | 13,426 | 2% | 19,231 µs | 301,823 µs | 149 ms | 163.8 MiB | 0 |
| rmcp + JWT | 4,096 | 13,100 | 3% | 19,919 µs | 1,367,039 µs | 152 ms | 545.6 MiB | 0 |
| flymcprust (default limits) | 1,024 | 63,512 | 1% | 7,235 µs | 73,727 µs | 31 ms | 24.7 MiB | 0 |
| flymcprust (default limits) | 4,096 | 63,464 | 2% | 7,259 µs | 73,727 µs | 32 ms | 25.0 MiB | 18 |

## `call_add_no_keepalive`

A trivial tool, with a new TCP connection for every request.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 20,289 | 0% | 1,600 µs | 3,511 µs | 83 ms | 43.5 MiB | 0 |
| rmcp | 64 | 16,354 | 0% | 3,507 µs | 6,343 µs | 120 ms | 167.0 MiB | 0 |
| flymcprust + JWT | 64 | 15,172 | 0% | 3,839 µs | 8,099 µs | 129 ms | 44.4 MiB | 0 |
| rmcp + JWT | 64 | 11,071 | 4% | 5,715 µs | 9,879 µs | 179 ms | 222.3 MiB | 0 |
| flymcprust (default limits) | 64 | 20,404 | 0% | 1,591 µs | 3,471 µs | 82 ms | 18.5 MiB | 0 |

## `call_fetch_upstream`

Each call waits for an HTTP upstream that answers after 20 ms.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 2,980 | 0% | 21,455 µs | 22,543 µs | 74 ms | 42.4 MiB | 0 |
| flymcprust | 256 | 11,660 | 0% | 21,887 µs | 23,871 µs | 64 ms | 42.8 MiB | 0 |
| flymcprust | 1,024 | 29,943 | 2% | 30,783 µs | 62,847 µs | 67 ms | 70.8 MiB | 0 |
| rmcp | 64 | 2,930 | 1% | 21,775 µs | 23,407 µs | 139 ms | 26.7 MiB | 0 |
| rmcp | 256 | 11,297 | 1% | 22,607 µs | 24,831 µs | 133 ms | 59.4 MiB | 0 |
| rmcp | 1,024 | 13,169 | 2% | 45,247 µs | 217,983 µs | 151 ms | 176.4 MiB | 0 |
| flymcprust + JWT | 64 | 2,978 | 0% | 21,471 µs | 22,575 µs | 141 ms | 43.0 MiB | 0 |
| flymcprust + JWT | 256 | 11,782 | 0% | 21,775 µs | 23,055 µs | 123 ms | 43.3 MiB | 0 |
| flymcprust + JWT | 1,024 | 15,850 | 1% | 60,863 µs | 135,423 µs | 126 ms | 70.9 MiB | 0 |
| rmcp + JWT | 64 | 2,929 | 0% | 21,839 µs | 23,375 µs | 189 ms | 25.4 MiB | 0 |
| rmcp + JWT | 256 | 11,256 | 1% | 22,687 µs | 24,591 µs | 171 ms | 57.0 MiB | 0 |
| rmcp + JWT | 1,024 | 10,334 | 1% | 106,815 µs | 272,639 µs | 193 ms | 173.4 MiB | 0 |
| flymcprust (default limits) | 64 | 2,976 | 0% | 21,423 µs | 22,543 µs | 71 ms | 18.6 MiB | 0 |
| flymcprust (default limits) | 256 | 2,991 | 0% | 85,567 µs | 88,255 µs | 78 ms | 18.6 MiB | 0 |
| flymcprust (default limits) | 1,024 | 839 | 7% | 299,519 µs | 494,591 µs | 1,477 ms | 33.3 MiB | 312037 |

## `mix`

50% trivial calls, 20% orders, 20% upstream calls, 10% `tools/list` of 200 tools.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 14,378 | 0% | 266 µs | 22,063 µs | 61 ms | 65.5 MiB | 0 |
| flymcprust | 1,024 | 34,158 | 1% | 12,039 µs | 126,143 µs | 58 ms | 71.3 MiB | 0 |
| rmcp | 64 | 9,758 | 1% | 2,261 µs | 26,239 µs | 201 ms | 167.7 MiB | 0 |
| rmcp | 1,024 | 8,420 | 3% | 122,559 µs | 423,423 µs | 235 ms | 180.6 MiB | 0 |
| flymcprust + JWT | 64 | 13,939 | 0% | 459 µs | 22,063 µs | 122 ms | 67.2 MiB | 0 |
| flymcprust + JWT | 1,024 | 16,051 | 0% | 71,615 µs | 186,239 µs | 124 ms | 73.7 MiB | 0 |
| rmcp + JWT | 64 | 8,126 | 1% | 3,425 µs | 29,119 µs | 243 ms | 169.0 MiB | 0 |
| rmcp + JWT | 1,024 | 7,440 | 2% | 129,599 µs | 463,359 µs | 266 ms | 183.6 MiB | 0 |
| flymcprust (default limits) | 64 | 14,424 | 0% | 253 µs | 22,047 µs | 60 ms | 30.5 MiB | 0 |
| flymcprust (default limits) | 1,024 | 5,054 | 6% | 65,343 µs | 204,927 µs | 304 ms | 35.7 MiB | 310494 |

## `mix_without_upstream`

The same mix with the upstream calls replaced by trivial calls.

| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 64 | 43,123 | 0% | 1,487 µs | 2,707 µs | 46 ms | 65.7 MiB | 0 |
| flymcprust | 1,024 | 40,323 | 1% | 13,207 µs | 69,375 µs | 50 ms | 71.3 MiB | 0 |
| rmcp | 64 | 10,782 | 1% | 5,963 µs | 10,335 µs | 183 ms | 168.2 MiB | 0 |
| rmcp | 1,024 | 8,916 | 2% | 135,295 µs | 328,959 µs | 221 ms | 190.9 MiB | 0 |
| flymcprust + JWT | 64 | 17,639 | 0% | 3,687 µs | 6,023 µs | 113 ms | 68.2 MiB | 0 |
| flymcprust + JWT | 1,024 | 17,213 | 0% | 39,647 µs | 105,023 µs | 116 ms | 73.7 MiB | 0 |
| rmcp + JWT | 64 | 8,779 | 2% | 7,407 µs | 12,479 µs | 226 ms | 172.7 MiB | 0 |
| rmcp + JWT | 1,024 | 7,421 | 2% | 146,559 µs | 383,999 µs | 266 ms | 196.3 MiB | 0 |
| flymcprust (default limits) | 64 | 42,682 | 0% | 1,500 µs | 2,737 µs | 47 ms | 34.2 MiB | 0 |
| flymcprust (default limits) | 1,024 | 41,383 | 1% | 12,623 µs | 67,903 µs | 48 ms | 34.2 MiB | 0 |

## FlyMCPRust relative to rmcp

Throughput: how many times more requests per second. CPU: how many times less CPU time per request. Memory: how many times less peak memory.

| Scenario | Clients | Throughput | CPU | Memory | Throughput, both with JWT | CPU, both with JWT |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `tools_list_200` | 64 | 6.04× | 6.10× | 2.70× | 3.27× | 3.26× |
| `call_last_of_200` | 64 | 2.50× | 2.52× | 2.57× | 1.24× | 1.23× |
| `call_order_complex` | 64 | 3.36× | 3.35× | 2.67× | 2.38× | 2.37× |
| `call_add_many_clients` | 1,024 | 3.47× | 3.46× | 6.65× | 1.46× | 1.45× |
| `call_add_many_clients` | 4,096 | 3.49× | 3.49× | 7.19× | 1.43× | 1.42× |
| `call_add_no_keepalive` | 64 | 1.24× | 1.45× | 3.84× | 1.37× | 1.39× |
| `call_fetch_upstream` | 64 | 1.02× | 1.87× | 0.63× | 1.02× | 1.34× |
| `call_fetch_upstream` | 256 | 1.03× | 2.06× | 1.39× | 1.05× | 1.40× |
| `call_fetch_upstream` | 1,024 | 2.27× | 2.26× | 2.49× | 1.53× | 1.53× |
| `mix` | 64 | 1.47× | 3.31× | 2.56× | 1.72× | 1.99× |
| `mix` | 1,024 | 4.06× | 4.02× | 2.53× | 2.16× | 2.13× |
| `mix_without_upstream` | 64 | 4.00× | 3.97× | 2.56× | 2.01× | 2.01× |
| `mix_without_upstream` | 1,024 | 4.52× | 4.45× | 2.68× | 2.32× | 2.29× |

Errors across all runs: 622549.
