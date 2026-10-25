//! Server behavior against the 2026-07-28 specification, using hand-written
//! tools (the same shape the `#[tool]` macro generates).

// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use schemars::JsonSchema;
use sdkmcprust_core::{
    BuildError, CallFn, Ctx, IntoToolResult, Json, Server, ServerBuilder, ToolDef, ToolError,
    ToolFuture, TransportContext, input_schema_for, output_schema_from, parse_arguments, spec,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    a: f64,
    b: f64,
}

#[derive(Serialize, JsonSchema)]
struct Sum {
    value: f64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn add(arguments: Option<&RawValue>, _ctx: Ctx) -> Result<ToolFuture, ToolError> {
    let args: AddArgs = parse_arguments(arguments)?;
    Ok(Box::pin(async move {
        Json(Sum {
            value: args.a + args.b,
        })
        .into_tool_result()
    }))
}

fn add_def() -> ToolDef {
    let mut tool = spec::Tool::new(input_schema_for::<AddArgs>(), "add".to_owned());
    tool.description = Some("Adds two numbers.".to_owned());
    tool.output_schema = <Json<Sum> as IntoToolResult>::output_schema().map(output_schema_from);
    ToolDef::new(tool, add)
}

fn simple(name: &str, call: CallFn) -> ToolDef {
    ToolDef::new(
        spec::Tool::new(input_schema_for::<NoArgs>(), name.to_owned()),
        call,
    )
}

fn user_error(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async {
        Err::<String, _>(ToolError::user("b must not be zero")).into_tool_result()
    }))
}

fn internal_error(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async {
        Err::<String, _>(std::io::Error::other("database at 10.0.0.5 refused")).into_tool_result()
    }))
}

fn slow(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        "done".into_tool_result()
    }))
}

#[allow(clippy::unreachable)]
fn panics(_: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    Ok(Box::pin(async {
        let values: Vec<u8> = Vec::new();
        // Deliberate panic: out-of-bounds index.
        #[allow(clippy::indexing_slicing)]
        let v = values[1];
        v.into_tool_result()
    }))
}

struct Counter(AtomicU64);

fn count(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    let counter = ctx
        .state::<Counter>()
        .ok_or_else(|| ToolError::internal(std::io::Error::other("state missing")))?;
    Ok(Box::pin(async move {
        (counter.0.fetch_add(1, Ordering::SeqCst) + 1).into_tool_result()
    }))
}

fn counter_def() -> ToolDef {
    simple("count", count).require_state::<Counter>()
}

fn server() -> Server {
    ServerBuilder::new("test-server", "1.2.3")
        .instructions("Use the tools.")
        .tool_def(simple("slow", slow))
        .tool_def(add_def())
        .tool_def(simple("user_error", user_error))
        .tool_def(simple("internal_error", internal_error))
        .tool_def(simple("panics", panics))
        .tool_def(counter_def())
        .state(Counter(AtomicU64::new(0)))
        .tool_timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test-client", "version": "0.0.1" }
    })
}

async fn send(server: &Server, message: Value) -> Option<Value> {
    let bytes = serde_json::to_vec(&message).unwrap();
    server
        .handle_bytes(
            &bytes,
            TransportContext::default(),
            CancellationToken::new(),
        )
        .await
        .map(|r| serde_json::from_slice(&r.to_vec()).unwrap())
}

async fn request(server: &Server, method: &str, params: Value) -> Value {
    send(
        server,
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
    )
    .await
    .expect("a response")
}

async fn call_tool(server: &Server, name: &str, arguments: Value) -> Value {
    let response = request(
        server,
        "tools/call",
        json!({ "name": name, "arguments": arguments, "_meta": meta() }),
    )
    .await;
    response
        .get("result")
        .cloned()
        .unwrap_or_else(|| panic!("expected a result, got {response}"))
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"].as_i64().unwrap()
}

#[tokio::test]
async fn discover_advertises_versions_capabilities_and_cache_hints() {
    let response = request(&server(), "server/discover", json!({ "_meta": meta() })).await;
    assert_eq!(response["id"], 1);
    let result = &response["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(result["capabilities"]["tools"], json!({}));
    assert_eq!(result["instructions"], "Use the tools.");
    assert_eq!(result["ttlMs"], 3_600_000);
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"],
        json!({ "name": "test-server", "version": "1.2.3" })
    );
    // The result is a valid DiscoverResult.
    serde_json::from_value::<spec::DiscoverResult>(result.clone()).unwrap();
}

