use sdkmcprust::tool;

#[tool(readonly)]
fn ping() -> &'static str {
    "pong"
}

fn main() {}
