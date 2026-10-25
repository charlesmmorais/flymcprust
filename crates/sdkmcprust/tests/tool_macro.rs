//! `#[tool]` end to end: generated schemas, descriptions, annotations,
//! extractors and calls.
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicU64, Ordering};

use sdkmcprust::prelude::*;
use sdkmcprust::{TransportContext, spec};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Adds two numbers.
///
/// Returns their sum.
///
/// # Arguments
/// * `a` - First operand.
/// * `b` - Second operand.
///
/// # Examples
/// Not sent to the model.
#[tool(read_only, idempotent)]
async fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// Divides `a` by `b`.
#[tool(name = "safe-divide", title = "Safe division", destructive = false)]
fn divide(
    /// The dividend.
    a: f64,
    /// The divisor (non-zero).
    b: f64,
) -> Result<f64, ToolError> {
    if b == 0.0 {
        return Err(ToolError::user("b must not be zero"));
    }
    Ok(a / b)
}

/// Statistics of a list of numbers.
#[derive(Serialize, schemars::JsonSchema)]
struct Stats {
    /// Number of values.
    count: usize,
    /// Arithmetic mean.
    mean: f64,
}

/// Computes statistics.
#[tool(open_world = false)]
async fn stats(
    values: Vec<f64>,
    #[serde(default)]
    #[schemars(range(min = 0, max = 10))]
    precision: u32,
) -> Result<Json<Stats>, ToolError> {
    if values.is_empty() {
        return Err(ToolError::user("values must not be empty"));
    }
    let factor = 10f64.powi(i32::try_from(precision).unwrap_or(0));
    let count = values.len();
    let mean = values.iter().sum::<f64>() / f64::from(u32::try_from(count).unwrap_or(u32::MAX));
    Ok(Json(Stats {
        count,
        mean: (mean * factor).round() / factor,
    }))
}

struct Counter(AtomicU64);

/// Counts calls and greets the caller.
#[tool]
async fn greet(ctx: Ctx, counter: State<Counter>, name: Option<String>) -> String {
    let n = counter.0.fetch_add(1, Ordering::SeqCst) + 1;
    let client = ctx.client_info().map_or("unknown", |c| c.name.as_str());
    format!(
        "Hello, {}! (call {n}, client {client})",
        name.as_deref().unwrap_or("world")
    )
}

/// Takes no input.
#[tool]
fn ping() -> &'static str {
    "pong"
}

fn server() -> Server {
    Server::builder("macro-test", "0.1.0")
        .tool::<add>()
        .tool::<divide>()
        .tool::<stats>()
        .tool::<greet>()
        .tool::<ping>()
        .state(Counter(AtomicU64::new(0)))
        .build()
        .unwrap()
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "macro-client", "version": "1" }
    })
}

async fn request(server: &Server, method: &str, params: Value) -> Value {
    let message = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let response = server
        .handle_bytes(
            &serde_json::to_vec(&message).unwrap(),
            TransportContext::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    serde_json::from_slice(&response.to_vec()).unwrap()
}

async fn call(server: &Server, name: &str, arguments: Value) -> Value {
    let response = request(
        server,
        "tools/call",
        json!({ "name": name, "arguments": arguments, "_meta": meta() }),
    )
    .await;
    response["result"].clone()
}

async fn tools() -> Vec<Value> {
    let response = request(&server(), "tools/list", json!({ "_meta": meta() })).await;
    response["result"]["tools"].as_array().unwrap().clone()
}

fn find<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools.iter().find(|t| t["name"] == name).unwrap()
}

#[tokio::test]
async fn schemas_and_descriptions() {
    let tools = tools().await;
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["add", "greet", "ping", "safe-divide", "stats"]);

    let add = find(&tools, "add");
    assert_eq!(
        add["description"],
        "Adds two numbers.\n\nReturns their sum."
    );
    assert_eq!(
        add["inputSchema"],
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First operand." },
                "b": { "type": "number", "description": "Second operand." }
            },
            "required": ["a", "b"],
            "additionalProperties": false
        })
    );
    assert_eq!(
        add["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": true })
    );
    assert!(add.get("outputSchema").is_none());

    // Doc comments on parameters, name and title overrides, explicit `= false`.
    let divide = find(&tools, "safe-divide");
    assert_eq!(divide["title"], "Safe division");
    assert_eq!(
        divide["inputSchema"]["properties"]["b"]["description"],
        "The divisor (non-zero)."
    );
    assert_eq!(divide["annotations"], json!({ "destructiveHint": false }));

    // `Json<T>` generates an output schema; serde/schemars attributes are forwarded.
    let stats = find(&tools, "stats");
    assert_eq!(stats["outputSchema"]["type"], "object");
    assert_eq!(
        stats["outputSchema"]["properties"]["mean"]["description"],
        "Arithmetic mean."
    );
    assert_eq!(stats["inputSchema"]["required"], json!(["values"]));
    assert_eq!(
        stats["inputSchema"]["properties"]["precision"]["maximum"],
        10
    );

    // `Ctx` and `State<T>` are not part of the schema; `Option` is optional.
    let greet = find(&tools, "greet");
    assert_eq!(
        greet["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["name"]
    );
    assert!(
        greet["inputSchema"]
            .get("required")
            .is_none_or(|r| r == &json!([]))
    );

    // No parameters: an object that accepts no properties.
    let ping = find(&tools, "ping");
    assert_eq!(ping["inputSchema"]["type"], "object");
    assert_eq!(ping["inputSchema"]["additionalProperties"], false);
}

#[tokio::test]
async fn calls() {
    let s = server();
    assert_eq!(
        call(&s, "add", json!({ "a": 1, "b": 2 })).await["content"][0]["text"],
        "3"
    );
    assert_eq!(
        call(&s, "ping", json!({})).await["content"][0]["text"],
        "pong"
    );

    let divided = call(&s, "safe-divide", json!({ "a": 1, "b": 0 })).await;
    assert_eq!(divided["isError"], true);
    assert_eq!(divided["content"][0]["text"], "b must not be zero");

    let stats = call(&s, "stats", json!({ "values": [1, 2, 4], "precision": 2 })).await;
    assert_eq!(
        stats["structuredContent"],
        json!({ "count": 3, "mean": 2.33 })
    );

    let greeting = call(&s, "greet", json!({})).await;
    assert_eq!(
        greeting["content"][0]["text"],
        "Hello, world! (call 1, client macro-client)"
    );
    let greeting = call(&s, "greet", json!({ "name": "Ada" })).await;
    assert_eq!(
        greeting["content"][0]["text"],
        "Hello, Ada! (call 2, client macro-client)"
    );
}

#[tokio::test]
async fn functions_stay_callable() {
    assert!((add(1.0, 2.0).await - 3.0).abs() < f64::EPSILON);
    assert_eq!(ping(), "pong");
    assert!(divide(1.0, 0.0).is_err());
}

#[test]
fn missing_state_fails_the_build() {
    let result = Server::builder("s", "1").tool::<greet>().build();
    assert!(matches!(
        result,
        Err(sdkmcprust::BuildError::MissingState { .. })
    ));
}

#[test]
fn tool_definition_is_available() {
    use sdkmcprust::Tool as _;
    let def = add::definition();
    let tool: &spec::Tool = def.tool();
    assert_eq!(tool.name, "add");
}
