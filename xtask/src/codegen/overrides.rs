//! Per-revision generator overrides (`xtask/overrides/<revision>.toml`).

use std::collections::BTreeMap;

use serde::Deserialize;

/// Adjustments the generator cannot infer from the schema alone.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Overrides {
    /// Definitions that are not generated.
    #[serde(default)]
    pub(crate) skip: Vec<String>,
    /// Object definitions that keep unknown keys in a flattened `extra` map.
    #[serde(default)]
    pub(crate) open: Vec<String>,
    /// Definitions mapped to an existing Rust type (path as Rust source).
    #[serde(default)]
    pub(crate) type_map: BTreeMap<String, String>,
    /// Ordered `[from, to]` replacements applied to definition names.
    #[serde(default)]
    pub(crate) acronyms: Vec<[String; 2]>,
    /// Names for inline types, keyed by JSON path (`Def.field[.field]`).
    #[serde(default)]
    pub(crate) inline_names: BTreeMap<String, String>,
    /// Names for constant marker types, keyed by `field=value`.
    #[serde(default)]
    pub(crate) marker_names: BTreeMap<String, String>,
}
