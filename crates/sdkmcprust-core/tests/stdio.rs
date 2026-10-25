//! stdio transport over an in-memory pipe.
#![cfg(feature = "stdio")]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use schemars::JsonSchema;
use sdkmcprust_core::transport::stdio::serve_io;
use sdkmcprust_core::{
    Ctx, IntoToolResult, Server, ToolDef, ToolError, ToolFuture, input_schema_for, parse_arguments,
    spec,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

#[derive(Deserialize, JsonSchema)]
struct SleepArgs {
    millis: u64,
}

fn sleep(arguments: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    let args: SleepArgs = parse_arguments(arguments)?;
    Ok(Box::pin(async move {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(args.millis)) => {}
            () = ctx.cancellation().cancelled() => {}
        }
        format!("slept {}", args.millis).into_tool_result()
    }))
}

fn server() -> Server {
    Server::builder("stdio-test", "0.1.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<SleepArgs>(), "sleep".to_owned()),
            sleep,
        ))
        .max_message_bytes(1024)
        .build()
        .unwrap()
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

struct Client {
    input: DuplexStream,
    output: BufReader<DuplexStream>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Client {
    fn start() -> Self {
        let (input, server_in) = tokio::io::duplex(64 * 1024);
        let (server_out, output) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(serve_io(server(), server_in, server_out));
        Self {
            input,
            output: BufReader::new(output),
            task,
        }
    }

    async fn send_raw(&mut self, line: &str) {
        self.input.write_all(line.as_bytes()).await.unwrap();
        self.input.write_all(b"\n").await.unwrap();
    }

    async fn send(&mut self, message: Value) {
        self.send_raw(&message.to_string()).await;
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), self.output.read_line(&mut line))
            .await
            .expect("timed out waiting for a response")
            .unwrap();
        assert!(line.ends_with('\n'), "messages are newline-terminated");
        assert_eq!(line.matches('\n').count(), 1, "no embedded newlines");
        serde_json::from_str(&line).unwrap()
    }

    async fn close(self) -> Option<String> {
        drop(self.input);
        self.task.await.unwrap().unwrap();
        let mut rest = String::new();
        let mut output = self.output;
        tokio::io::AsyncReadExt::read_to_string(&mut output, &mut rest)
            .await
            .unwrap();
        (!rest.is_empty()).then_some(rest)
    }
}

#[tokio::test]
async fn request_response_roundtrip() {
    let mut client = Client::start();
    client
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": meta() } }))
        .await;
    let response = client.recv().await;
    assert_eq!(response["id"], 1);
    assert_eq!(
        response["result"]["supportedVersions"],
        json!(["2026-07-28"])
    );
    assert!(client.close().await.is_none());
}

#[tokio::test]
async fn concurrent_requests_complete_out_of_order() {
    let mut client = Client::start();
    let call = |id: i64, millis: u64| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "sleep", "arguments": { "millis": millis }, "_meta": meta() } })
    };
    client.send(call(1, 300)).await;
    client.send(call(2, 1)).await;
    assert_eq!(client.recv().await["id"], 2);
    assert_eq!(client.recv().await["id"], 1);
    client.close().await;
}

#[tokio::test]
async fn cancelled_request_gets_no_response() {
    let mut client = Client::start();
    client
        .send(json!({ "jsonrpc": "2.0", "id": "slow", "method": "tools/call",
                      "params": { "name": "sleep", "arguments": { "millis": 60000 }, "_meta": meta() } }))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client
        .send(
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": { "requestId": "slow", "reason": "user" } }),
        )
        .await;
    client
        .send(json!({ "jsonrpc": "2.0", "id": "next", "method": "tools/list", "params": { "_meta": meta() } }))
        .await;
    // Only the second request is answered.
    assert_eq!(client.recv().await["id"], "next");
    assert!(client.close().await.is_none());
}

#[tokio::test]
async fn invalid_lines_get_errors_and_the_stream_continues() {
    let mut client = Client::start();
    client.send_raw("{not json").await;
    assert_eq!(client.recv().await["error"]["code"], -32700);

    client.send_raw(&"x".repeat(5000)).await;
    let too_long = client.recv().await;
    assert_eq!(too_long["error"]["code"], -32600);
    assert!(too_long.get("id").is_none());

    // Blank lines are ignored; a stray response gets no reply.
    client.send_raw("   ").await;
    client
        .send(json!({ "jsonrpc": "2.0", "id": 5, "result": {} }))
        .await;

    client
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": { "_meta": meta() } }))
        .await;
    assert_eq!(client.recv().await["id"], 2);
    client.close().await;
}

#[tokio::test]
async fn duplicate_in_flight_ids_are_rejected() {
    let mut client = Client::start();
    let slow = json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                       "params": { "name": "sleep", "arguments": { "millis": 200 }, "_meta": meta() } });
    client.send(slow.clone()).await;
    client.send(slow).await;
    let first = client.recv().await;
    assert_eq!(first["error"]["code"], -32600);
    let second = client.recv().await;
    assert_eq!(second["result"]["content"][0]["text"], "slept 200");
    client.close().await;
}

#[tokio::test]
async fn eof_waits_for_in_flight_requests() {
    let mut client = Client::start();
    client
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": { "name": "sleep", "arguments": { "millis": 100 }, "_meta": meta() } }))
        .await;
    let rest = client
        .close()
        .await
        .expect("the in-flight response is flushed");
    let response: Value = serde_json::from_str(rest.trim()).unwrap();
    assert_eq!(response["id"], 1);
}
