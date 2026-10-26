# Benchmark results

Date: 2026-10-25. Machine: 12th Gen Intel(R) Core(TM) i5-12450HX, 12 logical cores, 16 GiB, Windows 11 Home Single Language. Each point: 2 s warm-up, 8 s measured. The load generator runs on the same machine.

Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 (Python 3.14, uvicorn 0.54.0). All servers speak MCP 2026-07-28 over Streamable HTTP/1.1 with JSON responses, bound to 127.0.0.1, with release builds for Rust.

## Startup, memory and size

| Server | Startup to first response | Idle memory (RSS) | Artifact |
| --- | ---: | ---: | ---: |
| flymcprust | 42 ms | 5.9 MiB | 1.9 MiB (bench-flymcprust.exe) |
| rmcp | 43 ms | 6.3 MiB | 3.4 MiB (bench-rmcp.exe) |
| flymcprust (1 thread) | 51 ms | 5.7 MiB | 1.9 MiB (bench-flymcprust.exe) |
| rmcp (1 thread) | 48 ms | 6.0 MiB | 3.4 MiB (bench-rmcp.exe) |
| python mcp (1 process) | 1,697 ms | 78.0 MiB | 39.8 MiB (.venv) |

## `tools_list`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 13,433 | 74,722 | 79,051 | 60,837 | 64 µs | 2,447 µs | 52 ms | 8.8 MiB |
| rmcp | 10,862 | 56,883 | 42,164 | 48,574 | 78 µs | 3,681 µs | 116 ms | 16.3 MiB |
| flymcprust (1 thread) | 22,190 | 39,480 | 38,607 | 35,528 | 37 µs | 5,255 µs | 24 ms | 6.8 MiB |
| rmcp (1 thread) | 15,312 | 24,521 | 21,641 | 19,858 | 60 µs | 5,067 µs | 45 ms | 12.6 MiB |
| python mcp (1 process) | 1,812 | 2,052 | 2,024 | 2,036 | 531 µs | 52,287 µs | 479 ms | 82.8 MiB |

## `call_add`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 12,645 | 58,850 | 59,920 | 62,591 | 64 µs | 2,431 µs | 59 ms | 9.9 MiB |
| rmcp | 10,049 | 40,654 | 37,156 | 42,486 | 89 µs | 3,505 µs | 151 ms | 17.4 MiB |
| flymcprust (1 thread) | 19,464 | 36,328 | 35,563 | 33,520 | 40 µs | 5,671 µs | 26 ms | 7.4 MiB |
| rmcp (1 thread) | 13,285 | 19,729 | 17,979 | 16,806 | 70 µs | 7,011 µs | 54 ms | 12.2 MiB |
| python mcp (1 process) | 1,467 | 1,854 | 1,816 | 1,618 | 645 µs | 60,671 µs | 584 ms | 86.3 MiB |

## `call_stats_100`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 17,427 | 57,722 | 58,258 | 59,332 | 46 µs | 2,405 µs | 72 ms | 9.8 MiB |
| rmcp | 7,985 | 38,868 | 35,098 | 38,724 | 107 µs | 3,581 µs | 164 ms | 17.9 MiB |
| flymcprust (1 thread) | 17,514 | 32,696 | 33,860 | 31,379 | 46 µs | 5,383 µs | 28 ms | 7.1 MiB |
| rmcp (1 thread) | 11,200 | 16,020 | 14,906 | 13,472 | 83 µs | 6,659 µs | 65 ms | 14.3 MiB |
| python mcp (1 process) | 1,382 | 1,766 | 1,702 | 1,466 | 681 µs | 64,383 µs | 624 ms | 95.7 MiB |

## `call_echo_64KiB`

| Server | req/s c=1 | req/s c=16 | req/s c=64 | req/s c=256 | p50 c=1 | p99 c=64 | CPU per 1k req c=64 | Peak RSS c=64 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| flymcprust | 6,167 | 20,642 | 20,442 | 21,045 | 135 µs | 6,451 µs | 295 ms | 22.7 MiB |
| rmcp | 4,891 | 22,889 | 19,983 | 17,027 | 167 µs | 6,371 µs | 326 ms | 32.1 MiB |
| flymcprust (1 thread) | 6,847 | 9,044 | 9,096 | 8,540 | 130 µs | 19,087 µs | 108 ms | 16.1 MiB |
| rmcp (1 thread) | 6,995 | 8,854 | 7,272 | 6,879 | 130 µs | 13,487 µs | 133 ms | 29.9 MiB |
| python mcp (1 process) | 940 | 951 | 920 | 890 | 1,005 µs | 91,519 µs | 1,111 ms | 109.7 MiB |

## FlyMCPRust relative to the others (c=64)

Throughput: how many times more requests per second. CPU: how many times less CPU time per request.

| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |
| --- | ---: | ---: | ---: | ---: |
| `tools_list` | 1.9× | 2.3× | 39.1× | 9.3× |
| `call_add` | 1.6× | 2.6× | 33.0× | 9.8× |
| `call_stats_100` | 1.7× | 2.3× | 34.2× | 8.7× |
| `call_echo_64KiB` | 1.0× | 1.1× | 22.2× | 3.8× |

Errors across all runs: 0.
