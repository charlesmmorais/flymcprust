//! stdio transport: newline-delimited JSON-RPC over stdin/stdout.
//!
//! - Lines longer than the message limit are rejected and skipped up to the
//!   next newline, without buffering them.
//! - Requests run concurrently (bounded); responses are written by a single
//!   task, one message per line, so stdout only ever carries MCP messages.
//! - `notifications/cancelled` cancels the referenced in-flight request,
//!   which then produces no response.
//! - End of input stops reading; in-flight requests get a grace period to
//!   finish before the transport returns.
//! - Legacy clients (2025-11-25, 2025-06-18) open with `initialize`; the
//!   connection then remembers what they declared, and input requests of
//!   multi round-trip handlers are sent to them as server-to-client requests
//!   whose responses are routed back here. Server requests wait at most the
//!   tool timeout: if every request slot is taken by handlers waiting for
//!   the client while the client sends yet another request first, reading
//!   pauses until those waits time out.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::context::TransportContext;
use crate::error::McpError;
use crate::jsonrpc::{self, Message, Rejection, Response};
use crate::legacy::{ChannelPeer, LegacySession, PendingRequests};
use crate::server::{McpRequest, Server};
use crate::spec::{RequestId, methods};

/// Extra time granted to in-flight requests after end of input, on top of
/// the tool timeout.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

type InFlight = Arc<Mutex<HashMap<RequestId, CancellationToken>>>;

impl Server {
    /// Serves requests over the process's stdin and stdout until stdin is
    /// closed.
    ///
    /// When stdin is an interactive terminal (someone ran the server by
    /// hand), a short explanation is printed on stderr: an MCP server waits
    /// silently for JSON-RPC messages, which looks like a hang.
    ///
    /// # Errors
    ///
    /// Returns an error when reading stdin or writing stdout fails.
    pub async fn serve_stdio(self) -> io::Result<()> {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            explain_interactive_use();
        }
        serve_io(self, tokio::io::stdin(), tokio::io::stdout()).await
    }
}

/// Tells someone who started the server in a terminal what is going on.
// stderr is safe here: stdout is the protocol channel, stderr is not.
#[allow(clippy::print_stderr)]
fn explain_interactive_use() {
    let program = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "this-server".to_owned());
    eprintln!(
        "This is an MCP server: it reads JSON-RPC messages on stdin and is meant to be \
         started by an MCP client.\n  \
         = help: try it with the MCP Inspector:\n          \
         npx @modelcontextprotocol/inspector --cli path/to/{program} --protocol-era modern \
         --method tools/list\n\
         (Waiting for messages on stdin; press Ctrl+C to stop.)"
    );
}