#[tokio::test]
async fn tools_list_is_sorted_and_cacheable() {
    let response = request(&server(), "tools/list", json!({ "_meta": meta() })).await;
    let result = &response["result"];
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "add",
            "count",
            "internal_error",
            "panics",
            "slow",
            "user_error"
        ]
    );
    assert_eq!(result["ttlMs"], 300_000);
    assert_eq!(result["cacheScope"], "public");
    assert!(result.get("nextCursor").is_none());

    let add = &result["tools"][0];
    assert_eq!(add["description"], "Adds two numbers.");
    assert_eq!(add["inputSchema"]["type"], "object");
    assert_eq!(add["inputSchema"]["required"], json!(["a", "b"]));
    assert_eq!(add["inputSchema"]["additionalProperties"], false);
    assert_eq!(add["outputSchema"]["type"], "object");
    serde_json::from_value::<spec::ListToolsResult>(result.clone()).unwrap();
}

#[tokio::test]
async fn call_returns_structured_content_and_server_info() {
    let result = call_tool(&server(), "add", json!({ "a": 2, "b": 3.5 })).await;
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["structuredContent"], json!({ "value": 5.5 }));
    assert_eq!(result["content"][0]["type"], "text");
    assert_eq!(result["content"][0]["text"], r#"{"value":5.5}"#);
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "test-server"
    );
    assert!(result.get("isError").is_none());
}

#[tokio::test]
async fn invalid_arguments_are_tool_errors_for_the_model() {
    let result = call_tool(&server(), "add", json!({ "a": 2 })).await;
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("missing field `b`"), "{text}");

    let result = call_tool(&server(), "add", json!({ "a": 1, "b": 2, "c": 3 })).await;
    assert_eq!(result["isError"], true);
}

#[tokio::test]
async fn user_errors_are_visible_and_internal_errors_are_hidden() {
    let s = server();
    let user = call_tool(&s, "user_error", json!({})).await;
    assert_eq!(user["isError"], true);
    assert_eq!(user["content"][0]["text"], "b must not be zero");

    let internal = call_tool(&s, "internal_error", json!({})).await;
    assert_eq!(internal["isError"], true);
    let text = internal.to_string();
    assert!(
        !text.contains("10.0.0.5"),
        "internal details leaked: {text}"
    );

    let panicked = call_tool(&s, "panics", json!({})).await;
    assert_eq!(panicked["isError"], true);
}

#[tokio::test(start_paused = true)]
async fn slow_tools_time_out() {
    let result = call_tool(&server(), "slow", json!({})).await;
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("time limit")
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_requests_get_no_response() {
    let s = server();
    let token = CancellationToken::new();
    let bytes = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": { "name": "slow", "arguments": {}, "_meta": meta() }
    }))
    .unwrap();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        canceller.cancel();
    });
    let response = s
        .handle_bytes(&bytes, TransportContext::default(), token)
        .await;
    assert!(response.is_none());
}

#[tokio::test]
async fn shared_state_is_injected() {
    let s = server();
    assert_eq!(
        call_tool(&s, "count", json!({})).await["content"][0]["text"],
        "1"
    );
    assert_eq!(
        call_tool(&s, "count", json!({})).await["content"][0]["text"],
        "2"
    );
}

