//! Progress and log notifications over stdio and Streamable HTTP, for
//! 2026-07-28 and legacy clients.
#![cfg(all(feature = "stdio", feature = "http"))]
#![allow(deprecated)] // Logging is deprecated (SEP-2577) but still tested.
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt as _, Full};
use schemars::JsonSchema;
use sdkmcprust_core::transport::http::{HttpBody, StreamableHttpService};
use sdkmcprust_core::transport::stdio::serve_io;
use sdkmcprust_core::{
    Ctx, IntoToolResult, Server, ToolDef, ToolError, ToolFuture, input_schema_for, spec,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tower_service::Service as _;

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

/// Reports progress 1, 2, 3 of 3 and logs at info and debug.
fn work(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async move {
        for step in 1..=3 {
            ctx.progress(f64::from(step), 3.0);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ctx.log(spec::LoggingLevel::Info, "working");
        ctx.log(spec::LoggingLevel::Debug, json!({ "detail": 1 }));
        "done".into_tool_result()
    }))
}

/// Runs until cancelled, then records it.
#[derive(Clone)]
struct Cancelled(Arc<AtomicBool>);

/// Sets the flag when dropped: the server drops a cancelled handler.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn slow(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    let flag = ctx.state::<Cancelled>().map(|c| Arc::clone(&c.0));
    Ok(Box::pin(async move {
        let _dropped = flag.map(SetOnDrop);
        ctx.progress(1.0, None);
        tokio::time::sleep(Duration::from_secs(10)).await;
        "late".into_tool_result()
    }))
}

fn server(cancelled: &Cancelled) -> Server {
    let schema = || input_schema_for::<NoArgs>();
    Server::builder("notify", "1.0.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(schema(), "work".to_owned()),
            work,
        ))
        .tool_def(
            ToolDef::new(spec::Tool::new(schema(), "slow".to_owned()), slow)
                .require_state::<Cancelled>(),
        )
        .state(cancelled.clone())
        .build()
        .unwrap()
}

fn meta(extra: Value) -> Value {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    meta.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    meta
}

fn call(id: u64, tool: &str, meta: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": {}, "_meta": meta } })
}

// ---------------------------------------------------------------- stdio

struct StdioClient {
    writer: tokio::io::DuplexStream,
    reader: BufReader<tokio::io::DuplexStream>,
}

impl StdioClient {
    fn start(server: Server) -> Self {
        let (client_out, server_in) = tokio::io::duplex(1 << 20);
        let (server_out, client_in) = tokio::io::duplex(1 << 20);
        tokio::spawn(serve_io(server, server_in, server_out));
        Self {
            writer: client_out,
            reader: BufReader::new(client_in),
        }
    }

    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_vec(message).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
    }

    /// Messages up to and including the response with `id`.
    async fn until_response(&mut self, id: u64) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).await.unwrap();
            let message: Value = serde_json::from_str(&line).unwrap();
            let done = message["id"] == id;
            messages.push(message);
            if done {
                return messages;
            }
        }
    }
}

fn methods(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .map(|m| m["method"].as_str().unwrap_or("<response>").to_owned())
        .collect()
}

#[tokio::test]
async fn stdio_sends_notifications_before_the_response() {
    let mut client = StdioClient::start(server(&Cancelled(Arc::new(AtomicBool::new(false)))));
    client
        .send(&call(
            1,
            "work",
            meta(json!({ "progressToken": "p1", "io.modelcontextprotocol/logLevel": "info" })),
        ))
        .await;
    let messages = client.until_response(1).await;
    assert_eq!(
        methods(&messages),
        [
            "notifications/progress",
            "notifications/progress",
            "notifications/progress",
            "notifications/message",
            "<response>"
        ]
    );
    assert_eq!(
        messages[2]["params"],
        json!({ "progressToken": "p1", "progress": 3.0, "total": 3.0 })
    );
    // Debug is below the requested level.
    assert_eq!(messages[3]["params"]["data"], "working");
}

