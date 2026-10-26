//! FlyMCPRust: high-performance MCP server SDK for Rust. An ergonomic, safe
//! and fast way to build [Model Context Protocol](https://modelcontextprotocol.io) servers,
//! targeting specification revision **2026-07-28** (stateless).
//!
//! ```no_run
//! use flymcprust::prelude::*;
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
//! | `macros` | yes | The [`tool`], [`resource`] and [`prompt`] attribute macros. |
//! | `stdio` | no | stdio transport ([`Server::serve_stdio`]). |
//! | `http` | no | Streamable HTTP transport ([`transport::http`]). |
//! | `oauth` | no | `auth::JwtVerifier` for OAuth access tokens (implies `http`). |
//! | `auto-register` | no | [`ServerBuilder::auto_tools`] (and `auto_resources`, `auto_prompts`) collect every annotated item. |
//! | `logging` | no | `logging::init()` writes the log, with fix-it hints, to stderr. |
//! | `test-client` | no | In-process `testing::TestClient` for tests. |
//!
//! See the repository's `docs/ARCHITECTURE.md` for design decisions.
#![forbid(unsafe_code)]

pub use flymcprust_core::{
    AppUi, BuildError, CompleteFn, Completion, CompletionRequest, Ctx, Elicitation, ErrorKind,
    Extension, IntoContent, IntoPromptResult, IntoResourceContents, IntoToolError, IntoToolResult,
    Json, Limits, MIN_REQUEST_STATE_KEY_BYTES, McpError, McpRequest, MirroredParam, ParamKind,
    Permission, Principal, Prompt, PromptArguments, PromptDef, Resource, ResourceContents,
    ResourceDef, Server, ServerBuilder, State, TokenError, TokenVerifier, Tool, ToolDef, ToolError,
    ToolOutput, ToolResult, TransportContext, UriTemplate, UriTemplateError, UriVars, Visibility,
    apps, auth, jsonrpc, message, security, spec, transport,
};

#[cfg(feature = "macros")]
pub use flymcprust_macros::{prompt, resource, tool};

#[cfg(feature = "test-client")]
pub use flymcprust_core::testing;

/// Logging setup (feature `logging`).
#[cfg(feature = "logging")]
pub mod logging {
    /// Sends the SDK's log to stderr: warnings and errors come with a
    /// `help:` line that says how to fix them. stdout stays reserved for the
    /// stdio transport.
    ///
    /// The level comes from `RUST_LOG` (`error`, `warn`, `info`, `debug` or
    /// `trace`; default `info`). Does nothing if a global subscriber is
    /// already installed.
    pub fn init() {
        let level = std::env::var("RUST_LOG")
            .ok()
            .and_then(|v| {
                v.trim()
                    .parse::<tracing_subscriber::filter::LevelFilter>()
                    .ok()
            })
            .unwrap_or(tracing_subscriber::filter::LevelFilter::INFO);
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(level)
            .with_target(false)
            .try_init();
    }
}

/// The most common imports.
pub mod prelude {
    pub use crate::{Ctx, Json, Server, State, ToolError, message};

    #[cfg(feature = "macros")]
    pub use crate::{prompt, resource, tool};
}

/// Implementation details used by the macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use ::schemars;
    pub use ::serde;
    pub use ::serde_json;
    pub use serde_json::value::RawValue;

    pub use flymcprust_core::{
        AppUi, Completion, CompletionRequest, Ctx, IntoPromptResult, IntoResourceContents,
        IntoToolResult, Permission, Prompt, PromptArguments, PromptDef, PromptFuture, Resource,
        ResourceDef, ResourceFuture, Tool, ToolDef, ToolError, ToolFuture, UriVars, Visibility,
        apps, input_schema_for, output_schema_from, parse_arguments, parse_optional_uri_var,
        parse_uri_var, spec,
    };

    #[cfg(feature = "auto-register")]
    pub use flymcprust_core::{__inventory as inventory, AutoPrompt, AutoResource, AutoTool};

    /// Error for a `State<T>` that was never registered. Unreachable in
    /// practice: `ServerBuilder::build` rejects such servers.
    #[must_use]
    pub fn missing_state() -> ToolError {
        ToolError::internal(std::io::Error::other("tool state is not registered"))
    }
}
