"""Benchmark server: official MCP Python SDK (MCPServer API).

Usage: python server.py <port>

Configured like the other servers: 2026-07-28 stateless, JSON responses
(json_response=True), served by uvicorn on 127.0.0.1, one process.
"""

import sys

import uvicorn
from pydantic import BaseModel

from mcp.server import MCPServer

mcp = MCPServer("bench")


@mcp.tool()
def add(a: float, b: float) -> str:
    """Adds two numbers."""
    return str(a + b)


@mcp.tool()
def echo(text: str) -> str:
    """Returns the text unchanged."""
    return text


class Stats(BaseModel):
    count: int
    sum: float
    mean: float
    min: float
    max: float


@mcp.tool()
def stats(values: list[float]) -> Stats:
    """Computes count, sum, mean, minimum and maximum."""
    total = sum(values)
    count = len(values)
    return Stats(
        count=count,
        sum=total,
        mean=total / count if count else 0.0,
        min=min(values) if values else float("inf"),
        max=max(values) if values else float("-inf"),
    )


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8003
    app = mcp.streamable_http_app(json_response=True)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning", access_log=False)