/// Serves requests over any byte stream pair using the stdio framing (for
/// example a Unix socket, a TCP connection, or an in-memory pipe in tests).
///
/// # Errors
///
/// Returns an error when reading or writing fails.
pub async fn serve_io<R, W>(server: Server, reader: R, writer: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let limits = server.limits().clone();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(limits.max_concurrent_calls.max(1));
    let writer_task = tokio::spawn(write_loop(writer, rx));

    let in_flight: InFlight = Arc::default();
    // Legacy protocol: what the client declared at `initialize`, and the
    // server-to-client requests waiting for an answer.
    let legacy: Arc<Mutex<Option<LegacySession>>> = Arc::default();
    let peer = ChannelPeer {
        pending: Arc::new(PendingRequests::default()),
        owner: 0,
        out: tx.clone(),
        timeout: limits.tool_timeout,
    };
    let permits = Arc::new(Semaphore::new(limits.max_concurrent_calls.max(1)));
    let mut tasks = JoinSet::new();
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();

    let read_result = loop {
        // Reap finished tasks so the set does not grow without bound.
        while tasks.try_join_next().is_some() {}

        line.clear();
        match read_line(&mut reader, &mut line, limits.max_message_bytes).await {
            Ok(Line::Eof) => break Ok(()),
            Ok(Line::TooLong) => {
                let response =
                    Response::error(None, McpError::invalid_request("Message too large"));
                if tx.send(response.to_vec()).await.is_err() {
                    break Ok(());
                }
                continue;
            }
            Ok(Line::Complete) => {}
            Err(e) => break Err(e),
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let message = match jsonrpc::parse(&line, &limits) {
            Ok(message) => message,
            Err(Rejection::Error { id, error }) => {
                if tx.send(Response::error(id, error).to_vec()).await.is_err() {
                    break Ok(());
                }
                continue;
            }
            Err(Rejection::UnexpectedResponse) => {
                // A legacy client answering one of our requests.
                let routed = jsonrpc::parse_client_response(&line)
                    .is_some_and(|response| peer.pending.complete(response, 0));
                if !routed {
                    tracing::debug!("ignoring a JSON-RPC response that answers no pending request");
                }
                continue;
            }
        };

        let request = match message {
            Message::Notification(n) => {
                if n.method == methods::NOTIFICATIONS_CANCELLED {
                    cancel(&in_flight, n.params.as_deref());
                }
                // Other client notifications have no effect.
                continue;
            }
            Message::Request(request) => request,
        };

        if request.method == "initialize" && server.legacy_clients_enabled() {
            let response = match server.legacy_initialize(request.params.as_deref()) {
                Ok((result, session)) => {
                    *legacy
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session);
                    Response::Result {
                        id: request.id,
                        result,
                    }
                }
                Err(error) => Response::error(Some(request.id), error),
            };
            if tx.send(response.to_vec()).await.is_err() {
                break Ok(());
            }
            continue;
        }
        let session = legacy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let token = CancellationToken::new();
        let duplicate = match lock(&in_flight).entry(request.id.clone()) {
            std::collections::hash_map::Entry::Occupied(_) => true,
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(token.clone());
                false
            }
        };
        if duplicate {
            let response = Response::error(
                Some(request.id),
                McpError::invalid_request("Duplicate request id"),
            );
            if tx.send(response.to_vec()).await.is_err() {
                break Ok(());
            }
            continue;
        }

        // Backpressure: stop reading while too many requests are in flight.
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            break Ok(());
        };
        let server = server.clone();
        let tx = tx.clone();
        let in_flight = Arc::clone(&in_flight);
        let peer = peer.clone();
        tasks.spawn(async move {
            let id = request.id.clone();
            let response = match session {
                Some(session) => {
                    server
                        .handle_legacy(request, &session, TransportContext::default(), token, &peer)
                        .await
                }
                None => {
                    server
                        .handle(McpRequest {
                            message: Message::Request(request),
                            transport: TransportContext::default(),
                            cancellation: token,
                        })
                        .await
                }
            };
            lock(&in_flight).remove(&id);
            if let Some(response) = response {
                // The writer only stops when output is closed; nothing to do then.
                let _ = tx.send(response.to_vec()).await;
            }
            drop(permit);
        });
    };

    // End of input: let in-flight requests finish, within a deadline.
    let deadline = limits.tool_timeout + SHUTDOWN_GRACE;
    if tokio::time::timeout(deadline, async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tracing::warn!("in-flight requests did not finish before shutdown; aborting them");
        tasks.abort_all();
        // Aborted tasks release their copies of the writer channel when they
        // are dropped.
        while tasks.join_next().await.is_some() {}
    }
    // The writer stops once every sender is gone, including the legacy
    // peer's copy: otherwise end of input would never end the transport.
    drop(peer);
    drop(tx);
    let write_result = writer_task
        .await
        .unwrap_or_else(|e| Err(io::Error::other(e)));
    read_result.and(write_result)
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: mpsc::Receiver<Vec<u8>>,
) -> io::Result<()> {
    while let Some(mut message) = rx.recv().await {
        message.push(b'\n');
        writer.write_all(&message).await?;
        writer.flush().await?;
    }
    writer.shutdown().await.or(Ok(()))
}

#[derive(Deserialize)]
struct CancelledParams {
    #[serde(rename = "requestId")]
    request_id: RequestId,
}

fn cancel(in_flight: &InFlight, params: Option<&serde_json::value::RawValue>) {
    // Invalid cancellation notifications are ignored, as the spec requires.
    let Some(params) = params else { return };
    let Ok(params) = serde_json::from_str::<CancelledParams>(params.get()) else {
        return;
    };
    if let Some(token) = lock(in_flight).get(&params.request_id) {
        token.cancel();
    }
}

fn lock(in_flight: &InFlight) -> std::sync::MutexGuard<'_, HashMap<RequestId, CancellationToken>> {
    // A poisoned map is still structurally valid; keep serving.
    in_flight
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

enum Line {
    Complete,
    TooLong,
    Eof,
}

/// Reads one `\n`-terminated line into `buf` (without the terminator or a
/// trailing `\r`). A line longer than `max` is consumed and discarded.
async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<Line> {
    let mut too_long = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF: a final unterminated line still counts.
            return Ok(if too_long {
                Line::TooLong
            } else if buf.is_empty() {
                Line::Eof
            } else {
                Line::Complete
            });
        }
        let (chunk, found, used) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (available.get(..i).unwrap_or_default(), true, i + 1),
            None => (available, false, available.len()),
        };
        if !too_long {
            if buf.len() + chunk.len() > max {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        reader.consume(used);
        if found {
            if too_long {
                return Ok(Line::TooLong);
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Line::Complete);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_bounded_lines() {
        let input: &[u8] = b"short\r\nthis-line-is-too-long\nok\nlast";
        let mut reader = BufReader::with_capacity(4, input);
        let mut buf = Vec::new();
        let mut lines = Vec::new();
        loop {
            buf.clear();
            match read_line(&mut reader, &mut buf, 8).await.unwrap() {
                Line::Complete => lines.push(String::from_utf8(buf.clone()).unwrap()),
                Line::TooLong => lines.push("<too long>".to_owned()),
                Line::Eof => break,
            }
        }
        assert_eq!(lines, ["short", "<too long>", "ok", "last"]);
    }
}
