//! Runs the calculator MCP server.
//!
//! ```text
//! calculadora                      # stdio (default)
//! calculadora --http [ADDR]        # Streamable HTTP at http://ADDR/mcp
//! ```
#![forbid(unsafe_code)]

use std::process::ExitCode;

const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8000";

#[tokio::main]
async fn main() -> ExitCode {
    // Logs (with fix-it hints) go to stderr: on stdio, stdout carries MCP
    // messages only.
    flymcprust::logging::init();

    let server = match calculadora::server() {
        Ok(server) => server,
        Err(error) => {
            tracing::error!("invalid server configuration: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("--stdio") => server.serve_stdio().await,
        Some("--http") => {
            let addr = args.get(1).map_or(DEFAULT_HTTP_ADDR, String::as_str);
            tracing::info!("serving Streamable HTTP at http://{addr}/mcp");
            tokio::select! {
                result = server.serve_http(addr) => result,
                _ = tokio::signal::ctrl_c() => Ok(()),
            }
        }
        Some(other) => {
            tracing::error!("unknown argument `{other}`; use --stdio or --http [ADDR]");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "server stopped with an error");
            ExitCode::FAILURE
        }
    }
}
