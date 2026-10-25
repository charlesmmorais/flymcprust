//! An ergonomic, safe and fast SDK for building
//! [Model Context Protocol](https://modelcontextprotocol.io) servers,
//! targeting specification revision **2026-07-28** (stateless).
//!
//! ```no_run
//! use sdkmcprust::prelude::*;
//!
//! /// Adds two numbers.
//! ///
//! /// # Arguments
//! /// * `a` - First operand.
//! /// * `b` - Second operand.
//! #[tool(read_only, idempotent)]
//! async fn add(a: f64, b: f64) -> f64 {
//!     a + b
//! }
//!
//! # #[cfg(feature = "stdio")]
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     Server::builder("calculator", "1.0.0")
//!         .tool::<add>()
//!         .build()?
//!         .serve_stdio()
//!         .await?;
//!     Ok(())
//! }
//! # #[cfg(not(feature = "stdio"))]
//! # fn main() {}
//! ```
//!
//! # Features
//!
//! | Feature | Default | Description |
//! | --- | --- | --- |
//! | `macros` | yes | The [`tool`] attribute macro. |
//! | `stdio` | no | stdio transport ([`Server::serve_stdio`]). |
//! | `http` | no | Streamable HTTP transport ([`transport::http`]). |
//! | `auto-register` | no | [`ServerBuilder::auto_tools`] collects every `#[tool]`. |
//!
//! See the repository's `docs/ARCHITECTURE.md` for design decisions.
#![forbid(unsafe_code)]

pub use sdkmcprust_core::{
    BuildError, Ctx, ErrorKind, Extension, IntoToolError, IntoToolResult, Json, Limits, McpError,
    McpRequest, MirroredParam, ParamKind, Server, ServerBuilder, State, Tool, ToolDef, ToolError,
    ToolOutput, ToolResult, TransportContext, jsonrpc, spec, transport,
};

#[cfg(feature = "macros")]
pub use sdkmcprust_macros::tool;

/// The most common imports.
pub mod prelude {
    pub use crate::{Ctx, Json, Server, State, ToolError};

    #[cfg(feature = "macros")]
    pub use crate::tool;
}

/// Implementation details used by `#[tool]`. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use ::schemars;
    pub use ::serde;
    pub use ::serde_json;
    pub use serde_json::value::RawValue;

    pub use sdkmcprust_core::{
        Ctx, IntoToolResult, Tool, ToolDef, ToolError, ToolFuture, input_schema_for,
        output_schema_from, parse_arguments, spec,
    };

    #[cfg(feature = "auto-register")]
    pub use sdkmcprust_core::{__inventory as inventory, AutoTool};

    /// Error for a `State<T>` that was never registered. Unreachable in
    /// practice: `ServerBuilder::build` rejects such servers.
    #[must_use]
    pub fn missing_state() -> ToolError {
        ToolError::internal(std::io::Error::other("tool state is not registered"))
    }
}
