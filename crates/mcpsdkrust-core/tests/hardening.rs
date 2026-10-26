//! Regression tests for the security review of phases 4 to 6 (legacy
//! sessions, SSE streaming, OAuth): see the 2026-10-25 entry of
//! `docs/SECURITY.md`.
#![cfg(feature = "http")]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, StatusCode};
use http_body_util::Full;
use mcpsdkrust_core::transport::http::{HttpConfig, OAuth, StreamableHttpService};
use mcpsdkrust_core::{
    Ctx, IntoToolResult, Principal, Server, TokenError, ToolDef, ToolError, ToolFuture,
    input_schema_for, spec,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tower_service::Service as _;

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

fn slow(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        "late".into_tool_result()
    }))
}

/// Reports whether the handler can see an `Authorization` header.
fn sees_token(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async move {
        ctx.transport()
            .get::<http::request::Parts>()
            .is_some_and(|parts| parts.headers.contains_key(http::header::AUTHORIZATION))
            .into_tool_result()
    }))
}

fn server() -> Server {
    let tool = |name: &str, call| {
        ToolDef::new(
            spec::Tool::new(input_schema_for::<NoArgs>(), name.to_owned()),
            call,
        )
    };
    Server::builder("hardening", "1.0.0")
        .tool_def(tool("slow", slow))
        .tool_def(tool("sees_token", sees_token))
        .build()
        .unwrap()
}

fn legacy_post(body: &Value) -> Request<Full<Bytes>> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .header("authorization", "Bearer t")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

fn call(tool: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": {} } })
}

/// A legacy request keeps counting against the in-flight limit after its
/// client disconnects, while its handler still runs (legacy requests are
/// not cancelled by a disconnection).
#[tokio::test]
async fn abandoned_legacy_requests_still_count_as_in_flight() {
    let mut service =
        StreamableHttpService::with_config(server(), HttpConfig::new().max_in_flight(1));
    let abandoned = tokio::time::timeout(
        Duration::from_millis(200),
        service.call(legacy_post(&call("slow"))),
    )
    .await;
    assert!(abandoned.is_err(), "the slow call should still be running");

    let response = service
        .call(legacy_post(&call("sees_token")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// Handlers never see the bearer token, so they cannot pass it through to
/// other services by mistake (the specification forbids token passthrough).
#[tokio::test]
async fn handlers_do_not_see_the_access_token() {
    let verify = |_: &str| -> Result<Principal, TokenError> { Ok(Principal::new("u")) };
    let oauth = OAuth::builder("http://localhost/mcp", verify)
        .authorization_server("http://localhost:9000")
        .build()
        .unwrap();
    let mut service = StreamableHttpService::with_config(server(), HttpConfig::new().oauth(oauth));
    let response = service
        .call(legacy_post(&call("sees_token")))
        .await
        .unwrap();
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["result"]["content"][0]["text"], "false");
}

/// What a legacy client declares is carried in its session token, so it is
/// bounded.
#[tokio::test]
async fn oversized_legacy_declarations_are_refused() {
    let mut service = StreamableHttpService::new(server());
    let huge = "x".repeat(64 * 1024);
    let init = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25",
        "capabilities": { "experimental": { "padding": huge } },
        "clientInfo": { "name": "t", "version": "1" } } });
    let mut request = legacy_post(&init);
    request.headers_mut().remove("mcp-protocol-version");
    let response = service.call(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!response.headers().contains_key("mcp-session-id"));
}
