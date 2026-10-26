"""Turns the real-world suite's results.json into RESULTS.md.

Usage: python bench/report_real.py [results.json] [RESULTS.md]

Every point is the median of its repetitions; "spread" is
(max - min) / median of the throughput across repetitions.
"""

import json
import statistics
import sys
from pathlib import Path

src = Path(sys.argv[1] if len(sys.argv) > 1 else "bench/results/real/results.json")
dst = Path(sys.argv[2] if len(sys.argv) > 2 else src.with_name("RESULTS.md"))
data = json.loads(src.read_text(encoding="utf-8"))

FIELDS = ["rps", "p50_us", "p99_us", "cpu_ms_per_1k", "rss_peak_mib"]
DESCRIPTIONS = {
    "tools_list_200": "`tools/list` of 200 tools (~100 KB response).",
    "call_last_of_200": "Calling the last of 200 tools.",
    "call_order_complex": "A nested order (~10 KB, 40 lines, enums) deserialized and summarized.",
    "call_add_many_clients": "A trivial tool, with 1,024 and 4,096 keep-alive connections.",
    "call_add_no_keepalive": "A trivial tool, with a new TCP connection for every request.",
    "call_fetch_upstream": f"Each call waits for an HTTP upstream that answers after {data.get('upstream_ms', 20)} ms.",
    "mix": "50% trivial calls, 20% orders, 20% upstream calls, 10% `tools/list` of 200 tools.",
    "mix_without_upstream": "The same mix with the upstream calls replaced by trivial calls.",
}

runs = {}
for r in data["results"]:
    runs.setdefault((r["server"], r["scenario"], r["concurrency"]), []).append(r)


def median(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


point = {}
for k, rs in runs.items():
    p = {f: median([r[f] for r in rs]) for f in FIELDS}
    rps = [r["rps"] for r in rs]
    p["spread"] = (max(rps) - min(rps)) / p["rps"] if len(rps) > 1 and p["rps"] else None
    p["errors"] = sum(r["errors"] for r in rs)
    point[k] = p


def num(value, fmt="{:,.0f}"):
    return "–" if value is None else fmt.format(value)


def ratio(a, b, invert=False):
    if not a or not b:
        return "–"
    return f"{(b / a if invert else a / b):.2f}×"


servers = [s["server"] for s in data["servers"]]
scenarios = list(dict.fromkeys(k[1] for k in point))
m = data["machine"]
isolation = (
    f"the server is pinned to CPUs {m['server_cpus']} and the load generator (and the simulated upstream) to the others"
    if m.get("server_cpus")
    else "the load generator runs on the same CPUs as the server"
)
lines = [
    "# Real-world benchmark results",
    "",
    f"Date: {data.get('date') or 'unknown'}"
    + (f", commit `{data['commit'][:7]}`" if data.get("commit") else "")
    + f". Machine: {m['cpu']}, {m['logical_cores']} logical cores ({m.get('arch', '?')}), "
    f"{m['memory_gib']:.0f} GiB, {m['os']}; {isolation}. Each point: "
    f"{data['warmup_s']:.0f} s warm-up, {data['duration_s']:.0f} s measured"
    + (f", median of {data['repeat']} runs" if data.get("repeat", 1) > 1 else "")
    + ".",
    "",
    "FlyMCPRust and rmcp 3.7.0 serve the same 203 tools over Streamable HTTP/1.1 "
    "(MCP 2026-07-28, stateless, JSON responses). `+ JWT` targets require an RS256 "
    "access token on every request, checked by the same verifier in both servers. "
    "`default limits` is FlyMCPRust without raising its secure defaults (64 concurrent "
    "tool calls, 256 requests in flight, 1,024 connections).",
    "",
    "## Startup and idle memory (203 tools)",
    "",
    "| Server | Startup to first response | Idle memory (RSS) |",
    "| --- | ---: | ---: |",
]
for s in data["servers"]:
    lines.append(
        f"| {s['server']} | {num(s['startup_ms'], '{:,.0f} ms')} | {num(s['idle_rss_mib'], '{:.1f} MiB')} |"
    )

for scenario in scenarios:
    levels = sorted({k[2] for k in point if k[1] == scenario})
    lines += ["", f"## `{scenario}`", "", DESCRIPTIONS.get(scenario, ""), ""]
    lines.append(
        "| Server | Clients | req/s | spread | p50 | p99 | CPU per 1k req | Peak RSS | Errors |"
    )
    lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for server in servers:
        for c in levels:
            p = point.get((server, scenario, c))
            if not p:
                continue
            lines.append(
                f"| {server} | {c:,} | {num(p['rps'])} | {num(p['spread'], '{:.0%}')} | "
                f"{num(p['p50_us'], '{:,.0f} µs')} | {num(p['p99_us'], '{:,.0f} µs')} | "
                f"{num(p['cpu_ms_per_1k'], '{:,.0f} ms')} | {num(p['rss_peak_mib'], '{:.1f} MiB')} | {p['errors']} |"
            )

lines += [
    "",
    "## FlyMCPRust relative to rmcp",
    "",
    "Throughput: how many times more requests per second. CPU: how many times less CPU "
    "time per request. Memory: how many times less peak memory.",
    "",
    "| Scenario | Clients | Throughput | CPU | Memory | Throughput, both with JWT | CPU, both with JWT |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
]
for scenario in scenarios:
    for c in sorted({k[2] for k in point if k[1] == scenario}):
        f = point.get(("flymcprust", scenario, c)) or {}
        r = point.get(("rmcp", scenario, c)) or {}
        fa = point.get(("flymcprust + JWT", scenario, c)) or {}
        ra = point.get(("rmcp + JWT", scenario, c)) or {}
        lines.append(
            f"| `{scenario}` | {c:,} | {ratio(f.get('rps'), r.get('rps'))} | "
            f"{ratio(f.get('cpu_ms_per_1k'), r.get('cpu_ms_per_1k'), invert=True)} | "
            f"{ratio(f.get('rss_peak_mib'), r.get('rss_peak_mib'), invert=True)} | "
            f"{ratio(fa.get('rps'), ra.get('rps'))} | "
            f"{ratio(fa.get('cpu_ms_per_1k'), ra.get('cpu_ms_per_1k'), invert=True)} |"
        )

errors = sum(p["errors"] for p in point.values())
lines += ["", f"Errors across all runs: {errors}.", ""]
dst.write_text("\n".join(lines), encoding="utf-8")
print(f"wrote {dst}")
