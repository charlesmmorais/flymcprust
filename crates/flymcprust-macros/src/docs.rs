//! Doc-comment handling: the tool description and the `# Arguments` list.

use std::collections::BTreeMap;

use syn::{Attribute, Expr, ExprLit, Lit, Meta};

/// The text of every `#[doc = "..."]` attribute, one entry per line, with
/// the single leading space of `///` comments removed.
pub(crate) fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(nv) if nv.path.is_ident("doc") => match &nv.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|text| {
            // `split` (unlike `lines`) keeps empty `///` lines as paragraph breaks.
            text.split('\n')
                .map(|l| l.trim_end_matches('\r'))
                .map(|l| l.strip_prefix(' ').unwrap_or(l).to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Splits doc lines into the description (the text before the first
/// markdown heading) and the parameter descriptions found in an
/// `# Arguments` (or `# Parameters`) section. Other sections (`# Errors`,
/// `# Examples`, …) are rustdoc-only and are not sent to the model.
///
/// Accepted list items: ``* `name` - text``, ``- `name`: text`` and
/// ``* name - text``. Continuation lines are appended to the previous item.
pub(crate) fn split(lines: &[String]) -> (Option<String>, BTreeMap<String, String>) {
    let mut description = Vec::new();
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    let mut in_args = false;
    let mut seen_heading = false;
    let mut current: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            in_args = heading.eq_ignore_ascii_case("arguments")
                || heading.eq_ignore_ascii_case("parameters");
            seen_heading = true;
            current = None;
            continue;
        }
        if !in_args {
            if !seen_heading {
                description.push(line.as_str());
            }
            continue;
        }
        if let Some((name, text)) = parse_item(trimmed) {
            params.insert(name.clone(), text);
            current = Some(name);
        } else if let (Some(name), false) = (&current, trimmed.is_empty()) {
            if let Some(text) = params.get_mut(name) {
                text.push(' ');
                text.push_str(trimmed);
            }
        } else if trimmed.is_empty() {
            current = None;
        }
    }

    let description = description.join("\n").trim().to_owned();
    ((!description.is_empty()).then_some(description), params)
}

fn parse_item(line: &str) -> Option<(String, String)> {
    let rest = line
        .strip_prefix("* ")
        .or_else(|| line.strip_prefix("- "))?
        .trim_start();
    let (name, after) = if let Some(stripped) = rest.strip_prefix('`') {
        let end = stripped.find('`')?;
        (stripped.get(..end)?, stripped.get(end + 1..)?)
    } else {
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
        (rest.get(..end)?, rest.get(end..)?)
    };
    let text = after
        .trim_start()
        .trim_start_matches(['-', ':', '—', '–'])
        .trim();
    (!name.is_empty()).then(|| (name.to_owned(), text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn splits_description_and_arguments() {
        let (description, params) = split(&lines(
            "Adds two numbers.\n\nReturns the sum.\n\n# Arguments\n* `a` - First operand.\n* `b`: Second\n  operand.\n\n# Examples\nignored",
        ));
        assert_eq!(
            description.as_deref(),
            Some("Adds two numbers.\n\nReturns the sum.")
        );
        assert_eq!(params.get("a").map(String::as_str), Some("First operand."));
        assert_eq!(params.get("b").map(String::as_str), Some("Second operand."));
    }

    #[test]
    fn no_docs() {
        let (description, params) = split(&[]);
        assert!(description.is_none());
        assert!(params.is_empty());
    }
}
