//! Resource limits with secure defaults.

use std::time::Duration;

/// Hard upper bound for [`Limits::max_json_depth`]: `serde_json` refuses to
/// nest deeper than this regardless of configuration.
pub const MAX_SUPPORTED_JSON_DEPTH: usize = 128;

/// Resource limits applied to every request.
///
/// The defaults are deliberately conservative. Every value can be changed
/// through the [`ServerBuilder`](crate::ServerBuilder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of one JSON-RPC message in bytes (HTTP body or stdio line).
    pub max_message_bytes: usize,
    /// Maximum nesting depth of a JSON-RPC message (objects and arrays).
    /// Values above [`MAX_SUPPORTED_JSON_DEPTH`] are clamped.
    pub max_json_depth: usize,
    /// Maximum time a single tool call may run.
    pub tool_timeout: Duration,
    /// Maximum number of tool calls executing at the same time.
    pub max_concurrent_calls: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: 4 * 1024 * 1024,
            max_json_depth: 64,
            tool_timeout: Duration::from_secs(30),
            max_concurrent_calls: 64,
        }
    }
}

impl Limits {
    /// The effective depth limit after clamping.
    #[must_use]
    pub fn effective_json_depth(&self) -> usize {
        self.max_json_depth.min(MAX_SUPPORTED_JSON_DEPTH)
    }
}
