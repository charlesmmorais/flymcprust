//! Known specification revisions and the commits they are pinned to.

use anyhow::{Result, bail};

/// A specification revision the SDK generates types for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Revision {
    /// Revision name as used by the specification, e.g. `2026-07-28`.
    pub(crate) name: &'static str,
    /// Commit of `modelcontextprotocol/modelcontextprotocol` to fetch from.
    pub(crate) commit: &'static str,
    /// Rust module name of the generated code.
    pub(crate) module: &'static str,
}

/// Every revision the SDK knows about.
pub(crate) const REVISIONS: &[Revision] = &[Revision {
    name: "2026-07-28",
    commit: "271ecc9accafdd9b83a3c869fa67c22953b2af80",
    module: "v2026_07_28",
}];

/// Files fetched for each revision.
pub(crate) const SPEC_FILES: &[&str] = &["schema.json", "schema.ts"];

/// Looks up a revision by name.
pub(crate) fn find(name: &str) -> Result<Revision> {
    match REVISIONS.iter().find(|r| r.name == name) {
        Some(r) => Ok(*r),
        None => {
            let known: Vec<_> = REVISIONS.iter().map(|r| r.name).collect();
            bail!("unknown revision `{name}`; known: {}", known.join(", "))
        }
    }
}
