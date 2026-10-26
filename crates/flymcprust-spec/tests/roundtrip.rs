//! Round-trip tests using the JSON examples published in the 2026-07-28
//! specification pages (server/tools, server/discover, basic/index,
//! basic/versioning, basic/patterns/mrtr, server/utilities/caching).
//!
//! Each example is deserialized into its generated type and serialized back;
//! the result must be semantically identical to the original JSON.

// Integration tests may panic on failure.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use flymcprust_spec::v2026_07_28 as spec;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) -> T {
    let parsed: T = serde_json::from_value(value.clone()).expect("deserialize");
    let back = serde_json::to_value(&parsed).expect("serialize");
    assert_eq!(back, value, "round-trip changed the JSON");
    parsed
}

#[test]
fn discover_result() {
    let result: spec::DiscoverResult = roundtrip(json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": { "tools": {}, "resources": {} },
        "_meta": {
            "io.modelcontextprotocol/serverInfo": { "name": "ExampleServer", "version": "1.0.0" }
        },
        "instructions": "This server provides weather and resource utilities.",
        "ttlMs": 3600000,
        "cacheScope": "public"
    }));
    assert_eq!(result.cache_scope, spec::CacheScope::Public);
    assert_eq!(result.ttl_ms, 3_600_000);
}

#[test]
fn discover_request_params_meta() {
    let params: spec::RequestParams = roundtrip(json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "ExampleClient", "version": "1.0.0" },
            "io.modelcontextprotocol/clientCapabilities": {},
            "traceparent": "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01"
        }
    }));
    assert_eq!(params.meta.protocol_version, "2026-07-28");
    // Unknown `_meta` keys survive in `extra`.
    assert!(params.meta.extra.contains_key("traceparent"));
}

#[test]
fn list_tools_result() {
    let result: spec::ListToolsResult = roundtrip(json!({
        "resultType": "complete",
        "tools": [{
            "name": "get_weather",
            "title": "Weather Information Provider",
            "description": "Get current weather information for a location",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "location": { "type": "string", "description": "City name or zip code" }
                },
                "required": ["location"]
            },
            "icons": [{
                "src": "https://example.com/weather-icon.png",
                "mimeType": "image/png",
                "sizes": ["48x48"]
            }]
        }],
        "nextCursor": "next-page-cursor",
        "ttlMs": 300000,
        "cacheScope": "public"
    }));
    assert_eq!(result.tools.len(), 1);
}

#[test]
fn tool_with_output_schema_and_annotations() {
    roundtrip::<spec::Tool>(json!({
        "name": "get_weather_data",
        "title": "Weather Data Retriever",
        "description": "Get current weather data for a location",
        "inputSchema": {
            "type": "object",
            "properties": { "location": { "type": "string" } },
            "required": ["location"]
        },
        "outputSchema": {
            "type": "array",
            "items": { "type": "object" }
        },
        "annotations": { "readOnlyHint": true, "idempotentHint": true }
    }));
    // The no-parameter form recommended by the spec.
    roundtrip::<spec::Tool>(json!({
        "name": "get_current_time",
        "description": "Returns the current server time",
        "inputSchema": { "type": "object", "additionalProperties": false }
    }));
}

#[test]
fn call_tool_request_params() {
    let params: spec::CallToolRequestParams = roundtrip(json!({
        "name": "get_weather",
        "arguments": { "location": "Seattle, WA" },
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "ExampleClient", "version": "1.0.0" },
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    }));
    assert_eq!(params.name, "get_weather");
}

#[test]
fn call_tool_results() {
    let text: spec::CallToolResult = roundtrip(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "Current weather in New York: 72°F" }],
        "isError": false
    }));
    assert!(matches!(
        text.content.first(),
        Some(spec::ContentBlock::TextContent(_))
    ));

    // Structured content may be any JSON value (SEP-2106).
    roundtrip::<spec::CallToolResult>(json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": "Found 2 users." }],
        "structuredContent": [
            { "id": "1", "name": "Alice", "email": "alice@example.com" },
            { "id": "2", "name": "Bob", "email": "bob@example.com" }
        ]
    }));

    let mixed: spec::CallToolResult = roundtrip(json!({
        "resultType": "complete",
        "content": [
            {
                "type": "image",
                "data": "base64-encoded-data",
                "mimeType": "image/png",
                "annotations": { "audience": ["user"], "priority": 0.9 }
            },
            { "type": "audio", "data": "base64-encoded-audio-data", "mimeType": "audio/wav" },
            {
                "type": "resource_link",
                "uri": "file:///project/src/main.rs",
                "name": "main.rs",
                "description": "Primary application entry point",
                "mimeType": "text/x-rust"
            },
            {
                "type": "resource",
                "resource": {
                    "uri": "file:///project/src/main.rs",
                    "mimeType": "text/x-rust",
                    "text": "fn main() {}"
                }
            }
        ],
        "isError": true
    }));
    assert_eq!(mixed.content.len(), 4);
    assert!(matches!(
        mixed.content.get(3),
        Some(spec::ContentBlock::EmbeddedResource(_))
    ));
}

