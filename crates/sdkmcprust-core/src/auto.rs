//! Automatic tool registration (feature `auto-register`).
//!
//! `#[tool]` submits an [`AutoTool`] to a link-time registry provided by the
//! `inventory` crate; [`ServerBuilder::auto_tools`](crate::ServerBuilder::auto_tools)
//! collects them. The registry relies on platform linker sections, which is
//! why the feature is opt-in (see `docs/ARCHITECTURE.md` §8).

use crate::tool::ToolDef;

/// A tool registered by `#[tool]` for automatic discovery.
#[derive(Debug)]
pub struct AutoTool(fn() -> ToolDef);

impl AutoTool {
    /// Wraps a tool's definition function.
    #[must_use]
    pub const fn new(definition: fn() -> ToolDef) -> Self {
        Self(definition)
    }

    pub(crate) fn definition(&self) -> ToolDef {
        (self.0)()
    }
}

inventory::collect!(AutoTool);
