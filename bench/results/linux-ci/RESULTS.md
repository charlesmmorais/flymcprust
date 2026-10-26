# Benchmark results

Date: 2026-10-25. Machine: Intel(R) Xeon(R) 6973P-C, 4 logical cores, 16 GiB, Linux (Ubuntu 24.04). Each point: 2 s warm-up, 8 s measured. The load generator runs on the same machine.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.12.3, uvicorn 0.54.0). All servers speak MCP 2026-07-28 over Streamable HTTP/1.1 with JSON responses, bound to 127.0.0.1, with release builds for Rust.

## Startup, memory and size

| Server | Startup to first response | Idle memory (RSS) | Artifact |
| --- | ---: | ---: | ---: |
| flymcprust | 4 ms | 4.2 MiB | 2.3 MiB (bench-flymcprust) |
| rmcp | 5 ms | 5.1 MiB | 3.7 MiB (bench-rmcp) |
| flymcprust (1 thread) | 4 ms | 4.1 MiB | 2.3 MiB (bench-flymcprust) |
| rmcp (1 thread) | 5 ms | 4.8 MiB | 3.7 MiB (bench-rmcp) |
| python mcp (1 process) | 869 ms | 72.8 MiB | 90.3 MiB (.venv) |

## `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 19,421 | 113,937 | 130,627 | 147,622 | 49 µs | 1,059 µs | 13 ms | 6.1 MiB |
| rmcp | 11,692 | 48,009 | 53,472 | 56,142 | 82 µs | 2,525 µs | 56 ms | 15.5 MiB |
| flymcprust (1 thread) | 23,095 | 103,482 | 101,980 | 97,252 | 40 µs | 1,130 µs | 9 ms | 5.0 MiB |
| rmcp (1 thread) | 15,445 | 28,485 | 25,230 | 23,475 | 67 µs | 3,179 µs | 40 ms | 10.0 MiB |
| python mcp (1 process) | 2,608 | 2,758 | 2,720 | 2,689 | 370 µs | 49,087 µs | 368 ms | 72.8 MiB |

## `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 18,654 | 100,556 | 126,030 | 135,754 | 52 µs | 1,167 µs | 16 ms | 9.9 MiB |
| rmcp | 10,589 | 43,719 | 48,792 | 50,550 | 92 µs | 2,745 µs | 62 ms | 35.7 MiB |
| flymcprust (1 thread) | 21,080 | 89,706 | 88,745 | 82,350 | 42 µs | 950 µs | 11 ms | 7.3 MiB |
| rmcp (1 thread) | 14,018 | 25,114 | 22,976 | 21,066 | 73 µs | 3,917 µs | 43 ms | 18.9 MiB |
| python mcp (1 process) | 1,801 | 1,994 | 1,958 | 1,694 | 548 µs | 65,471 µs | 533 ms | 74.5 MiB |

## `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 16,290 | 81,790 | 90,975 | 94,404 | 61 µs | 1,648 µs | 29 ms | 10.6 MiB |
| rmcp | 9,156 | 34,917 | 35,090 | 35,006 | 106 µs | 3,693 µs | 91 ms | 29.2 MiB |
| flymcprust (1 thread) | 18,543 | 43,573 | 45,420 | 47,896 | 56 µs | 1,822 µs | 22 ms | 7.9 MiB |
| rmcp (1 thread) | 11,743 | 18,718 | 17,366 | 16,386 | 86 µs | 4,399 µs | 58 ms | 23.8 MiB |
| python mcp (1 process) | 1,621 | 1,867 | 1,821 | 1,618 | 604 µs | 67,007 µs | 568 ms | 81.2 MiB |

## `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 7,087 | 21,007 | 20,866 | 20,958 | 134 µs | 6,531 µs | 141 ms | 23.3 MiB |
| rmcp | 5,510 | 18,750 | 18,991 | 17,432 | 180 µs | 6,667 µs | 162 ms | 37.3 MiB |
| flymcprust (1 thread) | 8,392 | 11,210 | 11,178 | 10,709 | 122 µs | 6,799 µs | 89 ms | 15.7 MiB |
| rmcp (1 thread) | 8,060 | 10,455 | 9,970 | 8,608 | 126 µs | 8,311 µs | 100 ms | 29.2 MiB |
| python mcp (1 process) | 1,036 | 1,118 | 1,128 | 1,007 | 935 µs | 93,439 µs | 908 ms | 96.5 MiB |

## FlyMCPRust relative to the others (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 2.4× | 4.1× | 48.0× | 27.5× |
| `call_add` | 2.6× | 3.9× | 64.4× | 33.4× |
| `call_stats_100` | 2.6× | 3.2× | 50.0× | 19.8× |
| `call_echo_64KiB` | 1.1× | 1.2× | 18.5× | 6.5× |

Errors across all runs: 0.
