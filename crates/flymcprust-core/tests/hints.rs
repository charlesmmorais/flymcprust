//! Developer hints in the log. A separate test binary: `tracing` caches
//! callsite interest globally, which makes log capture flaky next to tests
//! running in parallel threads.
#![cfg(feature = "http")]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use flymcprust_core::transport::http::StreamableHttpService;
use flymcprust_core::{
    Ctx, IntoToolResult, Server, ToolDef, ToolError, ToolFuture, input_schema_for, parse_arguments,
    spec,
};
use http::{Method, Request, StatusCode};
use http_body_util::Full;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tower_service::Service as _;

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

fn echo(arguments: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    let args: EchoArgs = parse_arguments(arguments)?;
    Ok(Box::pin(async move { args.text.into_tool_result() }))
}

fn service() -> StreamableHttpService {
    let server = Server::builder("hints", "0.1.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<EchoArgs>(), "echo".to_owned()),
            echo,
        ))
        .build()
        .unwrap();
    StreamableHttpService::new(server)
}

fn post(body: &Value) -> http::request::Builder {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header(
            "mcp-name",
            body["params"]["name"].as_str().unwrap_or_default(),
        )
}

fn call(name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments, "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {} } } })
}

/// Collects log output in memory.
#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Rejections tell the developer how to fix them, once (a client cannot
/// flood the log), with the client's value shortened.
#[tokio::test(flavor = "current_thread")]
async fn rejections_log_a_fix_once() {
    let logs = LogBuffer::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut service = service();
    let body = call("echo", json!({ "text": "hi" }));
    for _ in 0..3 {
        let mut request = post(&body)
            .body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
            .unwrap();
        request.headers_mut().insert(
            "host",
            format!("attacker.example{}", "x".repeat(5_000))
                .parse()
                .unwrap(),
        );
        let response = service.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    // A second `Host` header (RFC 9112 forbids it) is rejected too.
    let request = post(&body)
        .header("host", "127.0.0.1:8000")
        .body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
        .unwrap();
    assert_eq!(
        service.call(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.matches("= help:").count(), 1, "{text}");
    assert!(text.contains("allowed_hosts"), "{text}");
    // The client's value is shortened in the log.
    assert!(text.len() < 2_000, "{} bytes of log", text.len());
}
