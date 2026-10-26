//! Protocol extensions (for example `io.modelcontextprotocol/tasks` or
//! `io.modelcontextprotocol/ui`).
//!
//! Extensions are negotiated through capabilities: the server advertises
//! them in `server/discover`, and a client lists the ones it supports in each
//! request's `_meta`. An extension may only change behavior for requests
//! whose client declared it ([`Ctx::client_supports_extension`]); otherwise
//! the server falls back to core behavior, as the specification requires.
//!
//! This first version covers advertisement only. Extension-defined methods,
//! result types and result interception will be added to this trait without
//! changes to the core request pipeline (see `docs/ARCHITECTURE.md` §6.6).
//!
//! [`Ctx::client_supports_extension`]: crate::Ctx::client_supports_extension

use serde_json::{Map, Value};

/// A protocol extension supported by the server.
pub trait Extension: Send + Sync + 'static {
    /// The extension identifier, e.g. `io.modelcontextprotocol/tasks`. It
    /// must have a vendor prefix (`{prefix}/{name}`).
    fn id(&self) -> &str;

    /// The settings object advertised in `capabilities.extensions`. An empty
    /// object means "supported, no settings".
    fn settings(&self) -> Map<String, Value> {
        Map::new()
    }
}

/// Checks the identifier format: a dotted prefix of labels, a slash, and a
/// non-empty name (the `_meta` key rules with a mandatory prefix).
pub(crate) fn is_valid_id(id: &str) -> bool {
    let Some((prefix, name)) = id.split_once('/') else {
        return false;
    };
    let label_ok = |label: &str| {
        let mut chars = label.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
        let last_ok = label
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        first_ok && last_ok && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let name_ok = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    prefix.split('.').all(label_ok) && name_ok
}

#[cfg(test)]
mod tests {
    use super::is_valid_id;

    #[test]
    fn extension_ids() {
        assert!(is_valid_id("io.modelcontextprotocol/tasks"));
        assert!(is_valid_id("com.example/my-extension"));
        assert!(!is_valid_id("tasks"));
        assert!(!is_valid_id("com.example/"));
        assert!(!is_valid_id("1com.example/x"));
    }
}
