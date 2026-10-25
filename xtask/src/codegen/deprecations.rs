//! Extracts `@deprecated` JSDoc tags from `schema.ts`.
//!
//! `schema.json` carries no deprecation information, so the generator reads
//! the TypeScript source of truth. A tag applies to the next declaration:
//! either `export interface X` / `export type X` (a whole type) or a property
//! line inside an exported interface (a single field).

use std::collections::BTreeMap;

/// Deprecation notes found in the TypeScript schema.
#[derive(Debug, Default)]
pub(crate) struct Deprecations {
    /// Deprecated types, by definition name.
    pub(crate) types: BTreeMap<String, String>,
    /// Deprecated fields, by `(definition name, property name)`.
    pub(crate) fields: BTreeMap<(String, String), String>,
}

pub(crate) fn parse(ts: &str) -> Deprecations {
    let lines: Vec<&str> = ts.lines().collect();
    let mut out = Deprecations::default();
    let mut current_interface: Option<String> = None;
    let mut depth: i64 = 0;
    let mut pending: Option<String> = None;
    let mut i = 0;

    while let Some(line) = lines.get(i) {
        let trimmed = line.trim();

        if trimmed.starts_with("/**") {
            // Collect the whole JSDoc block.
            let mut block = Vec::new();
            while let Some(l) = lines.get(i) {
                block.push(l.trim());
                if l.contains("*/") {
                    break;
                }
                i += 1;
            }
            if let Some(note) = deprecation_note(&block) {
                pending = Some(note);
            }
            i += 1;
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with("//") {
            i += 1;
            continue;
        }

        if let Some(name) = declared_type(trimmed) {
            if let Some(note) = pending.take() {
                out.types.insert(name.clone(), note);
            }
            if trimmed.starts_with("export interface") {
                current_interface = Some(name);
                depth = 0;
            }
        } else if let Some(note) = pending.take()
            && let (Some(interface), Some(field)) = (&current_interface, property_name(trimmed))
        {
            out.fields.insert((interface.clone(), field), note);
        }

        depth += brace_delta(trimmed);
        if current_interface.is_some() && depth <= 0 && trimmed.contains('}') {
            current_interface = None;
            depth = 0;
        }
        i += 1;
    }
    out
}

fn deprecation_note(block: &[&str]) -> Option<String> {
    let mut note: Option<String> = None;
    for line in block {
        let text = line
            .trim_start_matches("/**")
            .trim_end_matches("*/")
            .trim_start_matches('*')
            .trim();
        if let Some(rest) = text.strip_prefix("@deprecated") {
            note = Some(rest.trim().to_owned());
        } else if let Some(n) = note.as_mut() {
            // Continuation lines until the next tag or the end of the block.
            if text.starts_with('@') || text.is_empty() {
                break;
            }
            n.push(' ');
            n.push_str(text);
        }
    }
    note.map(|n| {
        if n.is_empty() {
            "Deprecated in the specification.".to_owned()
        } else {
            n
        }
    })
}

fn declared_type(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("export interface ")
        .or_else(|| line.strip_prefix("export type "))?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn property_name(line: &str) -> Option<String> {
    let line = line.trim_start_matches("readonly ");
    let (name, _) = line.split_once(':')?;
    let name = name.trim().trim_end_matches('?').trim_matches('"');
    (!name.is_empty() && !name.contains(' ')).then(|| name.to_owned())
}

fn brace_delta(line: &str) -> i64 {
    let opens = line.matches('{').count();
    let closes = line.matches('}').count();
    i64::try_from(opens).unwrap_or(0) - i64::try_from(closes).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = r#"
/** @deprecated Deprecated as of protocol version 2026-07-28 (SEP-2577). */
export type LoggingLevel = "debug" | "info";

export interface RequestMetaObject {
  /**
   * Log level.
   *
   * @deprecated Deprecated as of 2026-07-28.
   */
  "io.modelcontextprotocol/logLevel"?: LoggingLevel;
  progressToken?: ProgressToken;
}

export interface ClientCapabilities {
  /**
   * @deprecated Sampling is deprecated.
   */
  // eslint-disable-next-line
  sampling?: {
    context?: object;
  };
  roots?: {};
}
"#;

    #[test]
    fn finds_type_and_field_deprecations() {
        let d = parse(TS);
        assert!(d.types.contains_key("LoggingLevel"));
        assert!(d.fields.contains_key(&(
            "RequestMetaObject".to_owned(),
            "io.modelcontextprotocol/logLevel".to_owned()
        )));
        assert!(
            d.fields
                .contains_key(&("ClientCapabilities".to_owned(), "sampling".to_owned()))
        );
        assert_eq!(d.fields.len(), 2);
    }
}
