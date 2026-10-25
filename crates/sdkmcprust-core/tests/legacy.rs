//! Legacy clients (2025-11-25, 2025-06-18) over stdio and Streamable HTTP:
//! `initialize`, sessions, wire-format differences, and input requests sent
//! as server-to-client requests.
#![cfg(all(feature = "stdio", feature = "http"))]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use http::{Method, Request, StatusCode};
use http_body_util::{BodyExt as _, Full};
use schemars::JsonSchema;
use sdkmcprust_core::transport::http::StreamableHttpService;
use sdkmcprust_core::transport::stdio::serve_io;
use sdkmcprust_core::{
    Ctx, IntoToolResult, ResourceDef, Server, ToolDef, ToolError, ToolFuture, input_schema_for,
    spec,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tower_service::Service as _;

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[derive(Deserialize, JsonSchema)]
struct Confirm {
    confirmed: bool,
}

fn hello(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async { "hello".into_tool_result() }))
}

fn confirm(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async move {
        let answer: Confirm = ctx.elicit("confirm", "Sure?").await?;
        answer.confirmed.into_tool_result()
    }))
}

fn server(legacy: bool) -> Server {
    let resource = ResourceDef::new(
        spec::Resource::new("doc".to_owned(), "doc://one".to_owned()),
        |_, _, _| Ok(Box::pin(async { Ok(Vec::new()) })),
    );
    Server::builder("legacy", "1.0.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<NoArgs>(), "hello".to_owned()),
            hello,
        ))
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<NoArgs>(), "confirm".to_owned()),
            confirm,
        ))
        .resource_def(resource)
        .legacy_clients(legacy)
        .build()
        .unwrap()
}

fn initialize(version: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": version,
        "capabilities": { "elicitation": {} },
        "clientInfo": { "name": "legacy-test", "version": "1" } } })
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
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

    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

#[tokio::test]
async fn stdio_legacy_session() {
    let mut client = StdioClient::start(server(true));

    client.send(&initialize("2025-11-25")).await;
    let init = client.receive().await;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["serverInfo"]["name"], "legacy");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;

    client.send(&request(1, "ping", json!({}))).await;
    assert_eq!(client.receive().await["result"], json!({}));

    // No `_meta`, no `resultType`, no caching hints.
    client.send(&request(2, "tools/list", json!({}))).await;
    let list = client.receive().await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 2);
    assert!(list["result"].get("resultType").is_none());
    assert!(list["result"].get("ttlMs").is_none());

    // `_meta` with only a progress token is fine for legacy clients.
    client
        .send(&request(
            3,
            "tools/call",
            json!({ "name": "hello", "_meta": { "progressToken": 7 } }),
        ))
        .await;
    assert_eq!(
        client.receive().await["result"]["content"][0]["text"],
        "hello"
    );

    // Resource not found uses the 2025-11-25 code.
    client
        .send(&request(
            4,
            "resources/read",
            json!({ "uri": "doc://missing" }),
        ))
        .await;
    assert_eq!(client.receive().await["error"]["code"], -32002);
}

#[tokio::test]
async fn stdio_input_requests_become_server_requests() {
    let mut client = StdioClient::start(server(true));
    client.send(&initialize("2025-06-18")).await;
    assert_eq!(
        client.receive().await["result"]["protocolVersion"],
        "2025-06-18"
    );

    client
        .send(&request(1, "tools/call", json!({ "name": "confirm" })))
        .await;
    let ask = client.receive().await;
    assert_eq!(ask["method"], "elicitation/create");
    assert_eq!(ask["params"]["message"], "Sure?");
    let server_id = ask["id"].clone();
    assert!(
        server_id.as_str().unwrap().len() > 30,
        "server request ids are random"
    );

    client
        .send(&json!({ "jsonrpc": "2.0", "id": server_id,
            "result": { "action": "accept", "content": { "confirmed": true } } }))
        .await;
    let done = client.receive().await;
    assert_eq!(done["id"], 1);
    assert_eq!(done["result"]["content"][0]["text"], "true");
    assert!(done["result"].get("resultType").is_none());
}

#[tokio::test]
async fn stdio_unknown_versions_get_the_newest_legacy_version() {
    let mut client = StdioClient::start(server(true));
    client.send(&initialize("2024-11-05")).await;
    assert_eq!(
        client.receive().await["result"]["protocolVersion"],
        "2025-11-25"
    );
}

