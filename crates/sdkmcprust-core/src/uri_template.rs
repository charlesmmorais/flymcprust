//! RFC 6570 URI templates: the subset needed to match resource URIs.
//!
//! Supported expressions:
//!
//! | Expression | Matches |
//! | --- | --- |
//! | `{var}` | One or more characters other than `/`, `?` and `#` (percent-decoded) |
//! | `{+var}` | One or more characters other than `?` and `#`, so it may span `/` (percent-decoded) |
//! | `{/var}` | `/` followed by a `{var}` value |
//!
//! Decoded values are checked again, so encoding cannot smuggle characters
//! past these rules: `{var}`, `{/var}` and query values never contain `/` or
//! `\` (not even as `%2F` or `%5C`), and no value contains control
//! characters (such as `%00` or `%0A`). A URI that breaks these rules does
//! not match. `{+var}` values may contain `/` and `..`: handlers that map them
//! to files must confine them (see the `security` module of `sdkmcprust`).
//! | `{?a,b}`, `{&c}` | Optional query parameters, in any order (only at the end) |
//!
//! Matching is deterministic and linear in the URI length: a variable ends at
//! the first occurrence of the literal that follows it (`{var}`, `{/var}`) or
//! at the last occurrence (`{+var}`). Templates are checked when the server is
//! built, so ambiguous shapes (two adjacent variables, two `{+var}`) are
//! rejected up front.

use std::fmt;

/// URIs longer than this never match a template.
const MAX_MATCH_LEN: usize = 8 * 1024;

/// A parsed URI template.
#[derive(Clone, PartialEq, Eq)]
pub struct UriTemplate {
    source: String,
    parts: Vec<Part>,
    query: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Var { name: String, op: Op },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `{var}`
    Simple,
    /// `{+var}`
    Reserved,
    /// `{/var}`
    Path,
}

/// A template that cannot be parsed or is not supported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid URI template `{template}`: {reason}")]
pub struct UriTemplateError {
    template: String,
    reason: String,
}

impl UriTemplate {
    /// Parses a template.
    ///
    /// # Errors
    ///
    /// Returns a [`UriTemplateError`] for malformed templates and for RFC 6570
    /// features outside the supported subset (see the module documentation).
    pub fn parse(template: &str) -> Result<Self, UriTemplateError> {
        let fail = |reason: &str| UriTemplateError {
            template: template.to_owned(),
            reason: reason.to_owned(),
        };
        if template.is_empty() {
            return Err(fail("the template is empty"));
        }
        let mut parts = Vec::new();
        let mut query: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut rest = template;
        while !rest.is_empty() {
            if let Some(after_brace) = rest.strip_prefix('{') {
                let end = after_brace.find('}').ok_or_else(|| fail("unclosed `{`"))?;
                let (expr, tail) = after_brace.split_at(end);
                rest = tail.get(1..).unwrap_or_default();
                let (op, list) = match expr.chars().next() {
                    Some('+') => (Some(Op::Reserved), expr.get(1..).unwrap_or_default()),
                    Some('/') => (Some(Op::Path), expr.get(1..).unwrap_or_default()),
                    Some('?' | '&') => (None, expr.get(1..).unwrap_or_default()),
                    Some('#' | '.' | ';' | '=' | ',' | '!' | '@' | '|') => {
                        return Err(fail(
                            "only `{var}`, `{+var}`, `{/var}` and `{?var}` expressions are supported",
                        ));
                    }
                    _ => (Some(Op::Simple), expr),
                };
                let vars: Vec<&str> = list.split(',').collect();
                for var in &vars {
                    if var.ends_with('*') || var.contains(':') {
                        return Err(fail("variable modifiers (`*`, `:n`) are not supported"));
                    }
                    if !is_var_name(var) {
                        return Err(fail(
                            "variable names must be non-empty and use only A-Z, a-z, 0-9 and `_`",
                        ));
                    }
                    if names.iter().any(|n| n == var) {
                        return Err(fail("a variable appears twice"));
                    }
                    names.push((*var).to_owned());
                }
                match op {
                    None => query.extend(vars.iter().map(|v| (*v).to_owned())),
                    Some(op) => {
                        if !query.is_empty() {
                            return Err(fail("query expressions must come last"));
                        }
                        let [name] = vars.as_slice() else {
                            return Err(fail(
                                "path expressions take exactly one variable; use one expression per variable",
                            ));
                        };
                        if let Some(Part::Var { op: prev, .. }) = parts.last()
                            && !(op == Op::Path && *prev != Op::Reserved)
                        {
                            return Err(fail(
                                "two variables must be separated by a literal (or use `{/var}`)",
                            ));
                        }
                        if op == Op::Reserved
                            && parts.iter().any(|p| {
                                matches!(
                                    p,
                                    Part::Var {
                                        op: Op::Reserved,
                                        ..
                                    }
                                )
                            })
                        {
                            return Err(fail("at most one `{+var}` expression is supported"));
                        }
                        parts.push(Part::Var {
                            name: (*name).to_owned(),
                            op,
                        });
                    }
                }
            } else {
                let end = rest.find(['{', '}']).unwrap_or(rest.len());
                if rest.starts_with('}') {
                    return Err(fail("unmatched `}`"));
                }
                let (literal, tail) = rest.split_at(end);
                if literal.chars().any(|c| c.is_control() || c == ' ') {
                    return Err(fail("literals cannot contain spaces or control characters"));
                }
                if !query.is_empty() {
                    return Err(fail("query expressions must come last"));
                }
                parts.push(Part::Literal(literal.to_owned()));
                rest = tail;
            }
        }
        if !matches!(parts.first(), Some(Part::Literal(_))) {
            return Err(fail(
                "the template must start with a literal scheme, e.g. `file:///{path}`",
            ));
        }
        Ok(Self {
            source: template.to_owned(),
            parts,
            query,
        })
    }

