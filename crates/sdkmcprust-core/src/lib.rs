//! Runtime and transports for SDKMCPRust.
//!
//! Most users should depend on the `sdkmcprust` facade crate instead.
//!
//! The core is transport-agnostic: [`Server`] turns parsed JSON-RPC messages
//! into responses and implements `tower::Service`, while transports only move
//! bytes. See `docs/ARCHITECTURE.md` for the design.
#![forbid(unsafe_code)]

mod context;
mod error;
mod extension;
pub mod jsonrpc;
mod limits;
mod server;
mod tool;

/// Protocol types of the targeted revision.
pub use sdkmcprust_spec::v2026_07_28 as spec;

pub use context::{Ctx, State, TransportContext};
pub use error::McpError;
pub use extension::Extension;
pub use limits::{Limits, MAX_SUPPORTED_JSON_DEPTH};
pub use server::{BuildError, McpRequest, Server, ServerBuilder};
pub use tool::{
    CallFn, IntoToolError, IntoToolResult, Json, StateRequirement, Tool, ToolDef, ToolError,
    ToolFuture, ToolOutput, ToolResult, input_schema_for, output_schema_from, parse_arguments,
    schema_for,
};
