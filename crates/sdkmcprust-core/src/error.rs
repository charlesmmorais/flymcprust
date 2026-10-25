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

/// A JSON-RPC error returned to the client.
#[derive(Debug, Clone, PartialEq)]
pub struct McpError {
    code: i64,
    message: Cow<'static, str>,
    // Boxed to keep `McpError` (and every `Result` carrying it) small.
    data: Option<Box<Value>>,
}

impl McpError {
    fn new(code: i64, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// `-32700`: the message is not valid JSON.
    #[must_use]
    pub fn parse_error() -> Self {
        Self::new(error_codes::PARSE_ERROR, "Parse error")
    }

    /// `-32600`: the message is not a valid JSON-RPC request.
    #[must_use]
    pub fn invalid_request(reason: &'static str) -> Self {
        Self::new(error_codes::INVALID_REQUEST_ERROR, reason)
    }

    /// `-32601`: the method does not exist.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            error_codes::METHOD_NOT_FOUND_ERROR,
            format!("Method not found: {}", truncate(method)),
        )
    }

    /// `-32601` with a custom, client-safe explanation.
    #[must_use]
    pub fn method_not_found_explained(explanation: impl Into<Cow<'static, str>>) -> Self {
        Self::new(error_codes::METHOD_NOT_FOUND_ERROR, explanation)
    }

    /// `-32602`: the parameters are invalid.
    #[must_use]
    pub fn invalid_params(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(error_codes::INVALID_PARAMS_ERROR, message)
    }

    /// `-32603`: a generic internal error. Details must be logged, not sent.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(error_codes::INTERNAL_ERROR, "Internal error")
    }

    /// `-32020`: HTTP headers do not match the request body.
    #[must_use]
    pub fn header_mismatch(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(error_codes::HEADER_MISMATCH_ERROR, message)
    }

    /// `-32021`: the request needs client capabilities the client did not declare.
    #[must_use]
    pub fn missing_client_capability(required: &spec::ClientCapabilities) -> Self {
        let mut err = Self::new(
            error_codes::MISSING_REQUIRED_CLIENT_CAPABILITY_ERROR,
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
    fn echoed_text_is_truncated() {
        let long = "x".repeat(1000);
        let err = McpError::method_not_found(&long);
        assert!(err.message().chars().count() < 200);
    }
}
