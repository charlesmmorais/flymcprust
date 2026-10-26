//! Serves the MCP Apps demo over Streamable HTTP.
//!
//! ```text
//! cargo run -p apps-demo                # http://127.0.0.1:3001/mcp
//! cargo run -p apps-demo -- 127.0.0.1:8000
//! ```

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    flymcprust::logging::init();
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3001".to_owned());
    tracing::info!("MCP Apps demo on http://{addr}/mcp");
    apps_demo::server()?.serve_http(addr).await?;
    Ok(())
}
