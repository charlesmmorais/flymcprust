//! Compatibility with clients of the 2025-11-25 and 2025-06-18 protocol
//! versions ("legacy" clients), on top of the same stateless core.
//!
//! Legacy clients open with `initialize`, then send requests without the
//! per-request `_meta` of 2026-07-28, and expect the server to *ask* them for
//! input (elicitation, sampling, roots) with server-to-client requests. The
//! adapter:
//!
//! - answers `initialize` with the negotiated version and the server's
//!   capabilities, and remembers what the client declared in a
//!   [`LegacySession`] (per connection on stdio; on HTTP, in a signed
//!   `MCP-Session-Id` token, so the server stays stateless);
//! - runs each request through the normal dispatch, with the session
//!   standing in for `_meta`;
//! - turns multi round-trip results into server-to-client requests through
//!   a transport-provided [`Peer`], feeds the answers back, and reruns the
//!   handler, so handlers using `ctx.elicit` work unchanged;
//! - adapts results and errors to the older wire format.

// The transports use most of this module.
#![cfg_attr(not(all(feature = "stdio", feature = "http")), allow(dead_code))]
// Legacy clients still use logging, deprecated in 2026-07-28 (SEP-2577).
#![allow(deprecated)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::context::{RequestMeta, TransportContext};
use crate::error::McpError;
use crate::jsonrpc::{ClientResponse, Message, Request, Response};
use crate::mrtr::now_secs;
use crate::server::{McpRequest, Server};
use crate::spec::{self, RequestId};

/// Legacy protocol versions the server accepts, newest first.
pub const LEGACY_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18"];

/// Domain separation for session tokens.
const SESSION_CONTEXT: &[u8] = b"sdkmcprust/legacy-session/v1\0";

/// Upper bound on input round trips for one legacy request.
const MAX_INPUT_ROUNDS: usize = 16;

/// The 2025-11-25 code for "resource not found".
const LEGACY_RESOURCE_NOT_FOUND: i64 = -32002;

/// What a legacy client declared at `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LegacySession {
    /// Negotiated protocol version.
    #[serde(rename = "v")]
    pub(crate) version: String,
    /// Client capabilities, as sent.
    #[serde(rename = "c", default)]
    pub(crate) capabilities: Value,
    /// Client implementation info, as sent.
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_info: Option<Value>,
    /// Expiry of the session token (seconds since the Unix epoch).
    #[serde(rename = "e", default)]
    pub(crate) expires: u64,
    /// Random nonce: two clients declaring the same things at the same time
    /// still get different session ids.
    #[serde(rename = "n", default)]
    pub(crate) nonce: String,
    /// Minimum log level set with `logging/setLevel` (stdio only; HTTP
    /// sessions are immutable tokens). `None`: every level.
    #[allow(deprecated)]
    #[serde(rename = "l", default, skip_serializing_if = "Option::is_none")]
    pub(crate) log_level: Option<spec::LoggingLevel>,
}

impl LegacySession {
    /// A session for a request that carries a legacy version but no session
    /// (the client's capabilities are then unknown, so none are assumed).
    pub(crate) fn sessionless(version: &str) -> Self {
        Self {
            version: version.to_owned(),
            ..Self::default()
        }
    }

    /// The negotiated protocol version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Stands in for the per-request `_meta` of 2026-07-28.
    pub(crate) fn request_meta(&self) -> RequestMeta {
        RequestMeta {
            protocol_version: self.version.clone(),
            client_capabilities: serde_json::from_value(self.capabilities.clone())
                .unwrap_or_default(),
            client_info: self
                .client_info
                .clone()
                .and_then(|info| serde_json::from_value(info).ok()),
            progress_token: None,
            #[allow(deprecated)]
            log_level: Some(self.log_level.unwrap_or(spec::LoggingLevel::Debug)),
        }
    }
}

/// Whether a protocol version is one of the supported legacy versions.
#[must_use]
pub fn is_legacy_version(version: &str) -> bool {
    LEGACY_PROTOCOL_VERSIONS.contains(&version)
}

