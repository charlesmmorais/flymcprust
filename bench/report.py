"""Turns bench/results/results.json into bench/results/RESULTS.md.

Usage: python bench/report.py [results.json] [RESULTS.md]
"""

import json
import sys
from pathlib import Path

src = Path(sys.argv[1] if len(sys.argv) > 1 else "bench/results/results.json")
dst = Path(sys.argv[2] if len(sys.argv) > 2 else src.with_name("RESULTS.md"))
data = json.loads(src.read_text(encoding="utf-8"))

servers = [s["server"] for s in data["servers"]]
scenarios = list(dict.fromkeys(r["scenario"] for r in data["results"]))
levels = sorted({r["concurrency"] for r in data["results"]})
key = {(r["server"], r["scenario"], r["concurrency"]): r for r in data["results"]}
mid = 64 if 64 in levels else levels[-1]


def num(value, fmt="{:,.0f}"):
    return "–" if value is None else fmt.format(value)


lines = []
m = data["machine"]
lines += [
    "# Benchmark results",
    "",
    f"Date: {data.get('date') or 'unknown'}. Machine: {m['cpu']}, {m['logical_cores']} logical "
    f"cores, {m['memory_gib']:.0f} GiB, {m['os']}. Each point: {data['warmup_s']:.0f} s warm-up, "
    f"{data['duration_s']:.0f} s measured. The load generator runs on the same machine.",
    "",
    f"Versions: MCPSDKRust (this repository), rmcp 3.7.0, mcp 2.3.0 ({data.get('python') or 'Python'}, uvicorn 0.54.0). "
    "All servers speak MCP 2026-07-28 over Streamable HTTP/1.1 with JSON responses, bound to "
    "127.0.0.1, with release builds for Rust.",
    "",
    "## Startup, memory and size",
    "",
    "| Server | Startup to first response | Idle memory (RSS) | Artifact |",
    "| --- | ---: | ---: | ---: |",
]
for s in data["servers"]:
    lines.append(
        f"| {s['server']} | {num(s['startup_ms'], '{:,.0f} ms')} | "
        f"{num(s['idle_rss_mib'], '{:.1f} MiB')} | {num(s['artifact_mib'], '{:.1f} MiB')} ({s['artifact']}) |"
    )

for scenario in scenarios:
    lines += [
        "",
        f"## `{scenario}`",
        "",
        "| Server | "
        + " | ".join(f"req/s c={c}" for c in levels)
        + f" | p50 c=1 | p99 c={mid} | CPU per 1k req c={mid} | Peak RSS c={mid} |",
        "| --- | " + " | ".join("---:" for _ in levels) + " | ---: | ---: | ---: | ---: |",
    ]
    for server in servers:
        row = [key.get((server, scenario, c)) for c in levels]
        if not any(row):
            continue
        one = key.get((server, scenario, 1))
        at = key.get((server, scenario, mid))
        lines.append(
            f"| {server} | "
            + " | ".join(num(r and r["rps"]) for r in row)
            + f" | {num(one and one['p50_us'], '{:,} µs')}"
            + f" | {num(at and at['p99_us'], '{:,} µs')}"
            + f" | {num(at and at['cpu_ms_per_1k'], '{:,.0f} ms')}"
            + f" | {num(at and at['rss_peak_mib'], '{:.1f} MiB')} |"
        )


def ratio(a, b, field, invert=False):
    ra, rb = key.get((a, scenario, mid)), key.get((b, scenario, mid))
    if not ra or not rb or not ra[field] or not rb[field]:
        return "–"
    value = rb[field] / ra[field] if invert else ra[field] / rb[field]
    return f"{value:.1f}×"


lines += [
    "",
    f"## MCPSDKRust relative to the others (c={mid})",
    "",
    "Throughput: how many times more requests per second. CPU: how many times less CPU time per request.",
    "",
    "| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |",
    "| --- | ---: | ---: | ---: | ---: |",
]
for scenario in scenarios:
    lines.append(
        f"| `{scenario}` | {ratio('mcpsdkrust', 'rmcp', 'rps')} | "
        f"{ratio('mcpsdkrust', 'rmcp', 'cpu_ms_per_1k', invert=True)} | "
        f"{ratio('mcpsdkrust', 'python mcp (1 process)', 'rps')} | "
        f"{ratio('mcpsdkrust', 'python mcp (1 process)', 'cpu_ms_per_1k', invert=True)} |"
    )

errors = sum(r["errors"] for r in data["results"])
lines += ["", f"Errors across all runs: {errors}.", ""]
dst.write_text("\n".join(lines), encoding="utf-8")
print(f"wrote {dst}")
