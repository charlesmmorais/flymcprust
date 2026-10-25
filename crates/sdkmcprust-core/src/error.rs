//! Protocol-level (JSON-RPC) errors.
//!
//! Messages are written for the client and never contain internal details.
//! Codes follow the 2026-07-28 allocation policy: standard JSON-RPC codes plus
//! the MCP-reserved `-32020..=-32022`; the legacy `-32000..=-32019` range is
//! never used.

use std::borrow::Cow;

use serde_json::{Value, json};

use crate::spec::{self, error_codes};

/// Maximum length of client-provided text echoed back in error messages.
const MAX_ECHO_CHARS: usize = 128;

/// How an error relates to the request, independent of transport. HTTP uses
/// it to pick a status code (400, 404, 200 or 500).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request is malformed or cannot be accepted as sent.
    BadRequest,
    /// The method does not exist.
    MethodNotFound,
    /// A well-formed request that could not be completed (e.g. unknown tool).
    Failed,
    /// A server-side failure.
    Internal,
}

/// A JSON-RPC error returned to the client.
#[derive(Debug, Clone, PartialEq)]
pub struct McpError {
    code: i64,
    kind: ErrorKind,
    message: Cow<'static, str>,
    // Boxed to keep `McpError` (and every `Result` carrying it) small.
    data: Option<Box<Value>>,
}

impl McpError {
    fn new(code: i64, kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
            data: None,
        }
    }

    /// `-32700`: the message is not valid JSON.
    #[must_use]
    pub fn parse_error() -> Self {
        Self::new(
            error_codes::PARSE_ERROR,
            ErrorKind::BadRequest,
            "Parse error",
        )
    }

    /// `-32600`: the message is not a valid JSON-RPC request.
    #[must_use]
    pub fn invalid_request(reason: &'static str) -> Self {
        Self::new(
            error_codes::INVALID_REQUEST_ERROR,
            ErrorKind::BadRequest,
            reason,
        )
    }

    /// `-32601`: the method does not exist.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            error_codes::METHOD_NOT_FOUND_ERROR,
            ErrorKind::MethodNotFound,
            format!("Method not found: {}", truncate(method)),
        )
    }

    /// `-32601` with a custom, client-safe explanation.
    #[must_use]
    pub fn method_not_found_explained(explanation: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            error_codes::METHOD_NOT_FOUND_ERROR,
            ErrorKind::MethodNotFound,
            explanation,
        )
    }

    /// `-32602`: the parameters are malformed (including missing `_meta`).
    #[must_use]
    pub fn invalid_params(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            error_codes::INVALID_PARAMS_ERROR,
            ErrorKind::BadRequest,
            message,
        )
    }

    /// `-32602`: the requested tool does not exist.
    #[must_use]
    pub fn unknown_tool(name: &str) -> Self {
        Self::new(
            error_codes::INVALID_PARAMS_ERROR,
            ErrorKind::Failed,
            format!("Unknown tool: {}", truncate(name)),
        )
    }

    /// `-32602`: a well-formed request the handler refused, with a
    /// client-safe message (for example a `ToolError::user` raised while
    /// reading a resource or getting a prompt).
    #[must_use]
    pub fn request_failed(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            error_codes::INVALID_PARAMS_ERROR,
            ErrorKind::Failed,
            message,
        )
    }

    /// `-32602` with `data.uri`: the resource does not exist (the
    /// 2026-07-28 code for resource-not-found).
    #[must_use]
    pub fn resource_not_found(uri: &str) -> Self {
        let mut err = Self::new(
            error_codes::INVALID_PARAMS_ERROR,
            ErrorKind::Failed,
            format!("Resource not found: {}", truncate(uri)),
        );
        err.data = Some(Box::new(json!({ "uri": uri })));
        err
    }

    /// `-32602`: the requested prompt does not exist.
    #[must_use]
    pub fn unknown_prompt(name: &str) -> Self {
        Self::new(
            error_codes::INVALID_PARAMS_ERROR,
            ErrorKind::Failed,
            format!("Unknown prompt: {}", truncate(name)),
        )
    }

    /// `-32603` with a client-safe message (no internal details).
    #[must_use]
    pub fn internal_with_message(message: &'static str) -> Self {
        Self::new(error_codes::INTERNAL_ERROR, ErrorKind::Internal, message)
    }

    /// `-32603`: a generic internal error. Details must be logged, not sent.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(
            error_codes::INTERNAL_ERROR,
            ErrorKind::Internal,
            "Internal error",
        )
    }

    /// `-32020`: HTTP headers do not match the request body.
    #[must_use]
    pub fn header_mismatch(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(
            error_codes::HEADER_MISMATCH_ERROR,
            ErrorKind::BadRequest,
            message,
        )
    }

    /// `-32021`: the request needs client capabilities the client did not declare.
    #[must_use]
    pub fn missing_client_capability(required: &spec::ClientCapabilities) -> Self {
        let mut err = Self::new(
            error_codes::MISSING_REQUIRED_CLIENT_CAPABILITY_ERROR,
            ErrorKind::BadRequest,
            "Missing required client capability",
        );
        err.data = Some(Box::new(json!({ "requiredCapabilities": required })));
        err
    }

    /// `-32022`: the requested protocol version is not supported.
    #[must_use]
    pub fn unsupported_protocol_version(requested: &str, supported: &[String]) -> Self {
        let mut err = Self::new(
            error_codes::UNSUPPORTED_PROTOCOL_VERSION_ERROR,
            ErrorKind::BadRequest,
            "Unsupported protocol version",
        );
        err.data = Some(Box::new(
            json!({ "supported": supported, "requested": truncate(requested) }),
        ));
        err
    }

    /// The JSON-RPC error code.
    #[must_use]
    pub fn code(&self) -> i64 {
        self.code
    }

    /// How the error relates to the request.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The client-facing message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The optional `data` member.
    #[must_use]
    pub fn data(&self) -> Option<&Value> {
        self.data.as_deref()
    }

    /// Converts into the wire representation.
    #[must_use]
    pub fn into_spec(self) -> spec::Error {
        spec::Error {
            code: self.code,
            message: self.message.into_owned(),
            data: self.data.map(|d| *d),
        }
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for McpError {}

/// Maximum length of an error message built from client input.
const MAX_MESSAGE_CHARS: usize = 512;

/// Maximum length of a quoted fragment kept in a sanitized message.
const MAX_QUOTED_CHARS: usize = 32;

/// Makes a message that may quote client input safe to send back: quoted
/// fragments longer than a few words are shortened (deserializer messages
/// quote the offending value), and the whole message is capped, so a large
/// request never produces a large error.
pub(crate) fn sanitize(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(MAX_MESSAGE_CHARS));
    let mut in_quote = false;
    let mut quoted = 0usize;
    let mut escaped = false;
    for c in message.chars() {
        if in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quote = false;
                if quoted > MAX_QUOTED_CHARS {
                    out.push('…');
                }
                out.push('"');
                continue;
            }
            quoted += 1;
            if quoted <= MAX_QUOTED_CHARS {
                out.push(c);
            }
            continue;
        }
        if c == '"' {
            in_quote = true;
            quoted = 0;
        }
        out.push(c);
        if out.chars().count() >= MAX_MESSAGE_CHARS {
            out.push('…');
            break;
        }
    }
    out
}

