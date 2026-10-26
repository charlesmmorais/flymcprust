//! JSON-RPC 2.0 envelope parsing and response serialization.
//!
//! Parsing enforces the size and depth limits before `serde` sees the input,
//! rejects batches (not part of MCP 2026-07-28), null ids and responses, and
//! keeps `params` as raw JSON so each handler deserializes exactly once.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::error::McpError;
use crate::limits::Limits;
use crate::spec::{JsonRpcVersion, RequestId};

/// An incoming JSON-RPC message accepted by the server.
#[derive(Debug)]
pub enum Message {
    /// A request, which expects a response.
    Request(Request),
    /// A notification, which must not be answered.
    Notification(Notification),
}

/// A JSON-RPC request.
#[derive(Debug)]
pub struct Request {
    /// Request id (never null).
    pub id: RequestId,
    /// Method name.
    pub method: String,
    /// Raw `params`, if present and not null.
    pub params: Option<Box<RawValue>>,
}

/// A JSON-RPC notification.
#[derive(Debug)]
pub struct Notification {
    /// Method name.
    pub method: String,
    /// Raw `params`, if present and not null.
    pub params: Option<Box<RawValue>>,
}

/// Why an incoming message was rejected.
#[derive(Debug)]
pub enum Rejection {
    /// The message is invalid; answer with this error.
    Error {
        /// The request id, when it could be read.
        id: Option<RequestId>,
        /// The error to send.
        error: McpError,
    },
    /// The message is a JSON-RPC response, which clients must not send.
    /// Transports drop it (stdio) or answer with an HTTP error (HTTP).
    UnexpectedResponse,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow, default)]
    jsonrpc: Option<Cow<'a, str>>,
    // `deserialize_with` keeps an explicit `null` (rejected later) distinct
    // from an absent id (a notification).
    #[serde(borrow, default, deserialize_with = "present")]
    id: Option<&'a RawValue>,
    #[serde(borrow, default)]
    method: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
    #[serde(borrow, default)]
    result: Option<&'a RawValue>,
    #[serde(borrow, default)]
    error: Option<&'a RawValue>,
}

fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&'de RawValue>::deserialize(d).map(Some)
}

/// Parses one JSON-RPC message, enforcing `limits`.
///
/// # Errors
///
/// Returns a [`Rejection`] describing the response to send, if any.
pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Message, Rejection> {
    let reject = |error| Rejection::Error { id: None, error };

    if bytes.len() > limits.max_message_bytes {
        return Err(reject(McpError::invalid_request("Message too large")));
    }
    if exceeds_depth(bytes, limits.effective_json_depth()) {
        return Err(reject(McpError::invalid_request(
            "Message nested too deeply",
        )));
    }
    if first_significant_byte(bytes) == Some(b'[') {
        return Err(reject(McpError::invalid_request(
            "Batch requests are not supported",
        )));
    }

    let envelope: Envelope<'_> = serde_json::from_slice(bytes).map_err(|e| {
        reject(if e.is_data() {
            McpError::invalid_request("Invalid JSON-RPC message")
        } else {
            McpError::parse_error()
        })
    })?;

    let id = match envelope.id {
        None => None,
        Some(raw) if raw.get() == "null" => {
            return Err(reject(McpError::invalid_request(
                "Request id must not be null",
            )));
        }
        Some(raw) => Some(
            serde_json::from_str::<RequestId>(raw.get())
                .map_err(|_| reject(McpError::invalid_request("Invalid request id")))?,
        ),
    };
    let reject_with_id = |error| Rejection::Error {
        id: id.clone(),
        error,
    };

    if envelope.jsonrpc.as_deref() != Some(JsonRpcVersion::VALUE) {
        return Err(reject_with_id(McpError::invalid_request(
            "Unsupported JSON-RPC version",
        )));
    }
    if envelope.result.is_some() || envelope.error.is_some() {
        return Err(Rejection::UnexpectedResponse);
    }
    let Some(method) = envelope.method else {
        return Err(reject_with_id(McpError::invalid_request("Missing method")));
    };
    let method = method.into_owned();
    let params = envelope.params.map(RawValue::to_owned);

    Ok(match id {
        Some(id) => Message::Request(Request { id, method, params }),
        None => Message::Notification(Notification { method, params }),
    })
}

