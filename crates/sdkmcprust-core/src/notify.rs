//! Request-scoped notifications: progress (`notifications/progress`) and,
//! deprecated in 2026-07-28, logging (`notifications/message`).
//!
//! Transports that can deliver notifications put a [`NotifyChannel`] in the
//! request's transport context: stdio writes them on the connection before
//! the response; HTTP turns the response into an SSE stream at the first
//! notification (and stays plain JSON otherwise). The server only builds a
//! [`Notifier`] when the client asked for notifications (a `progressToken`,
//! or a log level), so ordinary requests pay nothing.
//!
//! Sending never blocks a handler. When the client reads slowly, log messages
//! beyond the buffer are dropped and progress keeps only the latest value,
//! which is delivered before the final response. Progress values must
//! increase (the specification requires it): others are ignored. Nothing is
//! sent after the response.

#![allow(deprecated)] // Logging is deprecated (SEP-2577) but still specified.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::context::Ctx;
use crate::spec::{self, LoggingLevel, methods};

/// How long the final progress update may wait for buffer space.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

/// Where a transport delivers request-scoped notifications (serialized
/// JSON-RPC messages).
#[derive(Debug, Clone)]
pub(crate) struct NotifyChannel(pub(crate) mpsc::Sender<Vec<u8>>);

/// Severity rank of a log level (RFC 5424 order).
pub(crate) fn severity(level: LoggingLevel) -> u8 {
    match level {
        LoggingLevel::Debug => 0,
        LoggingLevel::Info => 1,
        LoggingLevel::Notice => 2,
        LoggingLevel::Warning => 3,
        LoggingLevel::Error => 4,
        LoggingLevel::Critical => 5,
        LoggingLevel::Alert => 6,
        LoggingLevel::Emergency => 7,
    }
}

/// Sends the notifications of one request.
#[derive(Debug)]
pub(crate) struct Notifier {
    out: mpsc::Sender<Vec<u8>>,
    progress_token: Option<spec::ProgressToken>,
    /// Minimum level of log messages; `None`: no log messages.
    log_level: Option<LoggingLevel>,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    last_progress: Option<f64>,
    /// The latest progress update that did not fit in the buffer.
    pending_progress: Option<Vec<u8>>,
    closed: bool,
}

