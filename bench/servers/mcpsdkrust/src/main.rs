//! Benchmark server: MCPSDKRust. Usage: `bench-mcpsdkrust <port>`.
use mcpsdkrust::prelude::*;
use serde::Serialize;

/// Adds two numbers.
#[tool]
fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// Returns the text unchanged.
#[tool]
fn echo(text: String) -> String {
    text
}

/// Summary statistics.
#[derive(Serialize, schemars::JsonSchema)]
struct Stats {
    count: usize,
    sum: f64,
    mean: f64,
    min: f64,
    max: f64,
}

/// Computes count, sum, mean, minimum and maximum.
#[tool]
fn stats(values: Vec<f64>) -> Json<Stats> {
    let sum: f64 = values.iter().sum();
    let count = values.len();
    Json(Stats {
        count,
        sum,
        mean: if count == 0 { 0.0 } else { sum / count as f64 },
        min: values.iter().copied().fold(f64::INFINITY, f64::min),
        max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args().nth(1).unwrap_or_else(|| "8001".to_owned());
    Server::builder("bench", "1.0.0")
        .tool::<add>()
        .tool::<echo>()
        .tool::<stats>()
        // The load generator sends up to 256 concurrent requests.
        .max_concurrent_calls(1024)
        .build()?
        .serve_http(format!("127.0.0.1:{port}"))
        .await?;
    Ok(())
}