/// A client's answer to a request the server sent (legacy protocol).
#[cfg_attr(not(any(feature = "stdio", feature = "http")), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct ClientResponse {
    pub(crate) id: RequestId,
    /// `Ok(result)` or `Err(error object)`.
    pub(crate) outcome: Result<serde_json::Value, serde_json::Value>,
}

#[cfg_attr(not(any(feature = "stdio", feature = "http")), allow(dead_code))]
/// Parses a message that [`parse`] reported as
/// [`Rejection::UnexpectedResponse`] (size and depth were already checked).
pub(crate) fn parse_client_response(bytes: &[u8]) -> Option<ClientResponse> {
    #[derive(Deserialize)]
    struct Wire {
        id: RequestId,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<serde_json::Value>,
    }
    let wire: Wire = serde_json::from_slice(bytes).ok()?;
    let outcome = match (wire.result, wire.error) {
        (_, Some(error)) => Err(error),
        (Some(result), None) => Ok(result),
        (None, None) => return None,
    };
    Some(ClientResponse {
        id: wire.id,
        outcome,
    })
}

/// A JSON-RPC response produced by the server.
#[derive(Debug, Clone)]
pub enum Response {
    /// A successful result, already serialized.
    Result {
        /// The id of the request being answered.
        id: RequestId,
        /// The serialized `result` object.
        result: Box<RawValue>,
    },
    /// An error.
    Error {
        /// The id of the request being answered, when known.
        id: Option<RequestId>,
        /// The error.
        error: McpError,
    },
}

#[derive(Serialize)]
struct ResultWire<'a> {
    jsonrpc: JsonRpcVersion,
    id: &'a RequestId,
    result: &'a RawValue,
}

#[derive(Serialize)]
struct ErrorWire<'a> {
    jsonrpc: JsonRpcVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a RequestId>,
    error: ErrorObjectWire<'a>,
}

#[derive(Serialize)]
struct ErrorObjectWire<'a> {
    code: i64,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a serde_json::Value>,
}

impl Response {
    /// Builds an error response.
    #[must_use]
    pub fn error(id: Option<RequestId>, error: McpError) -> Self {
        Self::Error { id, error }
    }

    /// The JSON-RPC error, if this is an error response.
    #[must_use]
    pub fn as_error(&self) -> Option<&McpError> {
        match self {
            Self::Error { error, .. } => Some(error),
            Self::Result { .. } => None,
        }
    }

    /// Serializes the response as a single line of JSON (no trailing newline).
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        let serialized = match self {
            Self::Result { id, result } => serde_json::to_vec(&ResultWire {
                jsonrpc: JsonRpcVersion::default(),
                id,
                result,
            }),
            Self::Error { id, error } => serde_json::to_vec(&ErrorWire {
                jsonrpc: JsonRpcVersion::default(),
                id: id.as_ref(),
                error: ErrorObjectWire {
                    code: error.code(),
                    message: error.message(),
                    data: error.data(),
                },
            }),
        };
        // Serializing these plain structs cannot fail; fall back to a fixed,
        // valid error object just in case.
        serialized.unwrap_or_else(|_| {
            br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"}}"#.to_vec()
        })
    }
}