#[tokio::test]
async fn legacy_clients_can_be_disabled() {
    let mut client = StdioClient::start(server(false));
    client.send(&initialize("2025-11-25")).await;
    assert_eq!(client.receive().await["error"]["code"], -32601);
}

// ---------------------------------------------------------------- HTTP

fn post(body: &Value) -> http::request::Builder {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-test-body", body["method"].as_str().unwrap_or("response"))
}

fn with_body(builder: http::request::Builder, body: &Value) -> Request<Full<Bytes>> {
    builder
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap()
}

async fn json_body(response: http::Response<sdkmcprust_core::transport::http::HttpBody>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Initializes over HTTP and returns the session token.
async fn http_session(service: &mut StreamableHttpService) -> String {
    let body = initialize("2025-11-25");
    let response = service.call(with_body(post(&body), &body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let token = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let init = json_body(response).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    token
}

fn legacy_post(body: &Value, token: &str) -> http::request::Builder {
    post(body)
        .header("mcp-protocol-version", "2025-11-25")
        .header("mcp-session-id", token)
}

#[tokio::test]
async fn http_legacy_session() {
    let mut service = StreamableHttpService::new(server(true));
    let token = http_session(&mut service).await;

    // No Mcp-Method / Mcp-Name headers needed, no `_meta`.
    let body = request(1, "tools/call", json!({ "name": "hello" }));
    let response = service
        .call(with_body(legacy_post(&body, &token), &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = json_body(response).await;
    assert_eq!(result["result"]["content"][0]["text"], "hello");
    assert!(result["result"].get("resultType").is_none());

    // Notifications are accepted.
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let response = service
        .call(with_body(legacy_post(&note, &token), &note))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    // A tampered or foreign session token is unknown: 404, initialize again.
    let response = service
        .call(with_body(legacy_post(&body, &format!("{token}x")), &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let other = StreamableHttpService::new(server(true));
    let mut other = other;
    let response = other
        .call(with_body(legacy_post(&body, &token), &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Sessions cannot be deleted (they expire): 405.
    let delete = Request::builder()
        .method(Method::DELETE)
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("mcp-session-id", &token)
        .body(Full::new(Bytes::new()))
        .unwrap();
    assert_eq!(
        service.call(delete).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

/// Reads the next SSE event of a streaming response body.
async fn next_event(body: &mut sdkmcprust_core::transport::http::HttpBody) -> Value {
    let frame = body.frame().await.unwrap().unwrap();
    let data = frame.into_data().unwrap();
    let text = std::str::from_utf8(&data).unwrap();
    let json = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn http_input_requests_stream_over_sse() {
    let mut service = StreamableHttpService::new(server(true));
    let token = http_session(&mut service).await;

    let body = request(5, "tools/call", json!({ "name": "confirm" }));
    let response = service
        .call(with_body(legacy_post(&body, &token), &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body();

    let ask = next_event(&mut stream).await;
    assert_eq!(ask["method"], "elicitation/create");
    let answer = json!({ "jsonrpc": "2.0", "id": ask["id"],
        "result": { "action": "accept", "content": { "confirmed": false } } });

    // Another session cannot answer this request.
    let mut intruder = service.clone();
    let intruder_token = http_session(&mut intruder).await;
    let response = intruder
        .call(with_body(legacy_post(&answer, &intruder_token), &answer))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = service
        .call(with_body(legacy_post(&answer, &token), &answer))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let done = next_event(&mut stream).await;
    assert_eq!(done["id"], 5);
    assert_eq!(done["result"]["content"][0]["text"], "false");
    assert!(stream.frame().await.is_none(), "the stream ends");
}

#[tokio::test]
async fn http_modern_requests_ignore_session_headers() {
    let mut service = StreamableHttpService::new(server(true));
    let token = http_session(&mut service).await;
    let body = request(
        1,
        "tools/call",
        json!({ "name": "hello", "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {} } }),
    );
    let builder = post(&body)
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "hello")
        .header("mcp-session-id", &token);
    let response = service.call(with_body(builder, &body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = json_body(response).await;
    assert_eq!(result["result"]["resultType"], "complete");
}

#[tokio::test]
async fn http_modern_initialize_is_method_not_found() {
    let mut service = StreamableHttpService::new(server(true));
    let body = initialize("2025-11-25");
    let builder = post(&body).header("mcp-protocol-version", "2026-07-28");
    let response = service.call(with_body(builder, &body)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"]["code"], -32601);
}
