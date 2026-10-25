//! In-process test client (feature `test-client`).
//!
//! [`TestClient`] talks to a [`Server`] without any I/O, but through the
//! full serialization path: every request is encoded to bytes, parsed by the
//! server exactly as a transport would, and the response is decoded from
//! bytes. Tests therefore exercise the real wire format.
//!
//! ```
//! # use sdkmcprust_core::{Server, testing::TestClient};
//! # async fn demo(server: Server) -> Result<(), sdkmcprust_core::testing::ClientError> {
//! let client = TestClient::new(server);
//! let tools = client.list_tools().await?;
//! let result = client.call_tool("add", serde_json::json!({ "a": 1, "b": 2 })).await?;
//! # Ok(()) }
//! ```

use std::sync::atomic::{AtomicI64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::context::TransportContext;
use crate::server::Server;
use crate::spec::{self, methods};

/// An error returned by [`TestClient`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// The server answered with a JSON-RPC error.
    #[error("JSON-RPC error {code}: {message}")]
    Rpc {
        /// The error code.
        code: i64,
        /// The error message.
        message: String,
        /// The optional `data` member.
        data: Option<Value>,
    },
    /// The server sent no response (e.g. the request was a notification).
    #[error("the server sent no response")]
    NoResponse,
    /// The response could not be decoded.
    #[error("invalid response: {0}")]
    Decode(#[from] serde_json::Error),
}

/// A client that calls a [`Server`] in process.
#[derive(Debug)]
pub struct TestClient {
    server: Server,
    next_id: AtomicI64,
    meta: Value,
}

impl TestClient {
    /// Creates a client that declares no capabilities and the server's
    /// protocol version.
    #[must_use]
    pub fn new(server: Server) -> Self {
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": spec::PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": { "name": "sdkmcprust-test-client", "version": env!("CARGO_PKG_VERSION") },
        });
        Self {
            server,
            next_id: AtomicI64::new(1),
            meta,
        }
    }

    /// Replaces the `_meta` sent with every request.
    #[must_use]
    pub fn with_meta(mut self, meta: Value) -> Self {
        self.meta = meta;
        self
    }

    /// Calls `server/discover`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn discover(&self) -> Result<spec::DiscoverResult, ClientError> {
        self.request(methods::SERVER_DISCOVER, json!({})).await
    }

    /// Calls `tools/list`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn list_tools(&self) -> Result<spec::ListToolsResult, ClientError> {
        self.request(methods::TOOLS_LIST, json!({})).await
    }

    /// Calls `tools/call`. Tool failures are results with `is_error`, not
    /// errors.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<spec::CallToolResult, ClientError> {
        self.request(
            methods::TOOLS_CALL,
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    /// Calls `resources/list`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn list_resources(&self) -> Result<spec::ListResourcesResult, ClientError> {
        self.request(methods::RESOURCES_LIST, json!({})).await
    }

    /// Calls `resources/templates/list`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn list_resource_templates(
        &self,
    ) -> Result<spec::ListResourceTemplatesResult, ClientError> {
        self.request(methods::RESOURCES_TEMPLATES_LIST, json!({}))
            .await
    }

    /// Calls `resources/read`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors (including
    /// resource-not-found) or undecodable results.
    pub async fn read_resource(&self, uri: &str) -> Result<spec::ReadResourceResult, ClientError> {
        self.request(methods::RESOURCES_READ, json!({ "uri": uri }))
            .await
    }

    /// Calls `prompts/list`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn list_prompts(&self) -> Result<spec::ListPromptsResult, ClientError> {
        self.request(methods::PROMPTS_LIST, json!({})).await
    }

    /// Calls `prompts/get` with string arguments.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<spec::GetPromptResult, ClientError> {
        self.request(
            methods::PROMPTS_GET,
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    /// Calls `completion/complete` for a prompt argument.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn complete_prompt(
        &self,
        prompt: &str,
        argument: &str,
        value: &str,
    ) -> Result<spec::CompleteResult, ClientError> {
        self.request(
            methods::COMPLETION_COMPLETE,
            json!({
                "ref": { "type": "ref/prompt", "name": prompt },
                "argument": { "name": argument, "value": value },
            }),
        )
        .await
    }

    /// Calls `completion/complete` for a resource-template variable.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn complete_resource(
        &self,
        uri_template: &str,
        variable: &str,
        value: &str,
    ) -> Result<spec::CompleteResult, ClientError> {
        self.request(
            methods::COMPLETION_COMPLETE,
            json!({
                "ref": { "type": "ref/resource", "uri": uri_template },
                "argument": { "name": variable, "value": value },
            }),
        )
        .await
    }

    /// Sends any request; `_meta` is added to `params` unless present.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] for JSON-RPC errors or undecodable results.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        mut params: Value,
    ) -> Result<T, ClientError> {
        if let Value::Object(map) = &mut params {
            map.entry("_meta").or_insert_with(|| self.meta.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let bytes = serde_json::to_vec(&message)?;
        let response = self
            .server
            .handle_bytes(
                &bytes,
                TransportContext::default(),
                CancellationToken::new(),
            )
            .await
            .ok_or(ClientError::NoResponse)?;
        let mut envelope: Value = serde_json::from_slice(&response.to_vec())?;
        if let Some(error) = envelope.get_mut("error").map(Value::take) {
            let error: spec::Error = serde_json::from_value(error)?;
            return Err(ClientError::Rpc {
                code: error.code,
                message: error.message,
                data: error.data,
            });
        }
        let result = envelope
            .get_mut("result")
            .map(Value::take)
            .unwrap_or_default();
        Ok(serde_json::from_value(result)?)
    }
}
