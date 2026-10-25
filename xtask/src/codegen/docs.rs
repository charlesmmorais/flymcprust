//! Converts schema descriptions into rustdoc-friendly text.

/// Rewrites TypeScript-flavored markdown for rustdoc:
/// `{@link X}` becomes `` `X` ``, and unlabeled code fences become `text`
/// fences so rustdoc never treats them as Rust.
pub(crate) fn sanitize(description: &str) -> String {
    let linked = replace_links(description);
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in linked.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if !in_fence && trimmed.trim() == "```" {
                out.push(line.replacen("```", "```text", 1));
            } else {
                out.push(line.to_owned());
            }
            in_fence = !in_fence;
        } else {
            out.push(line.to_owned());
        }
    }
    out.join("\n").trim().to_owned()
}

fn replace_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{@link ") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + "{@link ".len()..).unwrap_or_default();
        match after.find('}') {
            Some(end) => {
                let target = after.get(..end).unwrap_or_default();
                // `{@link Target | label}` or `{@link Target label}`.
                let target = target.split(['|', ' ']).next().unwrap_or(target).trim();
                out.push('`');
                out.push_str(target);
                out.push('`');
                rest = after.get(end + 1..).unwrap_or_default();
            }
            None => {
                out.push_str(rest.get(start..).unwrap_or_default());
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_fences() {
        let input = "See {@link Tool} and {@link ClientCapabilities.sampling | sampling}.\n```\n{}\n```\n```json\n{}\n```";
        let out = sanitize(input);
        assert!(out.starts_with("See `Tool` and `ClientCapabilities.sampling`."));
        assert!(out.contains("```text\n{}\n```"));
        assert!(out.contains("```json\n{}\n```"));
    }
}
