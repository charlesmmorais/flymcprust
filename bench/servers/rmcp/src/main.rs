//! Benchmark server: rmcp (official Rust SDK).
//!
//! ```text
//! bench-rmcp <port> [real] [oauth <jwks>]   Streamable HTTP
//! bench-rmcp stdio                          stdio
//! ```
//!
//! Configured like the other servers: stateless, JSON responses (no SSE),
//! loopback hosts, HTTP/1.1 served by hyper. Tool routers are built once
//! and kept in the handler (the `server_handler` shortcut rebuilds them on
//! every request, which would penalize rmcp unfairly). `real` adds the
//! real-world tools (200 generated tools, a complex payload, an upstream
//! call); `oauth` checks every request's bearer token with the same JWT
//! verifier as the FlyMCPRust server.
mod many;
mod real;
mod upstream;

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use flymcprust::TokenVerifier as _;
use flymcprust::auth::JwtVerifier;
use http_body_util::{BodyExt as _, Full};
use hyper::{Request, Response, StatusCode};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
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
struct Bench {
    tool_router: ToolRouter<Bench>,
}

#[tool_router]
impl Bench {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

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

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Bench {}

/// Issuer and audience of the token in `bench/fixtures/token.txt`.
const ISSUER: &str = "http://127.0.0.1:9000";
const AUDIENCE: &str = "http://127.0.0.1/mcp";

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn status(code: StatusCode) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::new()).boxed());
    *response.status_mut() = code;
    response
}

/// The same check as FlyMCPRust's OAuth layer: a bearer token, verified by
/// the same JWT verifier, with the `mcp` scope.
async fn authorized(
    verifier: &JwtVerifier,
    request: &Request<hyper::body::Incoming>,
) -> StatusCode {
    let token = request
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token else {
        return StatusCode::UNAUTHORIZED;
    };
    match verifier.verify(token).await {
        Ok(principal) if principal.has_scope("mcp") => StatusCode::OK,
        Ok(_) => StatusCode::FORBIDDEN,
        Err(_) => StatusCode::UNAUTHORIZED,
    }
}

async fn serve_http<S>(
    port: &str,
    make: impl Fn() -> S + Send + Sync + 'static,
    verifier: Option<Arc<JwtVerifier>>,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: ServerHandler + Send + 'static,
{
    use hyper_util::rt::{TokioIo, TokioTimer};

    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = false;
    config.json_response = true;
    let service = StreamableHttpService::new(
        move || Ok(make()),
        NeverSessionManager::default().into(),
        config,
    );
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let service = service.clone();
        let verifier = verifier.clone();
        let hyper_service =
            hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                let mut service = service.clone();
                let verifier = verifier.clone();
                async move {
                    if let Some(verifier) = &verifier {
                        let code = authorized(verifier, &request).await;
                        if code != StatusCode::OK {
                            return Ok::<_, Infallible>(status(code));
                        }
                    }
                    tower_service::Service::call(&mut service, request).await
                }
            });
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .serve_connection(TokioIo::new(stream), hyper_service)
                .await;
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port = args.first().map_or("8002", String::as_str);
    if port == "stdio" {
        use rmcp::ServiceExt as _;
        Bench::new()
            .serve(rmcp::transport::stdio())
            .await?
            .waiting()
            .await?;
        return Ok(());
    }
    let verifier = match args.iter().position(|a| a == "oauth") {
        Some(i) => {
            let jwks = std::fs::read_to_string(args.get(i + 1).ok_or("missing JWKS path")?)?;
            Some(Arc::new(
                JwtVerifier::builder(ISSUER, AUDIENCE).jwks(jwks).build()?,
            ))
        }
        None => None,
    };
    if args.iter().any(|a| a == "real") {
        let tools = real::RealTools::new();
        serve_http(port, move || tools.clone(), verifier).await
    } else {
        let bench = Bench::new();
        serve_http(port, move || bench.clone(), verifier).await
    }
}