#[test]
fn input_required_vs_complete_result() {
    let interim: spec::CallToolResultResponseResult = roundtrip(json!({
        "resultType": "input_required",
        "inputRequests": {
            "github_login": {
                "method": "elicitation/create",
                "params": {
                    "mode": "form",
                    "message": "Please provide your GitHub username",
                    "requestedSchema": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }
                }
            }
        },
        "requestState": "eyJsb2NhdGlvbiI6Ik5ldyBZb3JrIn0..."
    }));
    assert!(matches!(
        interim,
        spec::CallToolResultResponseResult::InputRequired(_)
    ));

    let done: spec::CallToolResultResponseResult = roundtrip(json!({
        "resultType": "complete",
        "content": []
    }));
    assert!(matches!(
        done,
        spec::CallToolResultResponseResult::CallTool(_)
    ));
}

#[test]
fn error_responses() {
    let unsupported: spec::UnsupportedProtocolVersionError = roundtrip(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32022,
            "message": "Unsupported protocol version",
            "data": { "supported": ["2026-07-28", "2025-11-25"], "requested": "1900-01-01" }
        }
    }));
    assert_eq!(
        unsupported.error.code,
        spec::error_codes::UNSUPPORTED_PROTOCOL_VERSION_ERROR
    );

    roundtrip::<spec::HeaderMismatchError>(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32020,
            "message": "Header mismatch: Mcp-Name header value 'foo' does not match body value 'bar'"
        }
    }));

    roundtrip::<spec::JsonRpcErrorResponse>(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "error": { "code": -32602, "message": "Unknown tool: invalid_tool_name" }
    }));
}

#[test]
fn client_request_is_tagged_by_method() {
    let request: spec::ClientRequest = roundtrip(json!({
        "jsonrpc": "2.0",
        "id": "discover-1",
        "method": "server/discover",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    }));
    assert!(matches!(request, spec::ClientRequest::Discover(_)));

    let err = serde_json::from_value::<spec::ClientRequest>(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }))
    .unwrap_err();
    assert!(err.to_string().contains("unknown variant"));
}

#[test]
fn request_ids() {
    assert_eq!(
        serde_json::from_value::<spec::RequestId>(json!("abc")).unwrap(),
        spec::RequestId::String("abc".into())
    );
    assert_eq!(
        serde_json::from_value::<spec::RequestId>(json!(7)).unwrap(),
        spec::RequestId::Integer(7)
    );
    // Integral floats are read as integers.
    assert_eq!(
        serde_json::from_value::<spec::RequestId>(json!(7.0)).unwrap(),
        spec::RequestId::Integer(7)
    );
    assert!(serde_json::from_value::<spec::RequestId>(json!(7.5)).is_err());
    assert!(serde_json::from_value::<spec::RequestId>(json!(null)).is_err());
}

#[test]
fn constants_and_markers() {
    assert_eq!(spec::PROTOCOL_VERSION, "2026-07-28");
    assert_eq!(spec::methods::TOOLS_CALL, "tools/call");
    assert_eq!(spec::methods::SERVER_DISCOVER, "server/discover");
    assert_eq!(spec::JsonRpcVersion::VALUE, "2.0");
    assert!(serde_json::from_value::<spec::JsonRpcVersion>(json!("1.0")).is_err());
    assert_eq!(spec::error_codes::HEADER_MISMATCH_ERROR, -32020);
    assert_eq!(
        spec::error_codes::MISSING_REQUIRED_CLIENT_CAPABILITY_ERROR,
        -32021
    );
}

#[test]
#[allow(deprecated)]
fn deprecated_items_still_work() {
    // Logging is deprecated (SEP-2577) but must keep working.
    let level: spec::LoggingLevel = serde_json::from_value(json!("warning")).unwrap();
    assert_eq!(level.as_str(), "warning");
}
