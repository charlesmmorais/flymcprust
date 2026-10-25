use sdkmcprust::tool;

#[tool]
fn add(#[cfg(test)] a: i64, b: i64) -> i64 {
    b
}

fn main() {}
