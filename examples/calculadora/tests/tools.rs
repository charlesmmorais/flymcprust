//! The calculator through the in-process test client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sdkmcprust::spec::{CallToolResult, ContentBlock};
use sdkmcprust::testing::{ClientError, TestClient};
use serde_json::json;

fn client() -> TestClient {
    TestClient::new(calculadora::server().unwrap())
}

fn text(result: &CallToolResult) -> &str {
    match result.content.first() {
        Some(ContentBlock::TextContent(t)) => &t.text,
        other => panic!("expected text content, got {other:?}"),
    }
}

#[tokio::test]
async fn discover_and_list() {
    let client = client();
    let discover = client.discover().await.unwrap();
    assert_eq!(discover.supported_versions, ["2026-07-28"]);
    assert!(discover.capabilities.tools.is_some());

    let tools = client.list_tools().await.unwrap().tools;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["add", "divide", "multiply", "statistics", "subtract"]
    );
    for tool in &tools {
        assert!(
            tool.description.is_some(),
            "{} has no description",
            tool.name
        );
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }
    let statistics = tools.iter().find(|t| t.name == "statistics").unwrap();
    assert!(statistics.output_schema.is_some());
}

#[tokio::test]
async fn arithmetic() {
    let client = client();
    let cases = [
        ("add", 6.0, 3.0, "9"),
        ("subtract", 6.0, 3.0, "3"),
        ("multiply", 6.0, 3.0, "18"),
        ("divide", 6.0, 3.0, "2"),
        ("divide", 1.0, 4.0, "0.25"),
    ];
    for (tool, a, b, expected) in cases {
        let result = client
            .call_tool(tool, json!({ "a": a, "b": b }))
            .await
            .unwrap();
        assert_eq!(result.is_error, None, "{tool}");
        assert_eq!(text(&result), expected, "{tool}({a}, {b})");
    }
}

#[tokio::test]
async fn errors_are_reported_to_the_model() {
    let client = client();
    let result = client
        .call_tool("divide", json!({ "a": 1, "b": 0 }))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("division by zero"));

    let overflow = client
        .call_tool("multiply", json!({ "a": 1e308, "b": 10 }))
        .await
        .unwrap();
    assert_eq!(overflow.is_error, Some(true));

    let invalid = client
        .call_tool("add", json!({ "a": "one", "b": 2 }))
        .await
        .unwrap();
    assert_eq!(invalid.is_error, Some(true));
    assert!(text(&invalid).starts_with("Invalid arguments"));

    let unknown = client
        .call_tool("sqrt", json!({ "x": 4 }))
        .await
        .unwrap_err();
    assert!(matches!(unknown, ClientError::Rpc { code: -32602, .. }));
}

#[tokio::test]
async fn statistics_returns_structured_content() {
    let result = client()
        .call_tool("statistics", json!({ "values": [3, 1, 2] }))
        .await
        .unwrap();
    assert_eq!(
        result.structured_content,
        Some(json!({ "count": 3, "sum": 6.0, "mean": 2.0, "min": 1.0, "max": 3.0 }))
    );
    let empty = client()
        .call_tool("statistics", json!({ "values": [] }))
        .await
        .unwrap();
    assert_eq!(empty.is_error, Some(true));
}
