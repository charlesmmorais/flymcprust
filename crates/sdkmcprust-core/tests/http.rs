//! Streamable HTTP transport (2026-07-28) through the tower service, plus
//! one end-to-end check over TCP.
#![cfg(feature = "http")]
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
use sdkmcprust_core::transport::http::{
    AllowedHosts, AllowedOrigins, HttpConfig, StreamableHttpService,
};
use sdkmcprust_core::{
    BuildError, Ctx, IntoToolResult, Server, ToolDef, ToolError, ToolFuture, input_schema_for,
    parse_arguments, spec,
};
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

fn query(arguments: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    let args: Value = parse_arguments(arguments)?;
    Ok(Box::pin(async move { args.to_string().into_tool_result() }))
}

/// `execute_sql` from the spec, with `region` mirrored into `Mcp-Param-Region`.
fn query_def() -> ToolDef {
    let schema: spec::ToolInputSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "region": { "type": "string", "x-mcp-header": "Region" },
            "limit": { "type": "integer", "x-mcp-header": "Limit" },
            "query": { "type": "string" }
        },
        "required": ["query"]
    }))
    .unwrap();
    ToolDef::new(spec::Tool::new(schema, "execute_sql".to_owned()), query)
}

fn server() -> Server {
    Server::builder("http-test", "0.1.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<EchoArgs>(), "echo".to_owned()),
            echo,
        ))
        .tool_def(query_def())
        .max_message_bytes(4096)
        .build()
        .unwrap()
}

fn service() -> StreamableHttpService {
    StreamableHttpService::new(server())
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// A request with every standard header set correctly for `body`.
fn post(body: &Value) -> http::request::Builder {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "127.0.0.1:8000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28");
    if let Some(method) = body["method"].as_str() {
        builder = builder.header("mcp-method", method);
    }
    if let Some(name) = body["params"]["name"].as_str() {
        builder = builder.header("mcp-name", name);
    }
    builder
}

async fn send(
    service: &mut StreamableHttpService,
    request: Request<Full<Bytes>>,
) -> (StatusCode, http::HeaderMap, Option<Value>) {
    let response = service.call(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = (!bytes.is_empty()).then(|| serde_json::from_slice(&bytes).unwrap());
    (status, headers, body)
}

async fn send_json(
    builder: http::request::Builder,
    body: &Value,
) -> (StatusCode, http::HeaderMap, Option<Value>) {
    let request = builder
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap();
    send(&mut service(), request).await
}

fn call(name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments, "_meta": meta() } })
}

#[tokio::test]
async fn successful_request_returns_json() {
    let body =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });
    let (status, headers, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json");
    assert!(headers.get("mcp-session-id").is_none(), "no sessions");
    assert_eq!(response.unwrap()["result"]["resultType"], "complete");

    let body = call("echo", json!({ "text": "hi" }));
    let (status, _, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response.unwrap()["result"]["content"][0]["text"], "hi");
}

#[tokio::test]
async fn legacy_initialize_names_the_supported_versions() {
    // A 2025-11-25 client opens with `initialize` and no modern headers.
    let body = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": { "name": "legacy", "version": "1" } } });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let (status, _, response) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let response = response.unwrap();
    assert_eq!(response["error"]["code"], -32601);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("2026-07-28")
    );
}

#[tokio::test]
async fn notifications_get_202() {
    let body = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } });
    let (status, _, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(response.is_none());
}

#[tokio::test]
async fn status_codes_follow_the_spec() {
    // Unknown method: 404 + -32601.
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list", "params": { "_meta": meta() } });
    let (status, _, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response.unwrap()["error"]["code"], -32601);

    // Missing `_meta`: 400 + -32602.
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    let (status, _, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response.unwrap()["error"]["code"], -32602);

    // Unsupported version (header and body agree): 400 + -32022.
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2099-01-01",
        "io.modelcontextprotocol/clientCapabilities": {} } } });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("mcp-protocol-version", "2099-01-01")
        .header("mcp-method", "tools/list")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let (status, _, response) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let response = response.unwrap();
    assert_eq!(response["error"]["code"], -32022);
    assert_eq!(
        response["error"]["data"]["supported"],
        json!(["2026-07-28"])
    );

    // Unknown tool is a valid request that failed: 200 + -32602.
    let body = call("nope", json!({}));
    let (status, _, response) = send_json(post(&body), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response.unwrap()["error"]["code"], -32602);

    // Malformed JSON: 400 + -32700.
    let request = post(&json!({}))
        .body(Full::new(Bytes::from_static(b"{nope")))
        .unwrap();
    let (status, _, response) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(response.unwrap()["error"]["code"], -32700);
}

