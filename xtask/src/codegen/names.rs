//! Identifier conversion helpers.

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};

const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// Converts a JSON property name into a Rust field name (without `r#`).
///
/// Namespaced `_meta` keys keep only their last segment
/// (`io.modelcontextprotocol/protocolVersion` → `protocol_version`), and
/// leading sigils are dropped (`_meta` → `meta`, `$schema` → `schema`).
pub(crate) fn field_name(json: &str) -> String {
    let last = json.rsplit('/').next().unwrap_or(json);
    let trimmed = last.trim_start_matches(['_', '$']);
    trimmed.to_snake_case()
}

/// Whether a name must be written as a raw identifier.
pub(crate) fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

/// Converts an arbitrary string into an UpperCamelCase identifier.
pub(crate) fn pascal(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    let name = cleaned.to_upper_camel_case();
    match name.chars().next() {
        None => "Value".to_owned(),
        Some(c) if c.is_ascii_digit() => format!("V{name}"),
        Some(_) => name,
    }
}

/// Converts an arbitrary string into a SCREAMING_SNAKE_CASE identifier.
pub(crate) fn shouty(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.to_shouty_snake_case()
}

/// Applies acronym replacements to a definition name.
pub(crate) fn type_name(def: &str, acronyms: &[[String; 2]]) -> String {
    let mut name = def.to_owned();
    for [from, to] in acronyms {
        name = name.replace(from.as_str(), to);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_names() {
        assert_eq!(field_name("_meta"), "meta");
        assert_eq!(field_name("$schema"), "schema");
        assert_eq!(
            field_name("io.modelcontextprotocol/protocolVersion"),
            "protocol_version"
        );
        assert_eq!(field_name("structuredContent"), "structured_content");
        assert_eq!(field_name("ttlMs"), "ttl_ms");
    }

    #[test]
    fn pascal_names() {
        assert_eq!(pascal("tools/call"), "ToolsCall");
        assert_eq!(pascal("2.0"), "V20");
        assert_eq!(pascal("input_required"), "InputRequired");
        assert_eq!(pascal("thisServer"), "ThisServer");
    }

    #[test]
    fn acronyms() {
        let a = vec![
            ["JSONRPC".to_owned(), "JsonRpc".to_owned()],
            ["JSON".to_owned(), "Json".to_owned()],
        ];
        assert_eq!(
            type_name("JSONRPCErrorResponse", &a),
            "JsonRpcErrorResponse"
        );
        assert_eq!(type_name("JSONValue", &a), "JsonValue");
    }
}