#[tokio::test]
async fn protocol_errors() {
    let s = server();

    // Unknown tool: -32602.
    let r = request(&s, "tools/call", json!({ "name": "nope", "_meta": meta() })).await;
    assert_eq!(error_code(&r), -32602);
    assert!(r["error"]["message"].as_str().unwrap().contains("nope"));

    // Missing `_meta`: -32602.
    let r = request(&s, "tools/list", json!({})).await;
    assert_eq!(error_code(&r), -32602);

    // `_meta` without the required capabilities: -32602.
    let r = request(
        &s,
        "tools/list",
        json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } }),
    )
    .await;
    assert_eq!(error_code(&r), -32602);

    // Unsupported version: -32022 with supported/requested.
    let r = request(
        &s,
        "server/discover",
        json!({ "_meta": {
            "io.modelcontextprotocol/protocolVersion": "1900-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}),
    )
    .await;
    assert_eq!(error_code(&r), -32022);
    assert_eq!(
        r["error"]["data"],
        json!({ "supported": ["2026-07-28"], "requested": "1900-01-01" })
    );

    // Unknown method: -32601.
    let r = request(&s, "resources/list", json!({ "_meta": meta() })).await;
    assert_eq!(error_code(&r), -32601);

    // Legacy handshake with legacy clients turned off: -32601 naming the
    // supported versions.
    let strict = ServerBuilder::new("s", "1")
        .legacy_clients(false)
        .build()
        .unwrap();
    let r = request(&strict, "initialize", json!({})).await;
    assert_eq!(error_code(&r), -32601);
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("2026-07-28")
    );

    // A cursor is never valid (single page).
    let r = request(&s, "tools/list", json!({ "cursor": "x", "_meta": meta() })).await;
    assert_eq!(error_code(&r), -32602);

    // Removed in 2026-07-28: `ping`.
    let r = request(&s, "ping", json!({})).await;
    assert_eq!(error_code(&r), -32601);
}

#[tokio::test]
async fn notifications_and_stray_responses_get_no_reply() {
    let s = server();
    assert!(
        send(&s, json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } }))
            .await
            .is_none()
    );
    assert!(
        send(&s, json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn malformed_messages_get_errors_without_id() {
    let s = server();
    let r = s
        .handle_bytes(
            b"{oops",
            TransportContext::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&r.to_vec()).unwrap();
    assert_eq!(error_code(&v), -32700);
    assert!(v.get("id").is_none());
}

fn needs_sampling(_: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError> {
    #[allow(deprecated)] // Sampling is deprecated but still part of the spec.
    let declared = ctx.client_capabilities().sampling.is_some();
    #[allow(deprecated)]
    let required = spec::ClientCapabilities {
        sampling: Some(spec::ClientCapabilitiesSampling::default()),
        ..spec::ClientCapabilities::default()
    };
    Ok(Box::pin(async move {
        if declared {
            "ok".into_tool_result()
        } else {
            Err::<String, _>(ToolError::missing_client_capability(&required)).into_tool_result()
        }
    }))
}

#[tokio::test]
async fn tools_can_return_protocol_errors() {
    let s = ServerBuilder::new("s", "1")
        .tool_def(simple("needs_sampling", needs_sampling))
        .build()
        .unwrap();
    let r = request(
        &s,
        "tools/call",
        json!({ "name": "needs_sampling", "arguments": {}, "_meta": meta() }),
    )
    .await;
    assert_eq!(error_code(&r), -32021);
    assert_eq!(
        r["error"]["data"],
        json!({ "requiredCapabilities": { "sampling": {} } })
    );

    let mut declared = meta();
    declared["io.modelcontextprotocol/clientCapabilities"] = json!({ "sampling": {} });
    let r = request(
        &s,
        "tools/call",
        json!({ "name": "needs_sampling", "arguments": {}, "_meta": declared }),
    )
    .await;
    assert_eq!(r["result"]["content"][0]["text"], "ok");
}

#[test]
fn build_errors_say_how_to_fix_them() {
    struct Db;
    let error = ServerBuilder::new("s", "1")
        .tool_def(simple("needs_db", needs_sampling).require_state::<Db>())
        .build()
        .unwrap_err();
    // `fn main() -> Result<..>` prints `Debug`: it must be readable and
    // include the fix.
    let shown = format!("{error:?}");
    assert!(
        shown.starts_with("`needs_db` needs shared state of type"),
        "{shown}"
    );
    assert!(
        shown.contains("= help: register a value before `.build()`"),
        "{shown}"
    );
    assert!(!format!("{error}").contains("help"));
}

#[test]
fn build_validates_configuration() {
    let dup = ServerBuilder::new("s", "1")
        .tool_def(add_def())
        .tool_def(add_def())
        .build();
    assert!(matches!(dup, Err(BuildError::DuplicateTool(name)) if name == "add"));

    let bad = ServerBuilder::new("s", "1")
        .tool_def(simple("has space", slow))
        .build();
    assert!(matches!(bad, Err(BuildError::InvalidToolName(_))));

    let missing = ServerBuilder::new("s", "1").tool_def(counter_def()).build();
    assert!(matches!(missing, Err(BuildError::MissingState { .. })));

    let zero = ServerBuilder::new("s", "1").max_concurrent_calls(0).build();
    assert!(matches!(zero, Err(BuildError::InvalidLimit(_))));
}

#[tokio::test]
async fn extensions_are_advertised() {
    struct Tasks;
    impl sdkmcprust_core::Extension for Tasks {
        fn id(&self) -> &str {
            "io.modelcontextprotocol/tasks"
        }
    }
    let s = ServerBuilder::new("s", "1")
        .extension(Tasks)
        .build()
        .unwrap();
    let r = request(&s, "server/discover", json!({ "_meta": meta() })).await;
    assert_eq!(
        r["result"]["capabilities"]["extensions"],
        json!({ "io.modelcontextprotocol/tasks": {} })
    );

    struct Bad;
    impl sdkmcprust_core::Extension for Bad {
        fn id(&self) -> &str {
            "no-prefix"
        }
    }
    assert!(ServerBuilder::new("s", "1").extension(Bad).build().is_err());
}

#[tokio::test]
async fn server_is_a_tower_service() {
    use tower_service::Service as _;
    let mut s = server();
    let message = sdkmcprust_core::jsonrpc::parse(
        &serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": "x", "method": "tools/list", "params": { "_meta": meta() }
        }))
        .unwrap(),
        s.limits(),
    )
    .unwrap();
    let response = s
        .call(sdkmcprust_core::McpRequest::new(message))
        .await
        .unwrap()
        .unwrap();
    assert!(response.as_error().is_none());
}