#[tokio::test]
async fn header_validation() {
    let body = call("echo", json!({ "text": "hi" }));
    let expect_mismatch = |r: (StatusCode, http::HeaderMap, Option<Value>)| {
        assert_eq!(r.0, StatusCode::BAD_REQUEST);
        let response = r.2.unwrap();
        assert_eq!(response["error"]["code"], -32020, "{response}");
        assert_eq!(response["id"], 1);
    };

    let base = || {
        Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("host", "localhost:8000")
            .header("content-type", "application/json")
    };
    let full =
        |b: http::request::Builder| b.body(Full::new(Bytes::from(body.to_string()))).unwrap();

    // Missing MCP-Protocol-Version.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-method", "tools/call")
                .header("mcp-name", "echo"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Missing Mcp-Method.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-name", "echo"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Mcp-Method mismatch.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/list")
                .header("mcp-name", "echo"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Missing Mcp-Name on tools/call.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Mcp-Name mismatch.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call")
                .header("mcp-name", "other"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Protocol version header differs from `_meta`.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2025-11-25")
                .header("mcp-method", "tools/call")
                .header("mcp-name", "echo"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Duplicate header.
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call")
                .header("mcp-method", "tools/call")
                .header("mcp-name", "echo"),
        ),
    )
    .await;
    expect_mismatch(r);
    // Base64-encoded Mcp-Name is decoded before comparing ("echo").
    let r = send(
        &mut service(),
        full(
            base()
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call")
                .header("mcp-name", "=?base64?ZWNobw==?="),
        ),
    )
    .await;
    assert_eq!(r.0, StatusCode::OK);
}

#[tokio::test]
async fn mirrored_parameters_are_validated() {
    let body = call(
        "execute_sql",
        json!({ "region": "us-west1", "limit": 42, "query": "SELECT 1" }),
    );
    let ok = send_json(
        post(&body)
            .header("mcp-param-region", "us-west1")
            .header("mcp-param-limit", "42"),
        &body,
    )
    .await;
    assert_eq!(ok.0, StatusCode::OK, "{:?}", ok.2);

    // Integers compare numerically.
    let ok = send_json(
        post(&body)
            .header("mcp-param-region", "us-west1")
            .header("mcp-param-limit", "42.0"),
        &body,
    )
    .await;
    assert_eq!(ok.0, StatusCode::OK);

    // Missing header while the body has a value.
    let r = send_json(post(&body).header("mcp-param-region", "us-west1"), &body).await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
    assert_eq!(r.2.unwrap()["error"]["code"], -32020);

    // Header value differs from the body.
    let r = send_json(
        post(&body)
            .header("mcp-param-region", "eu-west1")
            .header("mcp-param-limit", "42"),
        &body,
    )
    .await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);

    // Absent parameter: the header must be absent too.
    let body = call("execute_sql", json!({ "query": "SELECT 1" }));
    let ok = send_json(post(&body), &body).await;
    assert_eq!(ok.0, StatusCode::OK);
    let r = send_json(post(&body).header("mcp-param-region", "us-west1"), &body).await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
}

#[test]
fn invalid_header_annotations_fail_the_build() {
    let schema: spec::ToolInputSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": { "ratio": { "type": "number", "x-mcp-header": "Ratio" } }
    }))
    .unwrap();
    let result = Server::builder("s", "1")
        .tool_def(ToolDef::new(spec::Tool::new(schema, "t".to_owned()), query))
        .build();
    assert!(matches!(
        result,
        Err(BuildError::InvalidHeaderAnnotation { .. })
    ));
}

#[tokio::test]
async fn origin_and_host_protection() {
    let body =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });

    // A browser page on another site (DNS rebinding): 403.
    let (status, _, _) =
        send_json(post(&body).header("origin", "http://evil.example"), &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Allowed origin: CORS headers echo it.
    let (status, headers, _) =
        send_json(post(&body).header("origin", "http://localhost:6274"), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers["access-control-allow-origin"],
        "http://localhost:6274"
    );

    // Foreign Host header: 403.
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "rebind.example:8000")
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let (status, _, _) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Explicit configuration for a public deployment.
    let mut public = StreamableHttpService::with_config(
        server(),
        HttpConfig::new()
            .allowed_hosts(AllowedHosts::List(vec!["mcp.example.com".into()]))
            .allowed_origins(AllowedOrigins::List(vec!["https://app.example.com".into()])),
    );
    let request = post(&body)
        .header("origin", "https://app.example.com")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let mut request = request;
    request
        .headers_mut()
        .insert("host", "mcp.example.com".parse().unwrap());
    let (status, _, _) = send(&mut public, request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn method_content_type_and_size_checks() {
    // GET and DELETE: 405 (no standalone stream, no sessions).
    for method in [Method::GET, Method::DELETE] {
        let request = Request::builder()
            .method(method)
            .uri("/mcp")
            .header("host", "localhost")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let (status, headers, _) = send(&mut service(), request).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(headers["allow"], "POST");
    }

    // CORS preflight from an allowed origin.
    let request = Request::builder()
        .method(Method::OPTIONS)
        .uri("/mcp")
        .header("host", "localhost")
        .header("origin", "http://localhost:6274")
        .header("access-control-request-headers", "content-type, mcp-method")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let (status, headers, _) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers["access-control-allow-methods"], "POST");

    // Wrong content type: 415.
    let body =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });
    let request = post(&body)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let mut request = request;
    request
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    let (status, _, _) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Oversized body: 413.
    let big = call("echo", json!({ "text": "x".repeat(10_000) }));
    let (status, _, response) = send_json(post(&big), &big).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.unwrap()["error"]["code"], -32600);

    // Batches are not part of the protocol: 400.
    let request = post(&json!({}))
        .body(Full::new(Bytes::from_static(
            b"[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}]",
        )))
        .unwrap();
    let (status, _, _) = send(&mut service(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Session headers from older clients are ignored.
    let body =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } });
    let (status, headers, _) = send_json(
        post(&body)
            .header("mcp-session-id", "abc")
            .header("last-event-id", "5"),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("mcp-session-id").is_none());
}

#[tokio::test]
async fn end_to_end_over_tcp() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(sdkmcprust_core::transport::http::serve(
        listener,
        service(),
        "/mcp",
        async {
            let _ = stopped.await;
        },
    ));

    let exchange = |path: &'static str| async move {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": meta() } }).to_string();
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\n\
             Mcp-Method: server/discover\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    };

    let ok = exchange("/mcp").await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("\"supportedVersions\":[\"2026-07-28\"]"));

    let not_found = exchange("/other").await;
    assert!(not_found.starts_with("HTTP/1.1 404"), "{not_found}");

    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