#[tokio::test]
async fn nothing_is_sent_unless_asked() {
    let mut client = StdioClient::start(server(&Cancelled(Arc::new(AtomicBool::new(false)))));
    client.send(&call(1, "work", meta(json!({})))).await;
    assert_eq!(methods(&client.until_response(1).await), ["<response>"]);
}

#[tokio::test]
async fn invalid_log_levels_are_rejected() {
    let mut client = StdioClient::start(server(&Cancelled(Arc::new(AtomicBool::new(false)))));
    client
        .send(&call(
            1,
            "work",
            meta(json!({ "io.modelcontextprotocol/logLevel": "loud" })),
        ))
        .await;
    let messages = client.until_response(1).await;
    assert_eq!(messages.last().unwrap()["error"]["code"], -32602);
}

#[tokio::test]
async fn stdio_legacy_clients_get_logs_at_their_level() {
    let mut client = StdioClient::start(server(&Cancelled(Arc::new(AtomicBool::new(false)))));
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": { "name": "t", "version": "1" } } }),
        )
        .await;
    let init = client.until_response(0).await;
    assert!(init[0]["result"]["capabilities"]["logging"].is_object());

    // Every level by default.
    client
        .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "work", "_meta": { "progressToken": 9 } } }))
        .await;
    let messages = client.until_response(1).await;
    assert_eq!(
        methods(&messages)
            .iter()
            .filter(|m| *m == "notifications/message")
            .count(),
        2
    );
    assert_eq!(
        methods(&messages)
            .iter()
            .filter(|m| *m == "notifications/progress")
            .count(),
        3
    );

    // `logging/setLevel` applies to the rest of the connection.
    client
        .send(
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "logging/setLevel",
            "params": { "level": "warning" } }),
        )
        .await;
    assert_eq!(client.until_response(2).await[0]["result"], json!({}));
    client
        .send(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "work" } }))
        .await;
    assert_eq!(methods(&client.until_response(3).await), ["<response>"]);
}

// ---------------------------------------------------------------- HTTP

fn post(body: &Value) -> Request<Full<Bytes>> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", body["params"]["name"].as_str().unwrap())
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap()
}

async fn sse_messages(body: HttpBody) -> Vec<Value> {
    let bytes = body.collect().await.unwrap().to_bytes();
    std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

#[tokio::test]
async fn http_streams_only_when_there_is_something_to_stream() {
    let cancelled = Cancelled(Arc::new(AtomicBool::new(false)));
    let mut service = StreamableHttpService::new(server(&cancelled));

    // Progress requested: an SSE stream, ending with the response.
    let response = service
        .call(post(&call(
            1,
            "work",
            meta(json!({ "progressToken": "t" })),
        )))
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let messages = sse_messages(response.into_body()).await;
    assert_eq!(
        methods(&messages),
        [
            "notifications/progress",
            "notifications/progress",
            "notifications/progress",
            "<response>"
        ]
    );
    assert_eq!(messages[3]["result"]["content"][0]["text"], "done");

    // Nothing requested: plain JSON, as before.
    let response = service
        .call(post(&call(2, "work", meta(json!({})))))
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "application/json");
}

#[tokio::test]
async fn closing_the_stream_cancels_the_request() {
    let cancelled = Cancelled(Arc::new(AtomicBool::new(false)));
    let mut service = StreamableHttpService::new(server(&cancelled));
    let response = service
        .call(post(&call(1, "slow", meta(json!({ "progressToken": 1 })))))
        .await
        .unwrap();
    let mut body = response.into_body();
    // The first progress update arrives, then the client goes away.
    let first = body.frame().await.unwrap().unwrap();
    assert!(
        std::str::from_utf8(first.data_ref().unwrap())
            .unwrap()
            .contains("notifications/progress")
    );
    drop(body);
    for _ in 0..100 {
        if cancelled.0.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the handler was not cancelled");
}

#[test]
fn discover_declares_logging() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let server = server(&Cancelled(Arc::new(AtomicBool::new(false))));
    let client = sdkmcprust_core::testing::TestClient::new(server);
    let caps = rt.block_on(client.discover()).unwrap().capabilities;
    assert!(caps.logging.is_some());
}
