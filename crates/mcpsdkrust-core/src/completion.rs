//! Argument completion (`completion/complete`) for prompt arguments and
//! resource-template variables.
//!
//! With the macros, annotate a parameter with `#[complete(path)]` (a function
//! `fn(&str) -> impl IntoIterator<Item = impl Into<String>>` that receives the
//! partial value) or `#[complete(values = ["a", "b"])]` (a fixed list filtered
//! by prefix). By hand, attach a [`CompleteFn`] with
//! [`PromptDef::complete`](crate::PromptDef::complete) or
//! [`ResourceDef::complete`](crate::ResourceDef::complete).

use std::collections::BTreeMap;

use crate::spec;

/// Maximum number of values in one completion result (specification limit).
pub const MAX_COMPLETION_VALUES: usize = 100;

/// A completion provider. Completion runs synchronously and must be fast.
pub type CompleteFn = fn(&CompletionRequest<'_>) -> Completion;

/// What the client asked to complete.
#[derive(Debug, Clone, Copy)]
pub struct CompletionRequest<'a> {
    pub(crate) argument: &'a str,
    pub(crate) value: &'a str,
    pub(crate) context: &'a BTreeMap<String, String>,
}

impl<'a> CompletionRequest<'a> {
    /// The argument (or template variable) being completed.
    #[must_use]
    pub fn argument(&self) -> &'a str {
        self.argument
    }

    /// The partial value typed so far.
    #[must_use]
    pub fn value(&self) -> &'a str {
        self.value
    }

    /// Values already chosen for other arguments, when the client sent them.
    #[must_use]
    pub fn context(&self) -> &'a BTreeMap<String, String> {
        self.context
    }
}

/// Completion values. Results longer than [`MAX_COMPLETION_VALUES`] are
/// truncated, with `total` and `hasMore` set accordingly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    values: Vec<String>,
    total: Option<i64>,
    has_more: Option<bool>,
}

impl Completion {
    /// Completion from a list of values.
    pub fn new<I>(values: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        Self {
            values: values.into_iter().map(Into::into).collect(),
            total: None,
            has_more: None,
        }
    }

    /// The values of `candidates` that start with `prefix`, ignoring ASCII
    /// case.
    #[must_use]
    pub fn filter_prefix(candidates: &[&str], prefix: &str) -> Self {
        Self::new(
            candidates
                .iter()
                .filter(|c| {
                    c.get(..prefix.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
                })
                .copied(),
        )
    }

    /// Sets the total number of matches, when known.
    #[must_use]
    pub fn with_total(mut self, total: u64) -> Self {
        self.total = Some(i64::try_from(total).unwrap_or(i64::MAX));
        self
    }

    /// Declares that more matches exist than the values returned.
    #[must_use]
    pub fn with_more(mut self, has_more: bool) -> Self {
        self.has_more = Some(has_more);
        self
    }

    pub(crate) fn into_spec(mut self) -> spec::CompleteResultCompletion {
        if self.values.len() > MAX_COMPLETION_VALUES {
            let total = self.values.len();
            self.values.truncate(MAX_COMPLETION_VALUES);
            self.total = Some(
                self.total
                    .unwrap_or_else(|| i64::try_from(total).unwrap_or(i64::MAX)),
            );
            self.has_more = Some(true);
        }
        spec::CompleteResultCompletion {
            has_more: self.has_more,
            total: self.total,
            values: self.values,
        }
    }
}

impl<S: Into<String>> FromIterator<S> for Completion {
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Self::new(iter)
    }
}

/// A completion provider attached to one argument.
#[derive(Debug, Clone)]
pub(crate) struct Completer {
    pub(crate) argument: String,
    pub(crate) complete: CompleteFn,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_filter_ignores_case() {
        let c = Completion::filter_prefix(&["Rust", "Ruby", "Python"], "ru");
        assert_eq!(c.into_spec().values, ["Rust", "Ruby"]);
    }

    #[test]
    fn long_results_are_truncated() {
        let c = Completion::new((0..150).map(|i| i.to_string())).into_spec();
        assert_eq!(c.values.len(), 100);
        assert_eq!(c.total, Some(150));
        assert_eq!(c.has_more, Some(true));
    }
}
