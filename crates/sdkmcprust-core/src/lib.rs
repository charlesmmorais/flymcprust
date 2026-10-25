//! Runtime and transports for SDKMCPRust.
//!
//! Most users should depend on the `sdkmcprust` facade crate instead.
//!
//! The core is transport-agnostic: [`Server`] turns parsed JSON-RPC messages
//! into responses and implements `tower::Service`, while transports only move
//! bytes. See `docs/ARCHITECTURE.md` for the design.
#![forbid(unsafe_code)]

#[cfg(feature = "auto-register")]
mod auto;
mod context;
mod error;
mod extension;
pub mod jsonrpc;
mod limits;
mod mirror;
mod server;
mod tool;
pub mod transport;

/// Protocol types of the targeted revision.
pub use sdkmcprust_spec::v2026_07_28 as spec;

pub use context::{Ctx, State, TransportContext};
pub use error::{ErrorKind, McpError};
pub use extension::Extension;
pub use limits::{Limits, MAX_SUPPORTED_JSON_DEPTH};
pub use mirror::{MirroredParam, ParamKind};
pub use server::{BuildError, McpRequest, Server, ServerBuilder};
pub use tool::{
    CallFn, IntoToolError, IntoToolResult, Json, StateRequirement, Tool, ToolDef, ToolError,
    ToolFuture, ToolOutput, ToolResult, input_schema_for, output_schema_from, parse_arguments,
    schema_for,
};

#[cfg(feature = "auto-register")]
pub use auto::AutoTool;
#[cfg(feature = "auto-register")]
#[doc(hidden)]
pub use inventory as __inventory;