/// Returns `true` when `bytes` nests objects/arrays deeper than `max`.
///
/// A single pass that skips string contents, so brackets inside strings are
/// ignored. Strings are skipped with `memchr` (vectorized), which keeps the
/// scan cheap for large payloads. Runs before `serde` to bound recursion.
pub(crate) fn exceeds_depth(bytes: &[u8], max: usize) -> bool {
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        i += 1;
        match b {
            b'"' => {
                // Jump to the closing quote, stepping over escapes.
                loop {
                    let rest = bytes.get(i..).unwrap_or_default();
                    let Some(offset) = memchr::memchr2(b'"', b'\\', rest) else {
                        // Unterminated string: the parser rejects it.
                        return false;
                    };
                    let found = rest.get(offset).copied();
                    i += offset + 1;
                    if found == Some(b'\\') {
                        // Skip the escaped character.
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            b'{' | b'[' => {
                depth += 1;
                if depth > max {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn first_significant_byte(bytes: &[u8]) -> Option<u8> {
    bytes.iter().copied().find(|b| !b.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::default()
    }

    fn rejection_code(r: Result<Message, Rejection>) -> i64 {
        match r {
            Err(Rejection::Error { error, .. }) => error.code(),
            other => panic!("expected an error rejection, got {other:?}"),
        }
    }

    #[test]
    fn parses_request_and_notification() {
        let req = parse(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"a":1}}"#,
            &limits(),
        );
        match req {
            Ok(Message::Request(r)) => {
                assert_eq!(r.id, RequestId::Integer(1));
                assert_eq!(r.method, "tools/list");
                assert_eq!(
                    r.params.map(|p| p.get().to_owned()).as_deref(),
                    Some(r#"{"a":1}"#)
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        let note = parse(
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#,
            &limits(),
        );
        assert!(matches!(note, Ok(Message::Notification(_))));
    }

    #[test]
    fn rejects_invalid_messages() {
        assert_eq!(rejection_code(parse(b"{not json", &limits())), -32700);
        assert_eq!(rejection_code(parse(b"[]", &limits())), -32600);
        assert_eq!(
            rejection_code(parse(
                br#"{"jsonrpc":"2.0","id":null,"method":"x"}"#,
                &limits()
            )),
            -32600
        );
        assert_eq!(
            rejection_code(parse(
                br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#,
                &limits()
            )),
            -32600
        );
        assert_eq!(
            rejection_code(parse(br#"{"jsonrpc":"2.0","id":1}"#, &limits())),
            -32600
        );
        assert_eq!(
            rejection_code(parse(
                br#"{"jsonrpc":"2.0","id":1.5,"method":"x"}"#,
                &limits()
            )),
            -32600
        );
        assert!(matches!(
            parse(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#, &limits()),
            Err(Rejection::UnexpectedResponse)
        ));
    }

    #[test]
    fn enforces_size_and_depth() {
        let small = Limits {
            max_message_bytes: 10,
            ..Limits::default()
        };
        assert_eq!(
            rejection_code(parse(br#"{"jsonrpc":"2.0","method":"x"}"#, &small)),
            -32600
        );
        let deep = format!(
            r#"{{"jsonrpc":"2.0","method":"x","params":{}{}}}"#,
            "[".repeat(100),
            "]".repeat(100)
        );
        assert_eq!(rejection_code(parse(deep.as_bytes(), &limits())), -32600);
    }

    #[test]
    fn depth_scan_ignores_strings() {
        assert!(!exceeds_depth(br#"{"a":"[[[[[[[[[[\"[[["}"#, 2));
        assert!(exceeds_depth(br#"{"a":[[1]]}"#, 2));
        assert!(!exceeds_depth(br#"{"a":[1]}"#, 2));
        // An escaped backslash right before the closing quote ends the string.
        assert!(exceeds_depth(br#"{"a":"x\\","b":[[1]]}"#, 2));
        // An escaped quote does not end it.
        assert!(!exceeds_depth(br#"{"a":"\"[[[[","b":1}"#, 2));
        // Unterminated strings are left to the parser.
        assert!(!exceeds_depth(br#"{"a":"[[[["#, 2));
    }

    #[test]
    fn serializes_responses() {
        let ok = Response::Result {
            id: RequestId::String("a".into()),
            result: RawValue::from_string(r#"{"resultType":"complete"}"#.into()).unwrap(),
        };
        assert_eq!(
            String::from_utf8(ok.to_vec()).unwrap(),
            r#"{"jsonrpc":"2.0","id":"a","result":{"resultType":"complete"}}"#
        );
        let err = Response::error(None, McpError::parse_error());
        assert_eq!(
            String::from_utf8(err.to_vec()).unwrap(),
            r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"}}"#
        );
    }
}
