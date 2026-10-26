//! OAuth 2.1 resource server over Streamable HTTP: challenges, scopes,
//! the metadata document, and binding of `requestState` and legacy sessions
//! to the authenticated user.
#![cfg(feature = "http")]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Full};
use mcpsdkrust_core::transport::http::{
    AllowedHosts, HttpBody, HttpConfig, OAuth, StreamableHttpService, serve,
};
use mcpsdkrust_core::{
    Ctx, IntoToolResult, Principal, Server, TokenError, ToolDef, ToolError, ToolFuture,
    input_schema_for, spec,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower_service::Service as _;

const RESOURCE: &str = "http://localhost:8000/mcp";
const METADATA: &str = "http://localhost:8000/.well-known/oauth-protected-resource/mcp";

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[derive(Deserialize, JsonSchema)]
struct Confirm {
    confirmed: bool,
}

/// Tokens: `alice` (mcp, files), `bob` (mcp), `nobody` (no scopes).
fn verify(token: &str) -> Result<Principal, TokenError> {
    let scopes: &[&str] = match token {
        "alice" => &["mcp", "files"],
        "bob" => &["mcp"],
        "nobody" => &[],
        _ => return Err(TokenError::new("unknown token")),
    };
    Ok(Principal::new(token)
        .with_issuer("https://auth.example.com")
        .with_scopes(scopes.iter().copied()))
}

/// `files` implies every `files:*` scope.
fn implies(granted: &str, required: &str) -> bool {
    required
        .strip_prefix(granted)
        .is_some_and(|rest| rest.starts_with(':'))
}

fn whoami(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async move {
        ctx.principal()
            .map(|p| p.subject().to_owned())
            .unwrap_or_default()
            .into_tool_result()
    }))
}

fn write_file(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async { "written".into_tool_result() }))
}

fn confirm(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async move {
        let answer: Confirm = ctx.elicit("confirm", "Sure?").await?;
        answer.confirmed.into_tool_result()
    }))
}

fn server() -> Server {
    let tool = |name: &str, call| {
        ToolDef::new(
            spec::Tool::new(input_schema_for::<NoArgs>(), name.to_owned()),
            call,
        )
    };
    Server::builder("oauth", "1.0.0")
        .tool_def(tool("whoami", whoami))
        .tool_def(tool("write_file", write_file).scopes(["files:write", "mcp"]))
        .tool_def(tool("confirm", confirm))
        .build()
        .unwrap()
}

fn oauth() -> OAuth {
    OAuth::builder(RESOURCE, verify)
        .authorization_server("https://auth.example.com")
        .scopes_supported(["mcp"])
        .required_scopes(["mcp"])
        .scope_implies(implies)
        .build()
        .unwrap()
}

fn service() -> StreamableHttpService {
    StreamableHttpService::with_config(server(), HttpConfig::new().oauth(oauth()))
}

fn modern_call(id: u64, tool: &str, extra: Value) -> Value {
    let mut params = json!({ "name": tool, "arguments": {}, "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": {} },
    } });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params })
}

fn post(body: &Value, token: Option<&str>) -> Request<Full<Bytes>> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost:8000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if body["method"] == "tools/call" && body["params"]["_meta"].is_object() {
        builder = builder
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", body["params"]["name"].as_str().unwrap());
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap()
}

async fn body_json(response: Response<HttpBody>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn challenge(response: &Response<HttpBody>) -> &str {
    response.headers()["www-authenticate"].to_str().unwrap()
}

#[tokio::test]
async fn requests_without_a_valid_token_are_challenged() {
    let mut service = service();
    let call = modern_call(1, "whoami", json!({}));

    let response = service.call(post(&call, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenge(&response),
        format!("Bearer resource_metadata=\"{METADATA}\", scope=\"mcp\"")
    );

    let response = service.call(post(&call, Some("forged"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge(&response).ends_with("error=\"invalid_token\""));
    // The verifier's reason stays on the server.
    let body = body_json(response).await;
    assert!(!body.to_string().contains("unknown token"));

    // Missing the server-wide scope.
    let response = service.call(post(&call, Some("nobody"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(challenge(&response).contains("error=\"insufficient_scope\""));
}

#[tokio::test]
async fn handlers_see_the_principal() {
    let mut service = service();
    let response = service
        .call(post(&modern_call(1, "whoami", json!({})), Some("bob")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["result"]["content"][0]["text"],
        "bob"
    );
}

#[tokio::test]
async fn handler_scopes_are_enforced_with_a_single_challenge() {
    let mut service = service();
    let call = modern_call(1, "write_file", json!({}));

    let response = service.call(post(&call, Some("bob"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // Every scope the tool needs, in one challenge.
    assert_eq!(
        challenge(&response),
        format!(
            "Bearer resource_metadata=\"{METADATA}\", scope=\"files:write mcp\", \
             error=\"insufficient_scope\""
        )
    );
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["data"]["requiredScopes"],
        json!(["files:write", "mcp"])
    );

    // `files` implies `files:write`.
    let response = service.call(post(&call, Some("alice"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["result"]["content"][0]["text"],
        "written"
    );
}

#[tokio::test]
async fn request_state_is_bound_to_the_user() {
    let mut service = service();
    let response = service
        .call(post(&modern_call(1, "confirm", json!({})), Some("alice")))
        .await
        .unwrap();
    let first = body_json(response).await;
    assert_eq!(first["result"]["resultType"], "input_required");
    let state = first["result"]["requestState"].clone();
    let retry = modern_call(
        2,
        "confirm",
        json!({
            "requestState": state,
            "inputResponses": { "confirm": { "action": "accept", "content": { "confirmed": true } } },
        }),
    );

    // Bob cannot complete Alice's request.
    let response = service.call(post(&retry, Some("bob"))).await.unwrap();
    assert_eq!(body_json(response).await["error"]["code"], -32602);

    let response = service.call(post(&retry, Some("alice"))).await.unwrap();
    assert_eq!(
        body_json(response).await["result"]["content"][0]["text"],
        "true"
    );
}

#[tokio::test]
async fn legacy_sessions_are_bound_to_the_user() {
    let mut service = service();
    let init = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": { "name": "t", "version": "1" } } });
    let response = service.call(post(&init, Some("alice"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"].clone();

    let ping = |token: &str| {
        let mut request = post(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
            Some(token),
        );
        request
            .headers_mut()
            .insert("mcp-session-id", session.clone());
        request
            .headers_mut()
            .insert("mcp-protocol-version", "2025-11-25".parse().unwrap());
        request
    };
    let response = service.call(ping("alice")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = service.call(ping("bob")).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serve_publishes_the_metadata_document() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = HttpConfig::new()
        .oauth(oauth())
        .allowed_hosts(AllowedHosts::Any);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(
        listener,
        StreamableHttpService::with_config(server(), config),
        "/mcp",
        async {
            let _ = stopped.await;
        },
    ));

    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).await.unwrap();
        assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["resource"], RESOURCE);
        assert_eq!(
            body["authorization_servers"],
            json!(["https://auth.example.com"])
        );
    }

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn malformed_handler_scopes_fail_the_build() {
    let error = Server::builder("s", "1")
        .tool_def(
            ToolDef::new(
                spec::Tool::new(input_schema_for::<NoArgs>(), "t".to_owned()),
                whoami,
            )
            .scopes(["has space"]),
        )
        .build()
        .unwrap_err();
    assert!(format!("{error:?}").contains("invalid scope"));
}