    /// The template text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The variable names, in order of appearance.
    pub fn variables(&self) -> impl Iterator<Item = &str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Var { name, .. } => Some(name.as_str()),
                Part::Literal(_) => None,
            })
            .chain(self.query.iter().map(String::as_str))
    }

    /// Matches a URI, returning the (percent-decoded) variable values.
    #[must_use]
    pub fn matches(&self, uri: &str) -> Option<UriVars> {
        if uri.len() > MAX_MATCH_LEN {
            return None;
        }
        let (path, query) = if self.query.is_empty() {
            (uri, None)
        } else {
            match uri.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (uri, None),
            }
        };
        let mut vars = Vec::new();
        let mut rest = path;
        let mut parts = self.parts.iter().peekable();
        while let Some(part) = parts.next() {
            match part {
                Part::Literal(literal) => rest = rest.strip_prefix(literal.as_str())?,
                Part::Var { name, op } => {
                    if *op == Op::Path {
                        rest = rest.strip_prefix('/')?;
                    }
                    let forbidden: &[char] = match op {
                        Op::Reserved => &['?', '#'],
                        Op::Simple | Op::Path => &['/', '?', '#'],
                    };
                    let end = match parts.peek() {
                        None => rest.len(),
                        Some(Part::Literal(next)) => match op {
                            Op::Reserved => rest.rfind(next.as_str())?,
                            Op::Simple | Op::Path => rest.find(next.as_str())?,
                        },
                        // A `{/var}` follows: stop at the next `/`.
                        Some(Part::Var { .. }) => rest.find('/').unwrap_or(rest.len()),
                    };
                    let (raw, tail) = rest.split_at(end);
                    if raw.is_empty() || raw.contains(forbidden) {
                        return None;
                    }
                    vars.push((name.clone(), decode_value(raw, *op != Op::Reserved)?));
                    rest = tail;
                }
            }
        }
        if !rest.is_empty() {
            return None;
        }
        if let Some(query) = query {
            for pair in query.split('&').filter(|p| !p.is_empty()) {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                if let Some(name) = self.query.iter().find(|n| n.as_str() == key)
                    && !vars.iter().any(|(n, _)| n == name)
                {
                    vars.push((name.clone(), decode_value(value, true)?));
                }
            }
        }
        Some(UriVars { vars })
    }
}

impl fmt::Debug for UriTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UriTemplate").field(&self.source).finish()
    }
}

impl fmt::Display for UriTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// Variable values extracted from a URI by [`UriTemplate::matches`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UriVars {
    vars: Vec<(String, String)>,
}