/// Shortens client-provided text before echoing it in an error message.
pub(crate) fn truncate(text: &str) -> Cow<'_, str> {
    match text.char_indices().nth(MAX_ECHO_CHARS) {
        Some((idx, _)) => Cow::Owned(format!("{}…", text.get(..idx).unwrap_or_default())),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_version_carries_data() {
        let err = McpError::unsupported_protocol_version("1900-01-01", &["2026-07-28".into()]);
        assert_eq!(err.code(), -32022);
        assert_eq!(
            err.data(),
            Some(&json!({ "supported": ["2026-07-28"], "requested": "1900-01-01" }))
        );
    }

    #[test]
    fn sanitize_shortens_quoted_input() {
        let long = format!(
            "invalid type: string \"{}\", expected f64 at line 1 column 9",
            "A".repeat(10_000)
        );
        let clean = sanitize(&long);
        assert!(clean.len() < 200, "{clean}");
        assert!(clean.contains("expected f64"));
        assert_eq!(sanitize("missing field `a`"), "missing field `a`");
        assert!(sanitize(&"x".repeat(10_000)).chars().count() <= MAX_MESSAGE_CHARS + 1);
    }

    #[test]
    fn echoed_text_is_truncated() {
        let long = "x".repeat(1000);
        let err = McpError::method_not_found(&long);
        assert!(err.message().chars().count() < 200);
    }
}