/// Sends server-to-client requests on behalf of the legacy adapter.
pub(crate) trait Peer: Send + Sync {
    /// Sends `method` with `params` to the client and waits for its result.
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>>;
}

/// Server-to-client requests waiting for the client's answer. Ids are
/// random (128 bits), so a client cannot guess another one's pending
/// request, and each entry remembers which connection or session asked.
#[derive(Debug, Default)]
pub(crate) struct PendingRequests {
    map: Mutex<HashMap<String, Pending>>,
}

#[derive(Debug)]
struct Pending {
    owner: u64,
    reply: oneshot::Sender<Result<Value, Value>>,
}

impl PendingRequests {
    fn register(&self, owner: u64) -> Option<(String, oneshot::Receiver<Result<Value, Value>>)> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).ok()?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let id = format!("sdkmcprust-{id}");
        let (reply, receiver) = oneshot::channel();
        self.lock().insert(id.clone(), Pending { owner, reply });
        Some((id, receiver))
    }

    fn forget(&self, id: &str) {
        self.lock().remove(id);
    }

    /// Delivers the client's answer. `false` if no request of this owner has
    /// that id.
    pub(crate) fn complete(&self, response: ClientResponse, owner: u64) -> bool {
        let RequestId::String(id) = &response.id else {
            return false;
        };
        let mut map = self.lock();
        match map.get(id) {
            Some(pending) if pending.owner == owner => {}
            _ => return false,
        }
        map.remove(id)
            .is_some_and(|pending| pending.reply.send(response.outcome).is_ok())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A [`Peer`] that writes requests to a channel (the stdio writer, or an
/// HTTP response stream) and waits for answers routed by the transport.
#[derive(Debug, Clone)]
pub(crate) struct ChannelPeer {
    pub(crate) pending: Arc<PendingRequests>,
    pub(crate) owner: u64,
    pub(crate) out: mpsc::Sender<Vec<u8>>,
    pub(crate) timeout: Duration,
}

impl Peer for ChannelPeer {
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let (id, receiver) = self
                .pending
                .register(self.owner)
                .ok_or_else(McpError::internal)?;
            let message = serde_json::json!({
                "jsonrpc": "2.0", "id": id, "method": method, "params": params,
            });
            let bytes = serde_json::to_vec(&message).map_err(|_| McpError::internal())?;
            if self.out.send(bytes).await.is_err() {
                self.pending.forget(&id);
                return Err(McpError::internal_with_message(
                    "The connection to the client is closed.",
                ));
            }
            match tokio::time::timeout(self.timeout, receiver).await {
                Ok(Ok(Ok(result))) => Ok(result),
                Ok(Ok(Err(_error))) => Err(McpError::request_failed(format!(
                    "The client returned an error for `{method}`."
                ))),
                Ok(Err(_)) | Err(_) => {
                    self.pending.forget(&id);
                    tracing::warn!(
                        "a client did not answer a `{method}` request in time
  = help: the                          wait is bounded by ServerBuilder::tool_timeout(..)"
                    );
                    Err(McpError::internal_with_message(
                        "The client did not answer in time.",
                    ))
                }
            }
        })
    }
}

/// The legacy-protocol data a server needs, computed at build time.
#[derive(Debug)]
pub(crate) struct LegacyConfig {
    pub(crate) enabled: bool,
    pub(crate) session_ttl: Duration,
    pub(crate) capabilities: Value,
    pub(crate) server_info: Value,
    pub(crate) instructions: Option<String>,
}

impl LegacyConfig {
    pub(crate) fn new(
        enabled: bool,
        session_ttl: Duration,
        capabilities: &spec::ServerCapabilities,
        server_info: &spec::Implementation,
        instructions: Option<&str>,
    ) -> Self {
        let mut capabilities = serde_json::to_value(capabilities).unwrap_or_default();
        if let Value::Object(map) = &mut capabilities {
            // Extensions are a 2026-07-28 concept.
            map.remove("extensions");
        }
        Self {
            enabled,
            session_ttl,
            capabilities,
            server_info: serde_json::to_value(server_info).unwrap_or_default(),
            instructions: instructions.map(str::to_owned),
        }
    }
}