impl UriVars {
    /// The value of a variable. Query variables absent from the URI are `None`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// All `(name, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Percent-decodes a variable value and re-checks it: no control
/// characters, and no path separators when `single_segment`.
fn decode_value(raw: &str, single_segment: bool) -> Option<String> {
    let value = percent_decode(raw)?;
    let forbidden = |c: char| c.is_control() || (single_segment && matches!(c, '/' | '\\'));
    (!value.contains(forbidden)).then_some(value)
}

/// Decodes `%XX` escapes; `None` for malformed escapes or invalid UTF-8.
fn percent_decode(input: &str) -> Option<String> {
    if !input.contains('%') {
        return Some(input.to_owned());
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Whether `uri` looks like an absolute URI (`scheme:rest`, RFC 3986 scheme
/// syntax) without whitespace or control characters.
pub(crate) fn is_absolute_uri(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !uri.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(template: &str, uri: &str) -> Option<Vec<(String, String)>> {
        UriTemplate::parse(template)
            .unwrap()
            .matches(uri)
            .map(|v| v.vars)
    }

    fn pairs(list: &[(&str, &str)]) -> Option<Vec<(String, String)>> {
        Some(
            list.iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn simple_variables() {
        assert_eq!(
            vars("test://template/{id}/data", "test://template/123/data"),
            pairs(&[("id", "123")])
        );
        assert_eq!(
            vars("test://template/{id}/data", "test://template//data"),
            None
        );
        assert_eq!(
            vars("test://template/{id}/data", "test://template/1/2/data"),
            None
        );
        assert_eq!(
            vars("db://{table}/{id}", "db://users/a%20b"),
            pairs(&[("table", "users"), ("id", "a b")])
        );
        assert_eq!(vars("db://{table}/{id}", "db://users/%zz"), None);
        assert_eq!(
            vars("file:///{name}.txt", "file:///a.b.txt"),
            pairs(&[("name", "a.b")])
        );
        assert_eq!(
            vars("file:///{name}.txt", "file:///a.txt.txt"),
            None,
            "a simple variable ends at the first occurrence of the next literal"
        );
    }

    #[test]
    fn reserved_and_path_variables() {
        assert_eq!(
            vars("file:///{+path}", "file:///src/main.rs"),
            pairs(&[("path", "src/main.rs")])
        );
        assert_eq!(
            vars("file:///{+path}.meta", "file:///a.b/c.meta"),
            pairs(&[("path", "a.b/c")])
        );
        assert_eq!(
            vars("repo://{owner}{/name}", "repo://rust-lang/rust"),
            pairs(&[("owner", "rust-lang"), ("name", "rust")])
        );
    }

    #[test]
    fn query_variables_are_optional_and_unordered() {
        let t = "search://items{?q,limit}";
        assert_eq!(
            vars(t, "search://items?limit=5&q=a%2Bb&other=1"),
            pairs(&[("limit", "5"), ("q", "a+b")])
        );
        assert_eq!(vars(t, "search://items"), pairs(&[]));
        let parsed = UriTemplate::parse(t).unwrap();
        assert_eq!(parsed.variables().collect::<Vec<_>>(), ["q", "limit"]);
    }

    #[test]
    fn unsupported_templates_are_rejected() {
        for bad in [
            "",
            "{x}",
            "a://{x",
            "a://x}",
            "a://{x}{y}",
            "a://{+x}/{+y}",
            "a://{x*}",
            "a://{x:3}",
            "a://{#x}",
            "a://{x,y}",
            "a://{x}/{x}",
            "a://{?q}/tail",
            "a://{bad-name}",
            "a:// {x}",
        ] {
            assert!(
                UriTemplate::parse(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn decoded_values_are_rechecked() {
        let t = "file:///docs/{name}";
        for uri in [
            "file:///docs/..%2Fsecret",
            "file:///docs/..%5Csecret",
            "file:///docs/..\\secret",
            "file:///docs/a%00b",
            "file:///docs/a%0Ab",
        ] {
            assert_eq!(vars(t, uri), None, "{uri}");
        }
        assert_eq!(vars(t, "file:///docs/a%20b"), pairs(&[("name", "a b")]));
        assert_eq!(vars("q://x{?q}", "q://x?q=a%2Fb"), None);
        // `{+var}` may span segments, but never carries control characters.
        assert_eq!(
            vars("file:///{+path}", "file:///a%2Fb/c"),
            pairs(&[("path", "a/b/c")])
        );
        assert_eq!(vars("file:///{+path}", "file:///a%00"), None);
    }

    #[test]
    fn long_uris_never_match() {
        let uri = format!("file:///{}", "a".repeat(MAX_MATCH_LEN));
        assert_eq!(vars("file:///{+path}", &uri), None);
    }

    #[test]
    fn absolute_uris() {
        assert!(is_absolute_uri("test://static-text"));
        assert!(is_absolute_uri("urn:isbn:0451450523"));
        assert!(!is_absolute_uri("no-scheme"));
        assert!(!is_absolute_uri("1http://x"));
        assert!(!is_absolute_uri("http://a b"));
        assert!(!is_absolute_uri("http:"));
    }
}
