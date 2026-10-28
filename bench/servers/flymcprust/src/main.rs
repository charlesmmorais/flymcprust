//! Benchmark server: FlyMCPRust.
//!
//! ```text
//! bench-flymcprust <port> [real] [defaults] [oauth <jwks>]   Streamable HTTP
//! bench-flymcprust stdio                                      stdio
//! ```
//!
//! `real` adds the real-world tools (200 generated tools, a complex payload,
//! an upstream call); `oauth` requires an RS256 JWT on every request;
//! `defaults` keeps the SDK's default limits instead of raising them (in
//! 0.1.0: 64 concurrent tool calls, 256 requests in flight, 1024
//! connections; since 0.1.1: 1024, 4096 and 4096, with request bodies
//! limited to 64 MiB in total).
mod many;
mod real;
mod upstream;

use flymcprust::auth::JwtVerifier;
use flymcprust::prelude::*;
use flymcprust::transport::http::{HttpConfig, OAuth, StreamableHttpService, serve};
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

/// Issuer and audience of the token in `bench/fixtures/token.txt`.
const ISSUER: &str = "http://127.0.0.1:9000";
const AUDIENCE: &str = "http://127.0.0.1/mcp";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let mut builder = Server::builder("bench", "1.0.0").tool::<add>();
    builder = if flag("real") {
        many::register(builder.tool::<real::order>().tool::<real::fetch>())
    } else {
        builder.tool::<echo>().tool::<stats>()
    };
    if !flag("defaults") {
        // The load generator sends up to 4096 concurrent requests.
        builder = builder.max_concurrent_calls(8192);
    }
    let server = builder.build()?;
    let port = args.first().map_or("8001", String::as_str);
    if port == "stdio" {
        server.serve_stdio().await?;
        return Ok(());
    }
    let mut config = if flag("defaults") {
        HttpConfig::new()
    } else {
        HttpConfig::new().max_in_flight(8192).max_connections(8192)
    };
    if let Some(i) = args.iter().position(|a| a == "oauth") {
        let jwks = std::fs::read_to_string(args.get(i + 1).ok_or("missing JWKS path")?)?;
        let verifier = JwtVerifier::builder(ISSUER, AUDIENCE).jwks(jwks).build()?;
        let oauth = OAuth::builder(AUDIENCE, verifier)
            .authorization_server(ISSUER)
            .required_scopes(["mcp"])
            .build()?;
        config = config.oauth(oauth);
    }
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    serve(
        listener,
        StreamableHttpService::with_config(server, config),
        "/mcp",
        std::future::pending(),
    )
    .await?;
    Ok(())
}
