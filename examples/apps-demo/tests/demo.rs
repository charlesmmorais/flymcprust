//! The demo end to end through the in-process test client: the interface
//! resource, the tool that shows it, and the app-only tool behind its
//! "Recompute" button.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use flymcprust::apps;
use flymcprust::testing::TestClient;
use serde_json::{Value, json};

fn client() -> TestClient {
    TestClient::new(apps_demo::server().unwrap())
}

#[tokio::test]
async fn the_interface_is_served_as_an_mcp_app() {
    let read = client()
        .read_resource("ui://apps-demo/price-table")
        .await
        .unwrap();
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(contents["mimeType"], apps::MIME_TYPE);
    assert_eq!(
        contents["_meta"]["ui"]["csp"]["resourceDomains"],
        json!(["https://cdn.jsdelivr.net"])
    );
    let html = contents["text"].as_str().unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("callServerTool"));
}

#[tokio::test]
async fn the_survey_opens_the_table_and_the_button_recomputes() {
    let client = client();
    let tools = client.list_tools().await.unwrap().tools;
    let survey = tools.iter().find(|t| t.name == "price_survey").unwrap();
    assert_eq!(
        Value::Object(survey.meta.clone().unwrap())["ui"]["resourceUri"],
        "ui://apps-demo/price-table"
    );

    let result = client
        .call_tool("price_survey", json!({ "item": "A4 paper, 500 sheets" }))
        .await
        .unwrap();
    let data = result.structured_content.unwrap();
    assert_eq!(data["quotes"].as_array().unwrap().len(), 5);

    let result = client
        .call_tool(
            "reference_price",
            json!({ "prices": [30.42, 32.45, 34.48, 38.53] }),
        )
        .await
        .unwrap();
    let reference = result.structured_content.unwrap();
    assert_eq!(reference["median"], 33.465);
    assert_eq!(reference["count"], 4);

    let empty = client
        .call_tool("reference_price", json!({ "prices": [] }))
        .await
        .unwrap();
    assert_eq!(empty.is_error, Some(true));
}
