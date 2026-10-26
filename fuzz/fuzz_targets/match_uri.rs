//! URI template parsing and matching must never panic, and values of
//! `{var}`, `{/var}` and query variables must never contain path separators
//! or control characters, whatever the template and URI.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mcpsdkrust_core::UriTemplate;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // First line: template; rest: URI.
    let (template, uri) = text.split_once('\n').unwrap_or((text, ""));
    let Ok(template) = UriTemplate::parse(template) else {
        return;
    };
    let Some(vars) = template.matches(uri) else {
        return;
    };
    let reserved: Vec<&str> = template
        .as_str()
        .split("{+")
        .skip(1)
        .filter_map(|rest| rest.split('}').next())
        .collect();
    for (name, value) in vars.iter() {
        assert!(!value.chars().any(char::is_control), "control char in {name}");
        if !reserved.contains(&name) {
            assert!(!value.contains(['/', '\\']), "separator in {name}");
        }
    }
});
