//! Security regression tests: each one reproduces a finding of the
//! 2026-10-25 security review (numbers refer to docs/SECURITY.md §6).
#![cfg(feature = "test-client")]
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::str::FromStr;

use mcpsdkrust::prelude::*;
use mcpsdkrust::spec;
use mcpsdkrust::testing::{ClientError, TestClient};
use serde::Deserialize;
use serde_json::{Value, json};

/// Echoes a file name, trusting that `{name}` holds no path separator.
#[resource(template = "file:///docs/{name}")]
fn doc(name: String) -> String {
    format!("doc {name}")
}

/// A value whose parser panics.
struct Boom;

impl FromStr for Boom {
    type Err = std::convert::Infallible;
    fn from_str(_: &str) -> Result<Self, Self::Err> {
        panic!("parser bug");
    }
}

/// A prompt whose argument parser panics.
#[prompt]
fn explode(_value: Boom) -> &'static str {
    "unreachable"
}

/// Takes a number.
#[tool]
fn square(x: f64) -> f64 {
    x * x
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Confirm {
    confirmed: bool,
}

/// Asks for confirmation.
#[tool]
async fn confirm(ctx: Ctx) -> Result<String, ToolError> {
    let c: Confirm = ctx.elicit("confirm", "Sure?").await?;
    Ok(c.confirmed.to_string())
}

fn client() -> TestClient {
    let server = Server::builder("sec", "1.0.0")
        .resource::<doc>()
        .prompt::<explode>()
        .tool::<square>()
        .tool::<confirm>()
        .build()
        .unwrap();
    TestClient::new(server).with_meta(json!({
        "io.modelcontextprotocol/protocolVersion": spec::PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": {} },
    }))
}

fn rpc_code(err: ClientError) -> i64 {
    match err {
        ClientError::Rpc { code, .. } => code,
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

/// Finding 1: percent-encoded separators and control characters must not
/// reach a `{var}` handler, which the docs promise holds no `/`.
#[tokio::test]
async fn encoded_separators_never_reach_simple_variables() {
    let client = client();
    assert_eq!(
        client
            .read_resource("file:///docs/readme")
            .await
            .unwrap()
            .contents
            .len(),
        1
    );
    for uri in [
        "file:///docs/..%2F..%2Fetc%2Fpasswd",
        "file:///docs/..%2f..%2fetc%2fpasswd",
        "file:///docs/..\\..\\secret",
        "file:///docs/..%5C..%5Csecret",
        "file:///docs/name%00.txt",
        "file:///docs/line%0Abreak",
    ] {
        let err = client.read_resource(uri).await.unwrap_err();
        assert_eq!(rpc_code(err), -32602, "{uri} must not match");
    }
}

/// Finding 2: a panic while parsing arguments (user `FromStr`) must become
/// an internal error, not tear down the request handling.
#[tokio::test]
async fn panics_while_parsing_arguments_are_contained() {
    let client = client();
    let err = client
        .get_prompt("explode", json!({ "_value": "x" }))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), -32603);
    // The server keeps working.
    let ok = client.call_tool("square", json!({ "x": 3 })).await.unwrap();
    assert_eq!(ok.content.len(), 1);
}

/// Finding 6: deserialization errors must not echo large client input.
#[tokio::test]
async fn error_messages_do_not_echo_large_input() {
    let client = client();
    let huge = "A".repeat(100_000);
    let result = client
        .call_tool("square", json!({ "x": huge }))
        .await
        .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text.len() < 2_000, "error echoed {} bytes", text.len());

    let err = client
        .request::<Value>("tools/call", json!({ "name": huge }))
        .await
        .unwrap_err();
    assert!(err.to_string().len() < 2_000);
}

/// Finding 5: answers the handler never asked for must not be signed into
/// `requestState` (state amplification).
#[tokio::test]
async fn unrequested_answers_are_not_persisted() {
    let client = client();
    let junk = "J".repeat(200_000);
    let r: Value = client
        .request(
            "tools/call",
            json!({ "name": "confirm", "inputResponses": { "junk": { "action": "accept", "content": { "x": junk } } } }),
        )
        .await
        .unwrap();
    assert_eq!(r["resultType"], "input_required");
    let state = r["requestState"].as_str().unwrap();
    assert!(state.len() < 1_000, "requestState is {} bytes", state.len());
}
