//! A Streamable HTTP server exposing the fixtures required by the official
//! MCP conformance suite (`@modelcontextprotocol/conformance`), so the SDK
//! can be measured against it in CI.
//!
//! ```text
//! conformance-server [ADDR]      # default 127.0.0.1:3000, endpoint /mcp
//! ```
//!
//! Fixtures that need features the SDK does not have (resource
//! subscriptions) are listed in the conformance baselines.
#![forbid(unsafe_code)]
// A CLI fixture: status messages go to stderr.
#![allow(clippy::print_stderr)]

mod fixtures;

use std::process::ExitCode;

use mcpsdkrust::prelude::*;
use mcpsdkrust::spec::{
    self, AudioContent, ClientCapabilities, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, ImageContent, TextContent, TextResourceContents,
};
use mcpsdkrust::{IntoToolResult, ToolDef};

/// A 1×1 red PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
/// An empty 8 kHz mono 16-bit PCM WAV file.
const WAV: &str = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAAZGF0YQAAAAA=";

fn image() -> ContentBlock {
    ImageContent::new(PNG.to_owned(), "image/png".to_owned()).into()
}

fn text_resource(uri: &str, mime_type: &str, text: &str) -> ContentBlock {
    let mut contents = TextResourceContents::new(text.to_owned(), uri.to_owned());
    contents.mime_type = Some(mime_type.to_owned());
    EmbeddedResource::new(EmbeddedResourceResource::Text(contents)).into()
}

/// Returns a fixed text response.
#[tool]
fn test_simple_text() -> &'static str {
    "This is a simple text response for testing."
}

/// Returns a 1×1 PNG image.
#[tool]
fn test_image_content() -> ContentBlock {
    image()
}

/// Returns a WAV audio clip.
#[tool]
fn test_audio_content() -> ContentBlock {
    AudioContent::new(WAV.to_owned(), "audio/wav".to_owned()).into()
}

/// Returns an embedded text resource.
#[tool]
fn test_embedded_resource() -> ContentBlock {
    text_resource(
        "test://embedded-resource",
        "text/plain",
        "This is an embedded resource content.",
    )
}

/// Returns text, image and resource content together.
#[tool]
fn test_multiple_content_types() -> Vec<ContentBlock> {
    vec![
        TextContent::new("Multiple content types test:".to_owned()).into(),
        image(),
        text_resource(
            "test://mixed-content-resource",
            "application/json",
            r#"{"test":"data","value":123}"#,
        ),
    ]
}

/// Always fails with a tool execution error.
#[tool]
fn test_error_handling() -> Result<String, ToolError> {
    Err(ToolError::user(
        "This tool intentionally returns an error for testing",
    ))
}

/// Requires the client to declare the `sampling` capability.
#[tool]
#[allow(deprecated)] // Sampling is deprecated (SEP-2577) but still part of the spec.
fn test_missing_capability(ctx: Ctx) -> Result<String, ToolError> {
    if ctx.client_capabilities().sampling.is_none() {
        let required = ClientCapabilities {
            sampling: Some(spec::ClientCapabilitiesSampling::default()),
            ..ClientCapabilities::default()
        };
        return Err(ToolError::missing_client_capability(&required));
    }
    Ok("The client declared the sampling capability.".to_owned())
}

/// Echoes a region that is also mirrored into the `Mcp-Param-Region` header.
#[tool]
fn test_custom_header(
    /// The region to echo.
    #[schemars(extend("x-mcp-header" = "Region"))]
    region: String,
) -> String {
    format!("region: {region}")
}

/// A tool whose input schema uses JSON Schema 2020-12 features.
fn json_schema_tool() -> Result<ToolDef, serde_json::Error> {
    let input_schema: spec::ToolInputSchema = serde_json::from_value(serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "$defs": {
            "address": {
                "$anchor": "addressDef",
                "type": "object",
                "properties": { "street": { "type": "string" }, "city": { "type": "string" } }
            }
        },
        "properties": {
            "name": { "type": "string" },
            "address": { "$ref": "#/$defs/address" },
            "contactMethod": { "type": "string", "enum": ["phone", "email"] },
            "phone": { "type": "string" },
            "email": { "type": "string" }
        },
        "allOf": [{ "anyOf": [{ "required": ["phone"] }, { "required": ["email"] }] }],
        "if": { "properties": { "contactMethod": { "const": "phone" } }, "required": ["contactMethod"] },
        "then": { "required": ["phone"] },
        "else": { "required": ["email"] },
        "additionalProperties": false
    }))?;
    let mut tool = spec::Tool::new(input_schema, "json_schema_2020_12_tool".to_owned());
    tool.description = Some("Tool with JSON Schema 2020-12 features".to_owned());
    Ok(ToolDef::new(tool, |_, _| {
        Ok(Box::pin(async { "ok".into_tool_result() }))
    }))
}

#[tokio::main]
async fn main() -> ExitCode {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3000".to_owned());
    let server = json_schema_tool()
        .map_err(|e| e.to_string())
        .and_then(|schema_tool| {
            let builder = Server::builder("mcpsdkrust-conformance", env!("CARGO_PKG_VERSION"))
                .tool::<test_simple_text>()
                .tool::<test_image_content>()
                .tool::<test_audio_content>()
                .tool::<test_embedded_resource>()
                .tool::<test_multiple_content_types>()
                .tool::<test_error_handling>()
                .tool::<test_missing_capability>()
                .tool::<test_custom_header>()
                .tool_def(schema_tool);
            fixtures::register_notifications(fixtures::register_legacy(fixtures::register(builder)))
                .build()
                .map_err(|e| e.to_string())
        });
    let server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("invalid server: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("conformance server listening on http://{addr}/mcp");
    let result = tokio::select! {
        result = server.serve_http(addr.as_str()) => result,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("server error: {error}");
            ExitCode::FAILURE
        }
    }
}
