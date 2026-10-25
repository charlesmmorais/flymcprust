//! Automatic registration (feature `auto-register`).
//!
//! `#[tool]`, `#[resource]` and `#[prompt]` submit an [`AutoTool`],
//! [`AutoResource`] or [`AutoPrompt`] to a link-time registry provided by the
//! `inventory` crate; [`ServerBuilder::auto_tools`](crate::ServerBuilder::auto_tools)
//! and its siblings collect them. The registry relies on platform linker sections, which is
//! why the feature is opt-in (see `docs/ARCHITECTURE.md` §8).

use crate::prompt::PromptDef;
use crate::resource::ResourceDef;
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

/// A resource registered by `#[resource]` for automatic discovery.
#[derive(Debug)]
pub struct AutoResource(fn() -> ResourceDef);

impl AutoResource {
    /// Wraps a resource's definition function.
    #[must_use]
    pub const fn new(definition: fn() -> ResourceDef) -> Self {
        Self(definition)
    }

    pub(crate) fn definition(&self) -> ResourceDef {
        (self.0)()
    }
}

inventory::collect!(AutoResource);

/// A prompt registered by `#[prompt]` for automatic discovery.
#[derive(Debug)]
pub struct AutoPrompt(fn() -> PromptDef);

impl AutoPrompt {
    /// Wraps a prompt's definition function.
    #[must_use]
    pub const fn new(definition: fn() -> PromptDef) -> Self {
        Self(definition)
    }

    pub(crate) fn definition(&self) -> PromptDef {
        (self.0)()
    }
}

inventory::collect!(AutoPrompt);