#[derive(Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    #[serde(default)]
    capabilities: Value,
    #[serde(rename = "clientInfo", default)]
    client_info: Option<Value>,
}

fn to_raw(value: &Value) -> Result<Box<RawValue>, McpError> {
    serde_json::value::to_raw_value(value).map_err(|_| McpError::internal())
}

impl Server {
    /// Whether legacy (2025-11-25, 2025-06-18) clients are served.
    #[must_use]
    pub fn legacy_clients_enabled(&self) -> bool {
        self.legacy().enabled
    }

    /// Answers `initialize`: negotiates the version and returns the
    /// `InitializeResult` plus the session to remember.
    pub(crate) fn legacy_initialize(
        &self,
        params: Option<&RawValue>,
    ) -> Result<(Box<RawValue>, LegacySession), McpError> {
        let config = self.legacy();
        let params: InitializeParams = serde_json::from_str(params.map_or("{}", RawValue::get))
            .map_err(|e| {
                McpError::invalid_params(format!(
                    "Invalid initialize params: {}",
                    crate::error::sanitize(&e.to_string())
                ))
            })?;
        let version = if is_legacy_version(&params.protocol_version) {
            params.protocol_version
        } else {
            // The client decides whether it can work with our newest version.
            LEGACY_PROTOCOL_VERSIONS
                .first()
                .copied()
                .unwrap_or_default()
                .to_owned()
        };
        let mut result = Map::new();
        result.insert("protocolVersion".to_owned(), Value::from(version.clone()));
        result.insert("capabilities".to_owned(), config.capabilities.clone());
        result.insert("serverInfo".to_owned(), config.server_info.clone());
        if let Some(instructions) = &config.instructions {
            result.insert("instructions".to_owned(), Value::from(instructions.clone()));
        }
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| McpError::internal())?;
        let session = LegacySession {
            version,
            capabilities: params.capabilities,
            client_info: params.client_info,
            expires: now_secs().saturating_add(config.session_ttl.as_secs()),
            nonce: nonce.iter().map(|b| format!("{b:02x}")).collect(),
            log_level: None,
        };
        Ok((to_raw(&Value::Object(result))?, session))
    }

    /// Signs a session into an `MCP-Session-Id` value.
    pub(crate) fn seal_session(&self, session: &LegacySession) -> Option<String> {
        self.state_codec().seal_for(SESSION_CONTEXT, session)
    }

    /// Verifies an `MCP-Session-Id` value; `None` if invalid or expired.
    pub(crate) fn open_session(&self, token: &str) -> Option<LegacySession> {
        self.state_codec()
            .open_for::<LegacySession>(SESSION_CONTEXT, token)
            .filter(|s| s.expires >= now_secs() && is_legacy_version(&s.version))
    }

    /// Handles one request from a legacy client. Input requests of multi
    /// round-trip handlers are sent to the client through `peer`.
    pub(crate) async fn handle_legacy(
        &self,
        request: Request,
        session: &LegacySession,
        mut transport: TransportContext,
        cancellation: CancellationToken,
        peer: &dyn Peer,
    ) -> Option<Response> {
        let Request { id, method, params } = request;
        match method.as_str() {
            "initialize" => {
                return Some(match self.legacy_initialize(params.as_deref()) {
                    Ok((result, _)) => Response::Result { id, result },
                    Err(error) => Response::error(Some(id), error),
                });
            }
            // Legacy log level: kept per connection on stdio (which answers it
            // before reaching here); HTTP sessions cannot change, so the
            // request is acknowledged and every level stays enabled.
            "logging/setLevel" | "ping" => {
                return Some(match to_raw(&Value::Object(Map::new())) {
                    Ok(result) => Response::Result { id, result },
                    Err(error) => Response::error(Some(id), error),
                });
            }
            _ => {}
        }
        transport.insert(session.clone());

        let mut params = match params
            .as_deref()
            .map(|p| serde_json::from_str::<Value>(p.get()))
        {
            None => Map::new(),
            Some(Ok(Value::Object(map))) => map,
            Some(Ok(Value::Null)) => Map::new(),
            Some(_) => {
                return Some(Response::error(
                    Some(id),
                    McpError::invalid_params("params must be an object"),
                ));
            }
        };
        for _ in 0..MAX_INPUT_ROUNDS {
            let raw = match to_raw(&Value::Object(params.clone())) {
                Ok(raw) => raw,
                Err(error) => return Some(Response::error(Some(id), error)),
            };
            let response = self
                .handle(McpRequest {
                    message: Message::Request(Request {
                        id: id.clone(),
                        method: method.clone(),
                        params: Some(raw),
                    }),
                    transport: transport.clone(),
                    cancellation: cancellation.clone(),
                })
                .await?;
            let (id, result) = match response {
                Response::Error { id, error } => {
                    return Some(Response::Error {
                        id,
                        error: legacy_error(error),
                    });
                }
                Response::Result { id, result } => (id, result),
            };
            let Ok(Value::Object(mut result)) = serde_json::from_str::<Value>(result.get()) else {
                return Some(Response::error(Some(id), McpError::internal()));
            };
            if result.get("resultType").and_then(Value::as_str) != Some("input_required") {
                for key in ["resultType", "ttlMs", "cacheScope"] {
                    result.remove(key);
                }
                return Some(match to_raw(&Value::Object(result)) {
                    Ok(result) => Response::Result { id, result },
                    Err(error) => Response::error(Some(id), error),
                });
            }

            // Ask the client, then rerun the handler with the answers.
            let mut answers = Map::new();
            if let Some(Value::Object(requests)) = result.remove("inputRequests") {
                for (key, request) in requests {
                    let method = request
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let empty = Value::Object(Map::new());
                    let request_params = request.get("params").unwrap_or(&empty);
                    let answer = tokio::select! {
                        biased;
                        () = cancellation.cancelled() => return None,
                        answer = peer.request(method, request_params) => answer,
                    };
                    match answer {
                        Ok(answer) => {
                            answers.insert(key, answer);
                        }
                        Err(error) => return Some(Response::error(Some(id), error)),
                    }
                }
            }
            params.insert("inputResponses".to_owned(), Value::Object(answers));
            match result.remove("requestState") {
                Some(state) => params.insert("requestState".to_owned(), state),
                None => params.remove("requestState"),
            };
        }
        tracing::warn!(
            "a legacy request needed more than {MAX_INPUT_ROUNDS} input rounds\n  = help: a \
             handler keeps asking for input; check that each `ctx.elicit` key is answered once"
        );
        Some(Response::error(
            Some(id),
            McpError::internal_with_message("Too many input rounds."),
        ))
    }
}

/// Adapts an error to the 2025-11-25 wire format.
fn legacy_error(error: McpError) -> McpError {
    let resource_not_found = error.code() == spec::error_codes::INVALID_PARAMS_ERROR
        && error.message().starts_with("Resource not found")
        && error.data().is_some_and(|d| d.get("uri").is_some());
    if resource_not_found {
        error.with_code(LEGACY_RESOURCE_NOT_FOUND)
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_legacy_version("2025-11-25"));
        assert!(is_legacy_version("2025-06-18"));
        assert!(!is_legacy_version("2025-03-26"));
        assert!(!is_legacy_version("2026-07-28"));
    }

    #[test]
    fn resource_not_found_uses_the_legacy_code() {
        let e = legacy_error(McpError::resource_not_found("x://y"));
        assert_eq!(e.code(), -32002);
        assert_eq!(legacy_error(McpError::invalid_params("bad")).code(), -32602);
    }
}
