//! Tool parameters mirrored into HTTP headers (`x-mcp-header`, SEP-2243).
//!
//! A property of a tool's `inputSchema` may carry `"x-mcp-header": "Name"`;
//! Streamable HTTP clients then send its value as `Mcp-Param-Name`, and the
//! server must check that header against the body. Annotations are validated
//! when the server is built, so an invalid tool definition is never served.

use serde_json::{Map, Value};

/// JSON Schema primitive type of a mirrored parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// `"type": "string"`.
    String,
    /// `"type": "integer"`.
    Integer,
    /// `"type": "boolean"`.
    Boolean,
}

/// A tool parameter mirrored into an `Mcp-Param-{name}` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredParam {
    header: String,
    path: Vec<String>,
    kind: ParamKind,
}

impl MirroredParam {
    /// The `{name}` part of the `Mcp-Param-{name}` header.
    #[must_use]
    pub fn header(&self) -> &str {
        &self.header
    }

    /// The chain of property names leading to the parameter.
    #[must_use]
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// The parameter's primitive type.
    #[must_use]
    pub fn kind(&self) -> ParamKind {
        self.kind
    }

    /// The parameter's value in the call arguments, if present.
    #[must_use]
    pub fn value_in<'v>(&self, arguments: &'v Value) -> Option<&'v Value> {
        self.path
            .iter()
            .try_fold(arguments, |current, key| current.get(key))
    }
}

const ANNOTATION: &str = "x-mcp-header";

/// Collects and validates every `x-mcp-header` annotation of a schema.
///
/// # Errors
///
/// Returns a description of the first violated constraint.
pub(crate) fn collect(schema: &Map<String, Value>) -> Result<Vec<MirroredParam>, String> {
    let mut found = Vec::new();
    walk_properties(schema, &mut Vec::new(), &mut found)?;

    // Annotations outside a pure `properties` chain (inside `items`,
    // `anyOf`, `$defs`, …) make the definition invalid.
    let total = count_annotations(&Value::Object(schema.clone()));
    if total != found.len() {
        return Err(format!(
            "`{ANNOTATION}` may only annotate properties reachable through `properties` keys"
        ));
    }

    let mut seen: Vec<String> = Vec::with_capacity(found.len());
    for param in &found {
        let lower = param.header.to_ascii_lowercase();
        if seen.contains(&lower) {
            return Err(format!(
                "`{ANNOTATION}` value `{}` is used twice (names are case-insensitive)",
                param.header
            ));
        }
        seen.push(lower);
    }
    Ok(found)
}

fn walk_properties(
    schema: &Map<String, Value>,
    path: &mut Vec<String>,
    found: &mut Vec<MirroredParam>,
) -> Result<(), String> {
    let Some(Value::Object(properties)) = schema.get("properties") else {
        return Ok(());
    };
    for (name, property) in properties {
        let Value::Object(property) = property else {
            continue;
        };
        path.push(name.clone());
        if let Some(annotation) = property.get(ANNOTATION) {
            found.push(validate(annotation, property, path)?);
        }
        walk_properties(property, path, found)?;
        path.pop();
    }
    Ok(())
}

fn validate(
    annotation: &Value,
    property: &Map<String, Value>,
    path: &[String],
) -> Result<MirroredParam, String> {
    let at = path.join(".");
    let header = annotation
        .as_str()
        .ok_or_else(|| format!("`{ANNOTATION}` on `{at}` must be a string"))?;
    if header.is_empty() || !header.bytes().all(is_tchar) {
        return Err(format!(
            "`{ANNOTATION}` value `{header}` on `{at}` is not a valid HTTP field name token"
        ));
    }
    let kind = match property.get("type").and_then(Value::as_str) {
        Some("string") => ParamKind::String,
        Some("integer") => ParamKind::Integer,
        Some("boolean") => ParamKind::Boolean,
        _ => {
            return Err(format!(
                "`{ANNOTATION}` on `{at}` requires `type` to be string, integer or boolean"
            ));
        }
    };
    Ok(MirroredParam {
        header: header.to_owned(),
        path: path.to_vec(),
        kind,
    })
}

fn count_annotations(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key(ANNOTATION))
                + map.values().map(count_annotations).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(count_annotations).sum(),
        _ => 0,
    }
}

/// `tchar` from RFC 9110 §5.6.2.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    #[test]
    fn collects_nested_annotations() {
        let s = schema(json!({
            "type": "object",
            "properties": {
                "region": { "type": "string", "x-mcp-header": "Region" },
                "query": { "type": "string" },
                "opts": {
                    "type": "object",
                    "properties": { "dry": { "type": "boolean", "x-mcp-header": "Dry-Run" } }
                }
            }
        }));
        let params = collect(&s).unwrap();
        assert_eq!(params.len(), 2);
        // Property order depends on serde_json's `preserve_order` feature.
        let by_header = |h: &str| params.iter().find(|p| p.header() == h).unwrap();
        let args = json!({ "region": "us-west1", "opts": { "dry": true } });
        assert_eq!(by_header("Dry-Run").value_in(&args), Some(&json!(true)));
        assert_eq!(by_header("Dry-Run").path(), ["opts", "dry"]);
        assert_eq!(
            by_header("Region").value_in(&args),
            Some(&json!("us-west1"))
        );
    }

    #[test]
    fn rejects_invalid_annotations() {
        let bad_type = schema(json!({ "properties": {
            "n": { "type": "number", "x-mcp-header": "N" } } }));
        assert!(collect(&bad_type).is_err());

        let bad_token = schema(json!({ "properties": {
            "n": { "type": "string", "x-mcp-header": "a b" } } }));
        assert!(collect(&bad_token).is_err());

        let duplicate = schema(json!({ "properties": {
            "a": { "type": "string", "x-mcp-header": "Name" },
            "b": { "type": "string", "x-mcp-header": "name" } } }));
        assert!(collect(&duplicate).is_err());

        let in_items = schema(json!({ "properties": {
            "list": { "type": "array", "items": { "type": "string", "x-mcp-header": "Item" } } } }));
        assert!(collect(&in_items).is_err());
    }
}