impl Notifier {
    pub(crate) fn new(
        channel: &NotifyChannel,
        progress_token: Option<spec::ProgressToken>,
        log_level: Option<LoggingLevel>,
    ) -> Option<Self> {
        (progress_token.is_some() || log_level.is_some()).then(|| Self {
            out: channel.0.clone(),
            progress_token,
            log_level,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn progress(&self, progress: f64, total: Option<f64>, message: Option<&str>) {
        let Some(token) = &self.progress_token else {
            return;
        };
        let mut state = self.state();
        if state.closed
            || !progress.is_finite()
            || state.last_progress.is_some_and(|last| progress <= last)
        {
            return;
        }
        state.last_progress = Some(progress);
        let mut params = Map::new();
        params.insert("progressToken".to_owned(), json!(token));
        params.insert("progress".to_owned(), json!(progress));
        if let Some(total) = total.filter(|t| t.is_finite()) {
            params.insert("total".to_owned(), json!(total));
        }
        if let Some(message) = message {
            params.insert("message".to_owned(), json!(message));
        }
        let Some(message) = notification(methods::NOTIFICATIONS_PROGRESS, params) else {
            return;
        };
        // Latest wins: a newer update replaces one still waiting for room.
        state.pending_progress = None;
        if let Err(mpsc::error::TrySendError::Full(message)) = self.out.try_send(message) {
            state.pending_progress = Some(message);
        }
    }

    fn log(&self, level: LoggingLevel, logger: Option<&str>, data: Value) {
        let Some(minimum) = self.log_level else {
            return;
        };
        if severity(level) < severity(minimum) || self.state().closed {
            return;
        }
        let mut params = Map::new();
        params.insert("level".to_owned(), json!(level));
        params.insert("data".to_owned(), data);
        if let Some(logger) = logger {
            params.insert("logger".to_owned(), json!(logger));
        }
        if let Some(message) = notification(methods::NOTIFICATIONS_MESSAGE, params) {
            // A full buffer drops the message: logging is rate-limited.
            let _ = self.out.try_send(message);
        }
    }

    /// Ends the request's notifications: the latest pending progress update
    /// is delivered (before the response), and nothing is sent afterwards.
    pub(crate) async fn finish(&self) {
        let pending = {
            let mut state = self.state();
            state.closed = true;
            state.pending_progress.take()
        };
        if let Some(message) = pending {
            let _ = tokio::time::timeout(FLUSH_TIMEOUT, self.out.send(message)).await;
        }
    }
}

fn notification(method: &str, params: Map<String, Value>) -> Option<Vec<u8>> {
    serde_json::to_vec(&json!({ "jsonrpc": "2.0", "method": method, "params": params })).ok()
}

impl Ctx {
    /// Reports progress to the client (`notifications/progress`), when it
    /// asked for it with a `progressToken`; otherwise does nothing.
    ///
    /// Never blocks: if the client reads slowly, only the latest update is
    /// kept, and it arrives before the result. `progress` must increase from
    /// one call to the next (smaller values are ignored); `total` may be
    /// omitted with `None`.
    ///
    /// ```ignore
    /// for (i, file) in files.iter().enumerate() {
    ///     ctx.progress(i as f64, files.len() as f64);
    ///     load(file).await;
    /// }
    /// ```
    pub fn progress(&self, progress: f64, total: impl Into<Option<f64>>) {
        if let Some(notifier) = self.notifier() {
            notifier.progress(progress, total.into(), None);
        }
    }

    /// Like [`progress`](Self::progress), with a human-readable message.
    pub fn progress_message(
        &self,
        progress: f64,
        total: impl Into<Option<f64>>,
        message: impl AsRef<str>,
    ) {
        if let Some(notifier) = self.notifier() {
            notifier.progress(progress, total.into(), Some(message.as_ref()));
        }
    }

    /// Whether the client asked for progress notifications.
    #[must_use]
    pub fn wants_progress(&self) -> bool {
        self.notifier().is_some_and(|n| n.progress_token.is_some())
    }

    /// Sends a log message to the client (`notifications/message`), when it
    /// asked for messages at this level or above; otherwise does nothing.
    /// Never blocks; messages beyond the buffer are dropped.
    ///
    /// Do not include secrets or internal details: the client sees `data`.
    /// For the server's own logs, use `tracing` (stderr).
    #[deprecated(
        note = "Logging is deprecated as of protocol version 2026-07-28 (SEP-2577); log to stderr with `tracing` instead."
    )]
    pub fn log(&self, level: LoggingLevel, data: impl Serialize) {
        if let Some(notifier) = self.notifier()
            && let Ok(data) = serde_json::to_value(data)
        {
            notifier.log(level, None, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(capacity: usize) -> (NotifyChannel, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(capacity);
        (NotifyChannel(tx), rx)
    }

    fn decode(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn nothing_is_built_without_a_request_for_notifications() {
        let (tx, _rx) = channel(4);
        assert!(Notifier::new(&tx, None, None).is_none());
    }

    #[tokio::test]
    async fn progress_increases_and_keeps_the_latest_value() {
        let (tx, mut rx) = channel(1);
        let n = Notifier::new(&tx, Some(spec::ProgressToken::Integer(7)), None).unwrap();
        n.progress(10.0, Some(100.0), None);
        n.progress(5.0, Some(100.0), None); // not increasing: ignored
        n.progress(50.0, Some(100.0), Some("half")); // buffer full: pending
        n.progress(90.0, Some(100.0), None); // replaces the pending one
        let first = decode(&rx.recv().await.unwrap());
        assert_eq!(first["method"], "notifications/progress");
        assert_eq!(
            first["params"],
            json!({ "progressToken": 7, "progress": 10.0, "total": 100.0 })
        );
        n.finish().await;
        let last = decode(&rx.recv().await.unwrap());
        assert_eq!(last["params"]["progress"], 90.0);
        // Nothing after the end.
        n.progress(100.0, Some(100.0), None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn logs_respect_the_level() {
        let (tx, mut rx) = channel(8);
        let n = Notifier::new(&tx, None, Some(LoggingLevel::Warning)).unwrap();
        n.log(LoggingLevel::Info, None, json!("hidden"));
        n.log(LoggingLevel::Error, Some("db"), json!({ "e": 1 }));
        let message = decode(&rx.try_recv().unwrap());
        assert_eq!(
            message["params"],
            json!({ "level": "error", "logger": "db", "data": { "e": 1 } })
        );
        assert!(rx.try_recv().is_err());
        // No progress token: progress is ignored.
        n.progress(1.0, None, None);
        assert!(rx.try_recv().is_err());
    }
}
