//! MCP Apps (`io.modelcontextprotocol/ui`) end to end: tool and resource
//! metadata, read results, the declared extension and build-time checks.
#![cfg(feature = "test-client")]
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mcpsdkrust::prelude::*;
use mcpsdkrust::testing::TestClient;
use mcpsdkrust::{AppUi, ResourceDef, ToolDef, Visibility, apps, spec};
use serde_json::{Value, json};

const PAGE: &str = "<!doctype html><html><body><div id=app></div></body></html>";

/// The price survey of a procurement study, as an editable table.
#[resource(
    "ui://etp/pesquisa-precos",
    app,
    csp_connect = ["https://api.example.gov.br"],
    csp_resources = ["https://*.cdn.example.com"],
    permissions = ["clipboardWrite"],
    prefers_border = true
)]
fn price_table() -> String {
    PAGE.to_owned()
}

/// Shows the price survey of an item.
///
/// # Arguments
///
/// * `item` - Catalog code of the item.
#[tool(read_only, ui = "ui://etp/pesquisa-precos")]
fn price_survey(ctx: Ctx, item: String) -> String {
    if ctx.client_supports_apps() {
        format!("Price survey of {item}, shown in the table.")
    } else {
        format!("Price survey of {item}: median R$ 10,00 (5 sources).")
    }
}

/// Recomputes the reference price (called by the table, not the model).
#[tool(visibility = ["app"])]
fn recompute(sources: Vec<f64>) -> f64 {
    sources.iter().sum::<f64>() / sources.len().max(1) as f64
}

fn server() -> Server {
    Server::builder("etp", "1.0.0")
        .resource::<price_table>()
        .tool::<price_survey>()
        .tool::<recompute>()
        .build()
        .unwrap()
}

fn find<'a>(tools: &'a [spec::Tool], name: &str) -> &'a spec::Tool {
    tools.iter().find(|t| t.name == name).unwrap()
}

#[tokio::test]
async fn tools_and_resources_carry_the_interface() {
    let client = TestClient::new(server());

    let discover = client.discover().await.unwrap();
    let extensions = discover.capabilities.extensions.unwrap();
    assert!(extensions[apps::EXTENSION_ID].is_empty());

    let tools = client.list_tools().await.unwrap().tools;
    let survey = Value::Object(find(&tools, "price_survey").meta.clone().unwrap());
    assert_eq!(
        survey,
        json!({
            "ui": { "resourceUri": "ui://etp/pesquisa-precos" },
            "ui/resourceUri": "ui://etp/pesquisa-precos",
        })
    );
    let recompute = Value::Object(find(&tools, "recompute").meta.clone().unwrap());
    assert_eq!(recompute, json!({ "ui": { "visibility": ["app"] } }));

    let resources = client.list_resources().await.unwrap().resources;
    let table = &resources[0];
    assert_eq!(table.mime_type.as_deref(), Some(apps::MIME_TYPE));
    let ui = json!({
        "csp": {
            "connectDomains": ["https://api.example.gov.br"],
            "resourceDomains": ["https://*.cdn.example.com"],
        },
        "permissions": { "clipboardWrite": {} },
        "prefersBorder": true,
    });
    assert_eq!(table.meta.as_ref().unwrap()["ui"], ui);

    let read = client
        .read_resource("ui://etp/pesquisa-precos")
        .await
        .unwrap();
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(contents["mimeType"], apps::MIME_TYPE);
    assert_eq!(contents["_meta"]["ui"], ui);
    assert_eq!(contents["text"], PAGE);
}

#[tokio::test]
async fn tools_know_whether_the_host_renders_apps() {
    let plain = TestClient::new(server());
    let result = plain
        .call_tool("price_survey", json!({ "item": "123" }))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&result.content)
            .unwrap()
            .contains("median")
    );

    let host = TestClient::new(server()).with_meta(json!({
        "io.modelcontextprotocol/protocolVersion": spec::PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": { "extensions": {
            "io.modelcontextprotocol/ui": { "mimeTypes": ["text/html;profile=mcp-app"] }
        } },
    }));
    let result = host
        .call_tool("price_survey", json!({ "item": "123" }))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&result.content)
            .unwrap()
            .contains("table")
    );
}

#[test]
fn a_tool_must_name_a_registered_interface() {
    let error = Server::builder("s", "1")
        .tool::<price_survey>()
        .build()
        .unwrap_err();
    assert!(format!("{error:?}").contains("no resource with this URI is marked as an app"));

    // A plain resource at that URI is not an interface.
    let plain = ResourceDef::new(
        spec::Resource::new("t".to_owned(), "ui://etp/pesquisa-precos".to_owned()),
        |_, _, _| Ok(Box::pin(async { Ok(Vec::new()) })),
    );
    let error = Server::builder("s", "1")
        .resource_def(plain)
        .tool::<price_survey>()
        .build()
        .unwrap_err();
    assert!(format!("{error:?}").contains("marked as an app"));
}

#[test]
fn builders_validate_interfaces() {
    fn read(
        _: &str,
        _: &mcpsdkrust::UriVars,
        _: Ctx,
    ) -> Result<mcpsdkrust::__private::ResourceFuture, ToolError> {
        Ok(Box::pin(async { Ok(Vec::new()) }))
    }
    let resource = |uri: &str, ui: AppUi| {
        ResourceDef::new(spec::Resource::new("v".to_owned(), uri.to_owned()), read).app(ui)
    };
    let build = |def: ResourceDef| Server::builder("s", "1").resource_def(def).build();

    assert!(build(resource("https://example.com/view", AppUi::new())).is_err());
    assert!(
        build(resource(
            "ui://x/view",
            AppUi::new().connect_domains(["https://a.example.com; script-src *"])
        ))
        .is_err()
    );
    assert!(build(resource("ui://x/view", AppUi::new())).is_ok());

    let tool = ToolDef::new(
        spec::Tool::new(
            mcpsdkrust::__private::input_schema_for::<NoArgs>(),
            "t".to_owned(),
        ),
        |_, _| {
            Ok(Box::pin(async {
                mcpsdkrust::IntoToolResult::into_tool_result("x")
            }))
        },
    )
    .visibility([]);
    assert!(Server::builder("s", "1").tool_def(tool).build().is_err());
    let _ = Visibility::Model;
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NoArgs {}
