//! Developer hints: what went wrong and how to fix it, written to the
//! server's log (never to clients, which only get short, sanitized errors).
//!
//! Problems a client can trigger repeatedly (rejected hosts, oversized
//! bodies) are logged at `WARN` the first time and at `DEBUG` afterwards, so
//! a hostile client cannot flood the log.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Remembers whether a hint was already logged.
#[derive(Debug, Default)]
pub(crate) struct Once(AtomicBool);

impl Once {
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// `true` the first time only.
    pub(crate) fn first(&self) -> bool {
        !self.0.swap(true, Ordering::Relaxed)
    }
}

/// Logs a problem with its fix: `WARN` the first time, `DEBUG` afterwards.
pub(crate) fn warn_once(once: &Once, problem: fmt::Arguments<'_>, help: &str) {
    if once.first() {
        tracing::warn!("{problem}\n  = help: {help}");
    } else {
        tracing::debug!("{problem}");
    }
}

/// Client-provided text, shortened and escaped for a log line (no newlines
/// or control characters, so it cannot forge log entries).
#[cfg_attr(not(feature = "http"), allow(dead_code))]
pub(crate) fn client_text(text: &str) -> String {
    format!("{:?}", crate::error::truncate(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_is_true_once() {
        let once = Once::new();
        assert!(once.first());
        assert!(!once.first());
    }

    #[test]
    fn client_text_is_escaped() {
        assert_eq!(client_text("a\nb\"c"), r#""a\nb\"c""#);
        assert!(client_text(&"x".repeat(1000)).len() < 200);
    }
}
