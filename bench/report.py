"""Turns bench/results/results.json into bench/results/RESULTS.md.

Usage: python bench/report.py [results.json] [RESULTS.md]

Every point is the median of its repetitions; "spread" is
(max - min) / median of the throughput across repetitions.
"""

import json
import statistics
import sys
from pathlib import Path

src = Path(sys.argv[1] if len(sys.argv) > 1 else "bench/results/results.json")
dst = Path(sys.argv[2] if len(sys.argv) > 2 else src.with_name("RESULTS.md"))
data = json.loads(src.read_text(encoding="utf-8"))

FLY = "flymcprust"
RMCP = "rmcp"
PY = "python mcp (1 process)"
OAUTH = "flymcprust + OAuth (JWT RS256)"
FIELDS = ["rps", "p50_us", "p99_us", "cpu_ms_per_1k", "rss_peak_mib"]

runs = {}
for r in data["results"]:
    k = (r.get("transport", "http"), r["server"], r["scenario"], r["concurrency"])
    runs.setdefault(k, []).append(r)


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
    return f"{(b / a if invert else a / b):.1f}×"


m = data["machine"]
repeat = data.get("repeat", 1)
isolation = (
    f"the server is pinned to CPUs {m['server_cpus']} and the load generator to the others"
    if m.get("server_cpus")
    else "the load generator runs on the same CPUs as the server"
)
lines = [
    "# Benchmark results",
    "",
    f"Date: {data.get('date') or 'unknown'}"
    + (f", commit `{data['commit'][:7]}`" if data.get("commit") else "")
    + f". Machine: {m['cpu']}, {m['logical_cores']} logical cores ({m.get('arch', '?')}), "
    f"{m['memory_gib']:.0f} GiB, {m['os']}; {isolation}. Each point: "
    f"{data['warmup_s']:.0f} s warm-up, {data['duration_s']:.0f} s measured"
    + (f", median of {repeat} runs" if repeat > 1 else "")
    + ".",
    "",
    f"Versions: FlyMCPRust (this repository), rmcp 3.7.0, mcp 2.3.0 "
    f"({data.get('python') or 'Python'}, uvicorn 0.54.0). All servers speak MCP "
    "2026-07-28, stateless; over HTTP/1.1 with JSON responses bound to 127.0.0.1; "
    "release builds for Rust.",
    "",
    "## Startup, memory and size",
    "",
    "| Server | Transport | Startup to first response | Idle memory (RSS) | Artifact |",
    "| --- | --- | ---: | ---: | ---: |",
]
for s in data["servers"]:
    lines.append(
        f"| {s['server']} | {s.get('transport', 'http')} | "
        + (num(s["startup_ms"], "{:,.0f} ms") if s["startup_ms"] is not None else "did not answer")
        + f" | {num(s['idle_rss_mib'], '{:.1f} MiB')} | "
        f"{num(s['artifact_mib'], '{:.1f} MiB')} ({s['artifact']}) |"
    )


def section(transport, title):
    keys = [k for k in point if k[0] == transport]
    if not keys:
        return
    servers = list(dict.fromkeys(k[1] for k in keys))
    scenarios = list(dict.fromkeys(k[2] for k in keys))
    levels = sorted({k[3] for k in keys})
    mid = 64 if 64 in levels else levels[-1]
    lines.extend(["", f"## {title}"])
    for scenario in scenarios:
        lines.extend(
            [
                "",
                f"### `{scenario}`",
                "",
                "| Server | "
                + " | ".join(f"req/s c={c}" for c in levels)
                + f" | spread c={mid} | p50 c=1 | p99 c={mid} | CPU per 1k req c={mid} | Peak RSS c={mid} |",
                "| --- | " + " | ".join("---:" for _ in levels) + " | ---: | ---: | ---: | ---: | ---: |",
            ]
        )
        for server in servers:
            row = [point.get((transport, server, scenario, c)) for c in levels]
            if not any(row):
                continue
            one = point.get((transport, server, scenario, 1))
            at = point.get((transport, server, scenario, mid))
            lines.append(
                f"| {server} | "
                + " | ".join(num(r and r["rps"]) for r in row)
                + f" | {num(at and at['spread'], '{:.0%}')}"
                + f" | {num(one and one['p50_us'], '{:,.0f} µs')}"
                + f" | {num(at and at['p99_us'], '{:,.0f} µs')}"
                + f" | {num(at and at['cpu_ms_per_1k'], '{:,.0f} ms')}"
                + f" | {num(at and at['rss_peak_mib'], '{:.1f} MiB')} |"
            )
    return scenarios, mid


http = section("http", "Streamable HTTP")
stdio = section("stdio", "stdio")


def comparison(transport, scenarios, mid, title):
    lines.extend(
        [
            "",
            f"## {title} (c={mid})",
            "",
            "Throughput: how many times more requests per second. CPU: how many times less CPU time per request.",
            "",
            "| Scenario | Throughput vs rmcp | CPU vs rmcp | Throughput vs Python | CPU vs Python |",
            "| --- | ---: | ---: | ---: | ---: |",
        ]
    )
    for scenario in scenarios:
        f = point.get((transport, FLY, scenario, mid)) or {}
        r = point.get((transport, RMCP, scenario, mid)) or {}
        p = point.get((transport, PY, scenario, mid)) or {}
        lines.append(
            f"| `{scenario}` | {ratio(f.get('rps'), r.get('rps'))} | "
            f"{ratio(f.get('cpu_ms_per_1k'), r.get('cpu_ms_per_1k'), invert=True)} | "
            f"{ratio(f.get('rps'), p.get('rps'))} | "
            f"{ratio(f.get('cpu_ms_per_1k'), p.get('cpu_ms_per_1k'), invert=True)} |"
        )


if http:
    comparison("http", *http, "FlyMCPRust relative to the others over HTTP")
    scenarios, mid = http
    if any(k[1] == OAUTH for k in point):
        lines.extend(
            [
                "",
                f"## Cost of OAuth (c={mid})",
                "",
                "The same FlyMCPRust server with every request carrying an RS256 JWT access "
                "token, verified (signature, issuer, audience, expiry, scopes) on each request.",
                "",
                "| Scenario | req/s without | req/s with | CPU per 1k req without | CPU per 1k req with |",
                "| --- | ---: | ---: | ---: | ---: |",
            ]
        )
        for scenario in scenarios:
            a = point.get(("http", FLY, scenario, mid)) or {}
            b = point.get(("http", OAUTH, scenario, mid)) or {}
            lines.append(
                f"| `{scenario}` | {num(a.get('rps'))} | {num(b.get('rps'))} | "
                f"{num(a.get('cpu_ms_per_1k'), '{:,.0f} ms')} | {num(b.get('cpu_ms_per_1k'), '{:,.0f} ms')} |"
            )
if stdio:
    comparison("stdio", *stdio, "FlyMCPRust relative to the others over stdio")

errors = sum(p["errors"] for p in point.values())
lines += ["", f"Errors across all runs: {errors}.", ""]
dst.write_text("\n".join(lines), encoding="utf-8")
print(f"wrote {dst}")
