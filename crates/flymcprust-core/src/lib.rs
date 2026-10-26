//! Runtime and transports for FlyMCPRust.
//!
//! Most users should depend on the `flymcprust` facade crate instead.
//!
//! The core is transport-agnostic: [`Server`] turns parsed JSON-RPC messages
//! into responses and implements `tower::Service`, while transports only move
//! bytes. See `docs/ARCHITECTURE.md` for the design.
#![forbid(unsafe_code)]

pub mod apps;
pub mod auth;
#[cfg(feature = "auto-register")]
mod auto;
mod completion;
mod context;
mod error;
mod extension;
mod hints;
pub mod jsonrpc;
mod legacy;
mod limits;
mod mirror;
mod mrtr;
mod notify;
mod prompt;
mod resource;
pub mod security;
mod server;
#[cfg(feature = "test-client")]
pub mod testing;
mod tool;
pub mod transport;
mod uri_template;

/// Protocol types of the targeted revision.
pub use flymcprust_spec::v2026_07_28 as spec;

pub use apps::{AppUi, Permission, Visibility};
pub use auth::{Principal, TokenError, TokenVerifier};
pub use completion::{CompleteFn, Completion, CompletionRequest, MAX_COMPLETION_VALUES};
pub use context::{Ctx, State, TransportContext};
pub use error::{ErrorKind, McpError};
pub use extension::Extension;
pub use legacy::{LEGACY_PROTOCOL_VERSIONS, LegacySession, is_legacy_version};
pub use limits::{Limits, MAX_SUPPORTED_JSON_DEPTH};
pub use mirror::{MirroredParam, ParamKind};
pub use mrtr::{Elicitation, MIN_REQUEST_STATE_KEY_BYTES};
pub use prompt::{
    GetFn, IntoContent, IntoPromptResult, Prompt, PromptArguments, PromptDef, PromptFuture, message,
};
pub use resource::{
    IntoResourceContents, ReadFn, Resource, ResourceContents, ResourceDef, ResourceFuture,
    parse_optional_uri_var, parse_uri_var,
};
pub use server::{BuildError, McpRequest, Server, ServerBuilder};
pub use tool::{
    CallFn, IntoToolError, IntoToolResult, Json, StateRequirement, Tool, ToolDef, ToolError,
    ToolFuture, ToolOutput, ToolResult, input_schema_for, output_schema_from, parse_arguments,
    schema_for,
};
pub use uri_template::{UriTemplate, UriTemplateError, UriVars};

#[cfg(feature = "auto-register")]
pub use auto::{AutoPrompt, AutoResource, AutoTool};
#[cfg(feature = "auto-register")]
#[doc(hidden)]
pub use inventory as __inventory;
