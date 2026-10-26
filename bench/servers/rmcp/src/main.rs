//! Benchmark server: rmcp (official Rust SDK). Usage: `bench-rmcp <port>`,
//! or `bench-rmcp stdio`.
//!
//! Configured like the other servers: stateless, JSON responses (no SSE),
//! loopback hosts, HTTP/1.1 served by hyper.
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use rmcp::{schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, schemars::JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct EchoArgs {
    text: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct StatsArgs {
    values: Vec<f64>,
}

#[derive(Serialize, schemars::JsonSchema)]
struct Stats {
    count: usize,
    sum: f64,
    mean: f64,
    min: f64,
    max: f64,
}

#[derive(Clone)]
struct Bench;

#[tool_router(server_handler)]
impl Bench {
    #[tool(description = "Adds two numbers.")]
    fn add(&self, Parameters(AddArgs { a, b }): Parameters<AddArgs>) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Returns the text unchanged.")]
    fn echo(&self, Parameters(EchoArgs { text }): Parameters<EchoArgs>) -> String {
        text
    }

    #[tool(description = "Computes count, sum, mean, minimum and maximum.")]
    fn stats(&self, Parameters(StatsArgs { values }): Parameters<StatsArgs>) -> Json<Stats> {
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::service::TowerToHyperService;

    let port = std::env::args().nth(1).unwrap_or_else(|| "8002".to_owned());
    if port == "stdio" {
        use rmcp::ServiceExt as _;
        Bench
            .serve(rmcp::transport::stdio())
            .await?
            .waiting()
            .await?;
        return Ok(());
    }
    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = false;
    config.json_response = true;
    let service = TowerToHyperService::new(StreamableHttpService::new(
        || Ok(Bench),
        NeverSessionManager::default().into(),
        config,
    ));
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let service = service.clone();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}
