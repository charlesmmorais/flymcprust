//! Streamable HTTP transport, stateless (MCP 2026-07-28).
//!
//! [`StreamableHttpService`] is a `tower::Service<http::Request<B>>`, so it
//! can be mounted in any tower-based framework, e.g. with axum:
//! `Router::new().nest_service("/mcp", StreamableHttpService::new(server))`.
//! [`Server::serve_http`] and [`serve`] run it on `hyper` directly.
//!
//! Request handling, in order:
//! 1. `Origin` must be allowed (403); `Host` must be allowed (403).
//! 2. Only `POST` (405); `OPTIONS` answers CORS preflights from allowed origins.
//! 3. `Content-Type: application/json` (415); `Accept` must allow JSON (406).
//! 4. At most [`HttpConfig::max_in_flight`] requests are processed at once,
//!    and their bodies may total at most
//!    [`HttpConfig::max_body_bytes_in_flight`] (503 beyond either); the body
//!    is read up to the message limit (413) within
//!    [`HttpConfig::body_timeout`] (408).
//! 5. `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` and recognized
//!    `Mcp-Param-*` headers must match the body (400, `-32020`).
//! 6. The message is dispatched; notifications get `202`, unknown methods
//!    `404`, malformed requests `400`, everything else `200`.
//!
//! `Last-Event-ID` is ignored: streams are not resumable. Dropping the
//! response future (client disconnect) cancels a 2026-07-28 request.
//!
//! Legacy clients (2025-11-25, 2025-06-18) open with `initialize` and get a
//! signed `MCP-Session-Id` that carries what they declared, so any instance
//! sharing the `request_state_key` serves them without server-side session
//! state. Their requests skip the 2026-07-28 header checks. When a handler
//! asks for input, the response becomes an SSE stream carrying the
//! server-to-client request; the client answers with a POST that is routed
//! back to the waiting handler. `DELETE` gets `405`: sessions end when their
//! token expires.

use std::borrow::Cow;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderValue};
use http::{Method, Request, Response, StatusCode};
use http_body::Body;
use http_body_util::{BodyExt as _, Full};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::oauth::Challenge;
pub use super::oauth::{OAuth, OAuthBuilder, OAuthError, ProtectedResourceMetadata};
use crate::auth::AuthContext;
use crate::context::TransportContext;
use crate::error::{ErrorKind, McpError};
use crate::hints::{self, Once};
use crate::jsonrpc::{self, Message, Rejection, Response as RpcResponse};
use crate::legacy::{self, ChannelPeer, LegacySession, PendingRequests};
use crate::mirror::ParamKind;
use crate::notify::NotifyChannel;
use crate::server::{McpRequest, Server};
use crate::spec::{RequestId, methods};

/// The HTTP response body: a complete JSON body, or an SSE stream (legacy
/// clients asked for input).
pub struct HttpBody(BodyKind);

enum BodyKind {
    Full(Full<Bytes>),
    Stream(Pin<Box<dyn Body<Data = Bytes, Error = Infallible> + Send>>),
}

impl HttpBody {
    pub(crate) fn full(bytes: impl Into<Bytes>) -> Self {
        Self(BodyKind::Full(Full::new(bytes.into())))
    }
}

impl std::fmt::Debug for HttpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            BodyKind::Full(_) => f.write_str("HttpBody::Full"),
            BodyKind::Stream(_) => f.write_str("HttpBody::Stream"),
        }
    }
}

impl Body for HttpBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, Infallible>>> {
        match &mut self.get_mut().0 {
            BodyKind::Full(body) => Pin::new(body).poll_frame(cx),
            BodyKind::Stream(body) => body.as_mut().poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.0 {
            BodyKind::Full(body) => body.is_end_stream(),
            BodyKind::Stream(body) => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match &self.0 {
            BodyKind::Full(body) => body.size_hint(),
            BodyKind::Stream(body) => body.size_hint(),
        }
    }
}

const MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const MCP_METHOD: &str = "mcp-method";
const MCP_NAME: &str = "mcp-name";
const MCP_PARAM_PREFIX: &str = "mcp-param-";
const MCP_SESSION_ID: &str = "mcp-session-id";
const BASE64_PREFIX: &str = "=?base64?";
const BASE64_SUFFIX: &str = "?=";
const ALLOWED_REQUEST_HEADERS: &str =
    "content-type, accept, authorization, mcp-protocol-version, mcp-method, mcp-name";

/// `Origin` values accepted by the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllowedOrigins {
    /// Origins whose host is `localhost`, `127.0.0.1` or `[::1]` (any
    /// scheme and port). The default.
    Localhost,
    /// Exactly these origins (e.g. `https://app.example.com`).
    List(Vec<String>),
    /// Any origin. Only safe behind other protections such as
    /// authentication; it disables DNS-rebinding protection.
    Any,
}

/// `Host` values accepted by the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllowedHosts {
    /// `localhost`, `127.0.0.1` or `[::1]`, on any port. The default.
    Localhost,
    /// Exactly these host names (ports are ignored).
    List(Vec<String>),
    /// Any host.
    Any,
}

/// Security settings of the Streamable HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    oauth: Option<OAuth>,
    origins: AllowedOrigins,
    hosts: AllowedHosts,
    header_timeout: Duration,
    body_timeout: Duration,
    max_in_flight: usize,
    max_body_bytes_in_flight: usize,
    max_connections: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            oauth: None,
            origins: AllowedOrigins::Localhost,
            hosts: AllowedHosts::Localhost,
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(30),
            max_in_flight: 4096,
            max_body_bytes_in_flight: 64 * 1024 * 1024,
            max_connections: 4096,
        }
    }
}

impl HttpConfig {
    /// Secure defaults: localhost origins and hosts only.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requires OAuth 2.1 access tokens on every request (the server acts
    /// as a resource server). See [`OAuth`].
    #[must_use]
    pub fn oauth(mut self, oauth: OAuth) -> Self {
        self.oauth = Some(oauth);
        self
    }

    /// Sets the accepted `Origin` values.
    #[must_use]
    pub fn allowed_origins(mut self, origins: AllowedOrigins) -> Self {
        self.origins = origins;
        self
    }

    /// Sets the accepted `Host` values. Public deployments must list their
    /// own host names (or use [`AllowedHosts::Any`]).
    #[must_use]
    pub fn allowed_hosts(mut self, hosts: AllowedHosts) -> Self {
        self.hosts = hosts;
        self
    }

    /// Time allowed, in [`serve`], to receive a request's headers, counted
    /// from when the connection opens or becomes idle. Slower or idle
    /// connections are closed (slow-loris protection). Default: 10 s.
    #[must_use]
    pub fn header_timeout(mut self, timeout: Duration) -> Self {
        self.header_timeout = timeout;
        self
    }

    /// Time allowed to receive a request body; slower clients get `408`.
    /// Default: 30 s.
    #[must_use]
    pub fn body_timeout(mut self, timeout: Duration) -> Self {
        self.body_timeout = timeout;
        self
    }

    /// Maximum number of requests being read or processed at once. Further
    /// requests get `503`. Default: 4096. Memory is bounded separately by
    /// [`max_body_bytes_in_flight`](Self::max_body_bytes_in_flight), so
    /// requests that wait on other services can be numerous.
    #[must_use]
    pub fn max_in_flight(mut self, requests: usize) -> Self {
        self.max_in_flight = requests.max(1);
        self
    }

    /// Maximum total size of the request bodies held at once (being read,
    /// parsed or handled). A request reserves its `Content-Length`, or the
    /// whole message limit when the length is unknown, and gets `503` when
    /// the budget is spent. Values below the message limit are raised to it.
    /// Default: 64 MiB.
    #[must_use]
    pub fn max_body_bytes_in_flight(mut self, bytes: usize) -> Self {
        self.max_body_bytes_in_flight = bytes;
        self
    }

    /// Maximum number of open connections in [`serve`]; further connections
    /// wait in the operating system's accept queue. Default: 4096.
    #[must_use]
    pub fn max_connections(mut self, connections: usize) -> Self {
        self.max_connections = connections.max(1);
        self
    }
}

/// Streamable HTTP as a `tower::Service`.
#[derive(Clone, Debug)]
pub struct StreamableHttpService {
    server: Server,
    config: Arc<HttpConfig>,
    in_flight: Arc<Semaphore>,
    /// One permit per byte of request body held (see
    /// [`HttpConfig::max_body_bytes_in_flight`]).
    body_bytes: Arc<Semaphore>,
    hints: Arc<Hints>,
    /// Server-to-client requests of legacy sessions awaiting an answer.
    pending: Arc<PendingRequests>,
}

/// Which developer hints were already logged.
#[derive(Debug, Default)]
struct Hints {
    origin: Once,
    host: Once,
    busy: Once,
    slow_body: Once,
    too_large: Once,
    session: Once,
}

impl StreamableHttpService {
    /// Wraps a server with the default (localhost-only) security settings.
    #[must_use]
    pub fn new(server: Server) -> Self {
        Self::with_config(server, HttpConfig::default())
    }

    /// Wraps a server with explicit security settings.
    #[must_use]
    pub fn with_config(server: Server, config: HttpConfig) -> Self {
        match &config.oauth {
            None if server.has_scopes() => tracing::warn!(
                "handlers declare OAuth scopes, but this HTTP service does not require OAuth: \
                 the scopes are not enforced\n  = help: add HttpConfig::new().oauth(OAuth::\
                 builder(..)..build()?), or remove the scopes"
            ),
            Some(oauth) => {
                let host = oauth
                    .resource()
                    .split_once("://")
                    .map(|(_, rest)| rest.split(['/', '?']).next().unwrap_or_default())
                    .unwrap_or_default();
                if !host_name_allowed(&config.hosts, strip_port(host)) {
                    tracing::warn!(
                        "the OAuth resource {} is not on an allowed host, so its requests \
                         will get HTTP 403\n  = help: list the host with \
                         HttpConfig::allowed_hosts(AllowedHosts::List(..))",
                        oauth.resource()
                    );
                }
            }
            None => {}
        }
        // A budget smaller than one message would refuse legitimate
        // requests forever.
        let body_budget = config
            .max_body_bytes_in_flight
            .max(server.limits().max_message_bytes)
            .min(Semaphore::MAX_PERMITS);
        Self {
            body_bytes: Arc::new(Semaphore::new(body_budget)),
            server,
            in_flight: Arc::new(Semaphore::new(config.max_in_flight)),
            config: Arc::new(config),
            hints: Arc::default(),
            pending: Arc::default(),
        }
    }

    async fn handle<B>(self, request: Request<B>) -> Response<HttpBody>
    where
        B: Body,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let (mut parts, body) = request.into_parts();

        let cors = match parts.headers.get(header::ORIGIN) {
            None => None,
            Some(origin) if origin_allowed(&self.config.origins, origin) => Some(origin.clone()),
            Some(origin) => {
                hints::warn_once(
                    &self.hints.origin,
                    format_args!(
                        "rejected a request from Origin {} (HTTP 403)",
                        hints::client_text(origin.to_str().unwrap_or("<not ASCII>"))
                    ),
                    match self.config.origins {
                        AllowedOrigins::Localhost => {
                            "only localhost origins are accepted by default (DNS-rebinding \
                             protection); to allow a web app, list it: \
                             HttpConfig::new().allowed_origins(AllowedOrigins::List(vec![\
                             \"https://app.example.com\".into()]))"
                        }
                        _ => "add the origin to HttpConfig::allowed_origins if it is legitimate",
                    },
                );
                return rpc_error(
                    StatusCode::FORBIDDEN,
                    None,
                    &McpError::invalid_request("Origin not allowed"),
                    None,
                );
            }
        };
        if !host_allowed(&self.config.hosts, &parts) {
            let host = parts
                .headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str))
                .unwrap_or("<none>");
            hints::warn_once(
                &self.hints.host,
                format_args!(
                    "rejected a request for Host {} (HTTP 403)",
                    hints::client_text(host)
                ),
                match self.config.hosts {
                    AllowedHosts::Localhost => {
                        "only localhost is accepted by default (DNS-rebinding protection); to \
                         serve other machines, list your host names: \
                         HttpConfig::new().allowed_hosts(AllowedHosts::List(vec![\
                         \"mcp.example.com\".into()]))"
                    }
                    _ => "add the host name to HttpConfig::allowed_hosts if it is legitimate",
                },
            );
            return rpc_error(
                StatusCode::FORBIDDEN,
                None,
                &McpError::invalid_request("Host not allowed"),
                None,
            );
        }

        match parts.method {
            Method::POST => {}
            Method::OPTIONS if cors.is_some() => return preflight(cors, &parts.headers),
            _ => {
                let mut response = plain(StatusCode::METHOD_NOT_ALLOWED, cors.as_ref());
                response
                    .headers_mut()
                    .insert(header::ALLOW, HeaderValue::from_static("POST"));
                return response;
            }
        }

        if !is_json_content_type(parts.headers.get(header::CONTENT_TYPE)) {
            return rpc_error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                None,
                &McpError::invalid_request("Content-Type must be application/json"),
                cors.as_ref(),
            );
        }
        if !accepts_json(&parts.headers) {
            return rpc_error(
                StatusCode::NOT_ACCEPTABLE,
                None,
                &McpError::invalid_request("Accept must allow application/json"),
                cors.as_ref(),
            );
        }

        // Bounds the memory held by bodies being read or processed.
        let Ok(permit) = Arc::clone(&self.in_flight).try_acquire_owned() else {
            hints::warn_once(
                &self.hints.busy,
                format_args!(
                    "rejected a request: {} requests already in flight (HTTP 503)",
                    self.config.max_in_flight
                ),
                "raise HttpConfig::max_in_flight(..) if the machine has room, or run more \
                 instances",
            );
            return busy(cors.as_ref());
        };

        // Authorization comes before the body is read: unauthenticated
        // clients cannot make the server buffer their messages.
        let auth = match &self.config.oauth {
            None => None,
            Some(oauth) => match oauth.authenticate(&parts.headers).await {
                Ok(auth) => {
                    // Handlers see the principal, never the token: they
                    // cannot pass it through to other services by mistake.
                    parts.headers.remove(header::AUTHORIZATION);
                    Some(auth)
                }
                Err(challenge) => return challenge_response(oauth, &challenge, cors.as_ref()),
            },
        };

        let limits = self.server.limits();
        let declared = parts
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let too_large = || {
            hints::warn_once(
                &self.hints.too_large,
                format_args!(
                    "rejected a request body over the {}-byte message limit (HTTP 413)",
                    limits.max_message_bytes
                ),
                "raise ServerBuilder::max_message_bytes(..) if clients legitimately                  send larger arguments",
            );
            rpc_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                None,
                &McpError::invalid_request("Message too large"),
                cors.as_ref(),
            )
        };
        if declared.is_some_and(|len| len > limits.max_message_bytes as u64) {
            return too_large();
        }
        // Each frame reserves its size in the memory budget as it arrives,
        // so a client that sends slowly holds only what it sent.
        let read = read_body(body, limits.max_message_bytes, Arc::clone(&self.body_bytes));
        let Ok(read) = tokio::time::timeout(self.config.body_timeout, read).await else {
            hints::warn_once(
                &self.hints.slow_body,
                format_args!(
                    "a client did not send its request body within {:?} (HTTP 408)",
                    self.config.body_timeout
                ),
                "raise HttpConfig::body_timeout(..) if clients on slow networks are expected",
            );
            return rpc_error(
                StatusCode::REQUEST_TIMEOUT,
                None,
                &McpError::invalid_request("Request body not received in time"),
                cors.as_ref(),
            );
        };
        // The reservation lasts as long as this request is handled.
        let (bytes, _body_permit) = match read {
            Ok(read) => read,
            Err(BodyError::TooLarge) => return too_large(),
            Err(BodyError::Busy) => {
                hints::warn_once(
                    &self.hints.busy,
                    format_args!(
                        "rejected a request: request bodies in flight reached {} bytes                          (HTTP 503)",
                        self.config.max_body_bytes_in_flight
                    ),
                    "raise HttpConfig::max_body_bytes_in_flight(..) if the machine has memory                      to spare, or run more instances",
                );
                return busy(cors.as_ref());
            }
            Err(BodyError::Read) => {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    None,
                    &McpError::invalid_request("Could not read the request body"),
                    cors.as_ref(),
                );
            }
        };

        // Legacy clients: a session token, or a legacy version header.
        let legacy = match self.legacy_session(&parts.headers, auth.as_ref()) {
            Ok(legacy) => legacy,
            Err(response) => return *response,
        };

        let message = match jsonrpc::parse(&bytes, limits) {
            Ok(message) => message,
            Err(Rejection::Error { id, error }) => {
                return rpc_error(status_for(&error), id, &error, cors.as_ref());
            }
            Err(Rejection::UnexpectedResponse) if legacy.is_some() => {
                // A legacy client answering one of our requests.
                let owner = session_owner(&parts.headers);
                let routed = jsonrpc::parse_client_response(&bytes)
                    .is_some_and(|response| self.pending.complete(response, owner));
                return if routed {
                    plain(StatusCode::ACCEPTED, cors.as_ref())
                } else {
                    rpc_error(
                        StatusCode::BAD_REQUEST,
                        None,
                        &McpError::invalid_request("The response answers no pending request"),
                        cors.as_ref(),
                    )
                };
            }
            Err(Rejection::UnexpectedResponse) => {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    None,
                    &McpError::invalid_request("Clients must not send JSON-RPC responses"),
                    cors.as_ref(),
                );
            }
        };

        // A body that declares 2026-07-28 in `_meta` follows 2026-07-28 rules
        // (including header/body agreement), whatever its version header.
        let legacy = legacy.filter(|_| {
            parts.headers.contains_key(MCP_SESSION_ID) || !declares_modern_meta(&message)
        });

        // `initialize` opens a legacy session, unless the request declares
        // 2026-07-28 (legacy clients never send the version header with
        // `initialize`): for that protocol the method does not exist.
        let modern = parts
            .headers
            .get(MCP_PROTOCOL_VERSION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| self.server.supported_versions().iter().any(|s| s == v));
        if let Message::Request(request) = &message
            && request.method == "initialize"
        {
            if self.server.legacy_clients_enabled() && !modern {
                return self.legacy_initialize(request, cors.as_ref(), auth.as_ref());
            }
            if modern {
                return rpc_error(
                    StatusCode::NOT_FOUND,
                    Some(request.id.clone()),
                    &McpError::method_not_found_explained(
                        "Method not found: `initialize` is not part of MCP 2026-07-28",
                    ),
                    cors.as_ref(),
                );
            }
        }
        if let Some(session) = legacy {
            let mut transport = TransportContext::new();
            let owner = session_owner(&parts.headers);
            transport.insert(parts);
            if let Some(auth) = auth {
                transport.insert(auth);
            }
            return self
                .handle_legacy(message, session, owner, transport, permit, cors)
                .await;
        }

        // A legacy client's `initialize` carries none of the modern headers;
        // let the server answer it with an error naming the supported
        // versions (the spec recommends that diagnostic for legacy clients).
        let mut wants_notifications = false;
        if let Message::Request(request) = &message
            && request.method != "initialize"
        {
            match validate_headers(&self.server, &parts.headers, request) {
                Ok(wants) => wants_notifications = wants,
                Err(error) => {
                    return rpc_error(
                        StatusCode::BAD_REQUEST,
                        Some(request.id.clone()),
                        &error,
                        cors.as_ref(),
                    );
                }
            }
        }

        let mut transport = TransportContext::new();
        transport.insert(parts);
        if let Some(auth) = auth {
            transport.insert(auth);
        }
        if wants_notifications {
            // The client asked for progress or log messages: they can only
            // travel on an SSE response, which closing cancels the request.
            let cancellation = CancellationToken::new();
            let guard = cancellation.clone().drop_guard();
            let server = self.server.clone();
            return self
                .respond_streaming(permit, cors, Some(guard), move |out| {
                    let mut transport = transport;
                    transport.insert(NotifyChannel(out));
                    Box::pin(async move {
                        server
                            .handle(McpRequest {
                                message,
                                transport,
                                cancellation,
                            })
                            .await
                    })
                })
                .await;
        }
        let response = self
            .server
            .handle(McpRequest {
                message,
                transport,
                cancellation: CancellationToken::new(),
            })
            .await;
        self.reply(response, cors.as_ref())
    }

    /// The HTTP response for a JSON-RPC response (`None`: a notification).
    /// Scope errors carry an `insufficient_scope` challenge.
    fn reply(
        &self,
        response: Option<RpcResponse>,
        cors: Option<&HeaderValue>,
    ) -> Response<HttpBody> {
        let Some(response) = response else {
            return plain(StatusCode::ACCEPTED, cors);
        };
        let status = response.as_error().map_or(StatusCode::OK, status_for);
        let challenge = match (&self.config.oauth, response.as_error()) {
            (Some(oauth), Some(error)) if error.kind() == ErrorKind::Forbidden => oauth
                .www_authenticate(&Challenge::InsufficientScope(
                    error.required_scopes().join(" "),
                )),
            _ => None,
        };
        let mut http = json(status, response.to_vec(), cors);
        if let Some(challenge) = challenge {
            http.headers_mut()
                .insert(header::WWW_AUTHENTICATE, challenge);
        }
        http
    }
}

impl StreamableHttpService {
    /// The legacy session of a request, if it is a legacy request. A token
    /// that fails verification (or expired) gets `404`, which tells the
    /// client to initialize again. A 2026-07-28 version header always means
    /// a 2026-07-28 request.
    #[allow(clippy::result_large_err)]
    fn legacy_session(
        &self,
        headers: &HeaderMap,
        auth: Option<&AuthContext>,
    ) -> Result<Option<LegacySession>, Box<Response<HttpBody>>> {
        if !self.server.legacy_clients_enabled() {
            return Ok(None);
        }
        let version = headers
            .get(MCP_PROTOCOL_VERSION)
            .and_then(|v| v.to_str().ok());
        if version.is_some_and(|v| self.server.supported_versions().iter().any(|s| s == v)) {
            return Ok(None);
        }
        match headers.get(MCP_SESSION_ID).and_then(|v| v.to_str().ok()) {
            Some(token) => match self
                .server
                .open_session(token)
                // A session opened with one user's token is not another's.
                .filter(|s| s.principal.as_deref() == auth.map(|a| a.digest.as_str()))
            {
                Some(session) => Ok(Some(session)),
                None => {
                    hints::warn_once(
                        &self.hints.session,
                        format_args!("rejected an unknown or expired MCP-Session-Id (HTTP 404)"),
                        "the client will initialize again; if this server runs as several \
                         instances, give all of them the same ServerBuilder::request_state_key(..)",
                    );
                    Err(Box::new(rpc_error(
                        StatusCode::NOT_FOUND,
                        None,
                        &McpError::invalid_request("Session not found or expired"),
                        None,
                    )))
                }
            },
            None => Ok(version
                .filter(|v| legacy::is_legacy_version(v))
                .map(LegacySession::sessionless)),
        }
    }

    fn legacy_initialize(
        &self,
        request: &jsonrpc::Request,
        cors: Option<&HeaderValue>,
        auth: Option<&AuthContext>,
    ) -> Response<HttpBody> {
        match self.server.legacy_initialize(request.params.as_deref()) {
            Ok((result, mut session)) => {
                session.principal = auth.map(|a| a.digest.clone());
                let body = RpcResponse::Result {
                    id: request.id.clone(),
                    result,
                }
                .to_vec();
                let mut response = json(StatusCode::OK, body, cors);
                if let Some(token) = self.server.seal_session(&session)
                    && let Ok(value) = HeaderValue::from_str(&token)
                {
                    response.headers_mut().insert(MCP_SESSION_ID, value);
                }
                response
            }
            Err(error) => rpc_error(status_for(&error), Some(request.id.clone()), &error, cors),
        }
    }

    /// Handles a legacy message. A request answers with JSON, or with an SSE
    /// stream as soon as the handler asks the client for input.
    async fn handle_legacy(
        &self,
        message: Message,
        session: LegacySession,
        owner: u64,
        transport: TransportContext,
        permit: OwnedSemaphorePermit,
        cors: Option<HeaderValue>,
    ) -> Response<HttpBody> {
        let Message::Request(request) = message else {
            // Notifications (`initialized`, `cancelled`) need no answer.
            return plain(StatusCode::ACCEPTED, cors.as_ref());
        };
        let pending = Arc::clone(&self.pending);
        let timeout = self.server.limits().tool_timeout;
        let server = self.server.clone();
        // 2025-11-25: a disconnection is not a cancellation.
        self.respond_streaming(permit, cors, None, move |out| {
            let mut transport = transport;
            transport.insert(NotifyChannel(out.clone()));
            let peer = ChannelPeer {
                pending,
                owner,
                out,
                timeout,
            };
            Box::pin(async move {
                server
                    .handle_legacy(
                        request,
                        &session,
                        transport,
                        CancellationToken::new(),
                        &peer,
                    )
                    .await
            })
        })
        .await
    }

    /// Runs a request whose handler may emit messages before its response
    /// (notifications, or requests to a legacy client). The response is
    /// plain JSON if the handler finishes first, and an SSE stream from the
    /// first message on. `cancel` is cancelled when the response is dropped
    /// (the client closed the stream).
    async fn respond_streaming<F>(
        &self,
        permit: OwnedSemaphorePermit,
        cors: Option<HeaderValue>,
        cancel: Option<DropGuard>,
        run: F,
    ) -> Response<HttpBody>
    where
        F: FnOnce(
            mpsc::Sender<Vec<u8>>,
        ) -> Pin<Box<dyn Future<Output = Option<RpcResponse>> + Send>>,
    {
        let (out, mut outgoing) = mpsc::channel::<Vec<u8>>(STREAM_BUFFER);
        let (done, mut finished) = oneshot::channel::<Option<RpcResponse>>();
        let task = run(out);
        // The request counts against the in-flight limit until both the
        // handler and the response stream are done: a legacy handler keeps
        // running after its client disconnects.
        let permit = Arc::new(permit);
        let task_permit = Arc::clone(&permit);
        tokio::spawn(async move {
            let _ = done.send(task.await);
            drop(task_permit);
        });

        let reply = |response: Option<RpcResponse>| self.reply(response, cors.as_ref());
        let first = tokio::select! {
            biased;
            first = outgoing.recv() => first,
            response = &mut finished => return reply(response.ok().flatten()),
        };
        let Some(first) = first else {
            return reply(finished.await.ok().flatten());
        };
        let state = SseState {
            queue: std::collections::VecDeque::from([first]),
            outgoing,
            finished: Some(finished),
            _permit: permit,
            _cancel: cancel,
        };
        let stream = futures_util::stream::unfold(state, |mut state| async move {
            loop {
                if let Some(message) = state.queue.pop_front() {
                    return Some((Ok(sse_event(&message)), state));
                }
                let finished = state.finished.as_mut()?;
                tokio::select! {
                    biased;
                    message = state.outgoing.recv() => match message {
                        Some(message) => state.queue.push_back(message),
                        None => {
                            // Every sender is gone: the response follows.
                            let response = state.finished.take()?.await.ok().flatten()?;
                            state.queue.push_back(response.to_vec());
                        }
                    },
                    response = finished => {
                        // Messages sent before the response come first.
                        state.finished = None;
                        while let Ok(message) = state.outgoing.try_recv() {
                            state.queue.push_back(message);
                        }
                        if let Ok(Some(response)) = response {
                            state.queue.push_back(response.to_vec());
                        }
                    }
                }
            }
        });
        let body = http_body_util::StreamBody::new(stream);
        let mut response = Response::new(HttpBody(BodyKind::Stream(Box::pin(body))));
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        // Reverse proxies must not buffer the stream (spec recommendation).
        headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        with_cors(&mut response, cors.as_ref());
        response
    }
}

/// Whether a request's `_meta` carries the 2026-07-28 protocol version.
fn declares_modern_meta(message: &Message) -> bool {
    let Message::Request(request) = message else {
        return false;
    };
    request.params.as_deref().is_some_and(|params| {
        serde_json::from_str::<PeekParams<'_>>(params.get())
            .ok()
            .and_then(|p| p.meta)
            .is_some_and(|meta| meta.protocol_version.is_some())
    })
}

/// Messages buffered between a handler and its SSE response.
const STREAM_BUFFER: usize = 32;

/// State of an SSE response.
struct SseState {
    /// Messages ready to send, in order.
    queue: std::collections::VecDeque<Vec<u8>>,
    outgoing: mpsc::Receiver<Vec<u8>>,
    /// The handler's response; `None` once received.
    finished: Option<oneshot::Receiver<Option<RpcResponse>>>,
    /// Counts against the in-flight limit until the stream ends.
    _permit: Arc<OwnedSemaphorePermit>,
    /// Cancels the request if the stream is dropped (client disconnect).
    _cancel: Option<DropGuard>,
}

fn sse_event(message: &[u8]) -> http_body::Frame<Bytes> {
    let mut event = Vec::with_capacity(message.len() + 24);
    event.extend_from_slice(b"event: message\ndata: ");
    event.extend_from_slice(message);
    event.extend_from_slice(b"\n\n");
    http_body::Frame::data(Bytes::from(event))
}

/// Binds server-to-client requests to the session that caused them, so one
/// client cannot answer another's.
fn session_owner(headers: &HeaderMap) -> u64 {
    use std::hash::{BuildHasher as _, BuildHasherDefault, DefaultHasher};
    headers.get(MCP_SESSION_ID).map_or(0, |token| {
        BuildHasherDefault::<DefaultHasher>::default().hash_one(token.as_bytes())
    })
}

impl<B> tower_service::Service<Request<B>> for StreamableHttpService
where
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = Response<HttpBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let service = self.clone();
        Box::pin(async move { Ok(service.handle(request).await) })
    }
}

impl Server {
    /// Serves Streamable HTTP at path `/mcp` with the default security
    /// settings (localhost origins and hosts only) until the process stops.
    ///
    /// Bind to a loopback address (e.g. `127.0.0.1:8000`) unless the server
    /// is meant to be reachable from other machines.
    ///
    /// # Errors
    ///
    /// Returns an error when the address cannot be bound.
    pub async fn serve_http(
        self,
        addr: impl tokio::net::ToSocketAddrs + Clone + std::fmt::Debug,
    ) -> io::Result<()> {
        let listener = TcpListener::bind(addr.clone()).await.map_err(|error| {
            let help = match error.kind() {
                io::ErrorKind::AddrInUse => {
                    "another program (or another copy of this server) is \
                     already using this port; stop it or choose another port"
                }
                io::ErrorKind::PermissionDenied => {
                    "ports below 1024 usually need elevated \
                     privileges; choose a port such as 8000"
                }
                _ => "check the address, for example `127.0.0.1:8000`",
            };
            io::Error::new(
                error.kind(),
                format!("could not listen on {addr:?}: {error}\n  = help: {help}"),
            )
        })?;
        serve(
            listener,
            StreamableHttpService::new(self),
            "/mcp",
            std::future::pending(),
        )
        .await
    }
}

/// Serves `service` at `path` on an already-bound listener until `shutdown`
/// completes, then waits up to the tool timeout for open connections.
///
/// # Errors
///
/// Returns an error when accepting connections fails.
pub async fn serve(
    listener: TcpListener,
    service: StreamableHttpService,
    path: &str,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    use hyper::server::conn::http1;
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::server::graceful::GracefulShutdown;

    if let Ok(addr) = listener.local_addr()
        && !addr.ip().is_loopback()
        && service.config.hosts == AllowedHosts::Localhost
    {
        tracing::warn!(
            "listening on {addr}, but only requests addressed to localhost are accepted\n  \
             = help: other machines will get HTTP 403; list the host names they use with \
             HttpConfig::allowed_hosts and serve with StreamableHttpService::with_config, or \
             bind 127.0.0.1"
        );
    }

    let path: Arc<str> = Arc::from(path);
    let metadata = service.config.oauth.as_ref().map(|oauth| {
        (
            Arc::<[String]>::from(oauth.metadata_paths()),
            oauth.metadata_service(),
        )
    });
    let grace = service.server.limits().tool_timeout;
    let connections = Arc::new(Semaphore::new(service.config.max_connections));
    // HTTP/1.1 only: its header timeout runs from the moment a connection
    // opens, so idle connections cannot hold slots. (Automatic HTTP/2
    // detection waits for the first bytes without any deadline.) Mount
    // `StreamableHttpService` in another server for HTTP/2.
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(service.config.header_timeout);
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);

    loop {
        // At capacity, stop accepting; new connections wait in the backlog.
        let permit = tokio::select! {
            permit = Arc::clone(&connections).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_closed) => break,
            },
            () = &mut shutdown => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Transient errors (e.g. too many open files) must not
                    // stop the server.
                    tracing::warn!(%error, "failed to accept a connection");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let service = service.clone();
        let path = Arc::clone(&path);
        let metadata = metadata.clone();
        let hyper_service =
            hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                let mut service = service.clone();
                let path = Arc::clone(&path);
                let metadata = metadata.clone();
                async move {
                    let requested = request.uri().path();
                    if requested == &*path {
                        return tower_service::Service::call(&mut service, request).await;
                    }
                    match &metadata {
                        Some((paths, metadata)) if paths.iter().any(|p| p == requested) => {
                            Ok(metadata.respond(request.method()))
                        }
                        _ => Ok(plain(StatusCode::NOT_FOUND, None)),
                    }
                }
            });
        let connection = builder.serve_connection(TokioIo::new(stream), hyper_service);
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%peer, %error, "connection closed with an error");
            }
            drop(permit);
        });
    }

    if tokio::time::timeout(grace, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("open connections did not close before the shutdown deadline");
    }
    Ok(())
}

#[derive(Deserialize)]
struct PeekParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<PeekMeta<'a>>,
    #[serde(borrow, default)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    uri: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    arguments: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct PeekMeta<'a> {
    #[serde(rename = "io.modelcontextprotocol/protocolVersion", borrow, default)]
    protocol_version: Option<Cow<'a, str>>,
    #[serde(rename = "progressToken", borrow, default)]
    progress_token: Option<&'a RawValue>,
    #[serde(rename = "io.modelcontextprotocol/logLevel", borrow, default)]
    log_level: Option<&'a RawValue>,
}

/// Checks the mirrored request headers against the body (SEP-2243). Returns
/// whether the request asks for notifications (a progress token or a log
/// level), which need an SSE response.
fn validate_headers(
    server: &Server,
    headers: &HeaderMap,
    request: &jsonrpc::Request,
) -> Result<bool, McpError> {
    let version = single_header(headers, MCP_PROTOCOL_VERSION)?
        .ok_or_else(|| McpError::header_mismatch("Missing MCP-Protocol-Version header"))?;
    let method = single_header(headers, MCP_METHOD)?
        .ok_or_else(|| McpError::header_mismatch("Missing Mcp-Method header"))?;
    if method != request.method {
        return Err(McpError::header_mismatch(
            "Mcp-Method header does not match the request method",
        ));
    }

    // When the params cannot be read, the server reports that itself.
    let Ok(params) = serde_json::from_str::<PeekParams<'_>>(
        request.params.as_deref().map_or("{}", RawValue::get),
    ) else {
        return Ok(false);
    };
    let wants_notifications = params
        .meta
        .as_ref()
        .is_some_and(|m| m.progress_token.is_some() || m.log_level.is_some());

    if let Some(body_version) = params
        .meta
        .as_ref()
        .and_then(|m| m.protocol_version.as_deref())
        && body_version != version
    {
        return Err(McpError::header_mismatch(
            "MCP-Protocol-Version header does not match `_meta`",
        ));
    }

    let name_source = match request.method.as_str() {
        methods::TOOLS_CALL | methods::PROMPTS_GET => Some(params.name.as_deref()),
        methods::RESOURCES_READ => Some(params.uri.as_deref()),
        _ => None,
    };
    if let Some(body_name) = name_source {
        let header_name = single_header(headers, MCP_NAME)?
            .ok_or_else(|| McpError::header_mismatch("Missing Mcp-Name header"))?;
        if Some(decode_header_value(header_name)?.as_ref()) != body_name {
            return Err(McpError::header_mismatch(
                "Mcp-Name header does not match the request body",
            ));
        }
    }

    if request.method == methods::TOOLS_CALL
        && let Some(name) = params.name.as_deref()
    {
        validate_param_headers(server, headers, name, params.arguments)?;
    }
    Ok(wants_notifications)
}

fn validate_param_headers(
    server: &Server,
    headers: &HeaderMap,
    tool: &str,
    arguments: Option<&RawValue>,
) -> Result<(), McpError> {
    let Some(mirrored) = server.mirrored_params(tool).filter(|m| !m.is_empty()) else {
        return Ok(());
    };
    let arguments: Value = match arguments {
        Some(raw) => serde_json::from_str(raw.get()).unwrap_or(Value::Null),
        None => Value::Null,
    };
    for param in mirrored {
        let header_name = format!("{MCP_PARAM_PREFIX}{}", param.header().to_ascii_lowercase());
        let header = single_header(headers, &header_name)?;
        let value = param.value_in(&arguments).filter(|v| !v.is_null());
        let matches = match (value, header) {
            (None, None) => true,
            (None, Some(_)) | (Some(_), None) => false,
            (Some(value), Some(header)) => {
                let header = decode_header_value(header)?;
                match param.kind() {
                    ParamKind::String => value.as_str() == Some(header.as_ref()),
                    ParamKind::Boolean => {
                        value.as_bool().map(|b| if b { "true" } else { "false" })
                            == Some(header.as_ref())
                    }
                    ParamKind::Integer => integers_equal(value, &header),
                }
            }
        };
        if !matches {
            return Err(McpError::header_mismatch(format!(
                "Mcp-Param-{} header does not match the request body",
                param.header()
            )));
        }
    }
    Ok(())
}

/// Compares a JSON integer with a header value exactly (no floating-point
/// rounding). `42.0` is accepted for `42` within the exactly representable
/// range of `f64`.
fn integers_equal(value: &Value, header: &str) -> bool {
    const EXACT_F64: f64 = 9_007_199_254_740_992.0; // 2^53
    let as_integer = |text: &str, float: Option<f64>| -> Option<i128> {
        text.parse::<i128>().ok().or_else(|| {
            float
                .filter(|f| f.fract() == 0.0 && f.abs() <= EXACT_F64)
                // The guard above keeps the value within i128 exactly.
                .map(|f| f as i128)
        })
    };
    let body = match value {
        Value::Number(n) => as_integer(&n.to_string(), n.as_f64()),
        _ => None,
    };
    let header = as_integer(header, header.parse::<f64>().ok());
    matches!((body, header), (Some(a), Some(b)) if a == b)
}

/// Returns the single value of a header; duplicates and non-visible-ASCII
/// values are rejected.
fn single_header<'h>(headers: &'h HeaderMap, name: &str) -> Result<Option<&'h str>, McpError> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(McpError::header_mismatch("Duplicate MCP header"));
    }
    first
        .to_str()
        .map(Some)
        .map_err(|_| McpError::header_mismatch("MCP header contains invalid characters"))
}

/// Decodes the `=?base64?…?=` sentinel encoding.
fn decode_header_value(value: &str) -> Result<Cow<'_, str>, McpError> {
    match value
        .strip_prefix(BASE64_PREFIX)
        .and_then(|v| v.strip_suffix(BASE64_SUFFIX))
    {
        None => Ok(Cow::Borrowed(value)),
        Some(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(Cow::Owned)
            .ok_or_else(|| McpError::header_mismatch("Invalid base64-encoded MCP header")),
    }
}

fn status_for(error: &McpError) -> StatusCode {
    match error.kind() {
        ErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        ErrorKind::MethodNotFound => StatusCode::NOT_FOUND,
        ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        _ => StatusCode::OK,
    }
}

fn origin_allowed(allowed: &AllowedOrigins, origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    match allowed {
        AllowedOrigins::Any => true,
        AllowedOrigins::List(list) => list.iter().any(|o| o.eq_ignore_ascii_case(origin)),
        AllowedOrigins::Localhost => {
            let Some((scheme, rest)) = origin.split_once("://") else {
                return false;
            };
            matches!(scheme, "http" | "https") && is_localhost(strip_port(rest))
        }
    }
}

fn host_allowed(allowed: &AllowedHosts, parts: &http::request::Parts) -> bool {
    // RFC 9112 §3.2: a request with several `Host` headers is invalid. Only
    // the first would be checked, while an intermediary might use another.
    if parts.headers.get_all(header::HOST).iter().nth(1).is_some() {
        return false;
    }
    if *allowed == AllowedHosts::Any {
        return true;
    }
    // HTTP/1 sends `Host`; HTTP/2 carries the host in the URI authority.
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| parts.uri.authority().map(http::uri::Authority::as_str));
    let Some(host) = host else {
        return false;
    };
    host_name_allowed(allowed, strip_port(host))
}

/// Whether a host name (without port) is allowed.
fn host_name_allowed(allowed: &AllowedHosts, host: &str) -> bool {
    match allowed {
        AllowedHosts::Any => true,
        AllowedHosts::List(list) => list.iter().any(|h| h.eq_ignore_ascii_case(host)),
        AllowedHosts::Localhost => is_localhost(host),
    }
}

pub(crate) fn strip_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        // `[::1]:8080` → `[::1]`
        return authority
            .find(']')
            .and_then(|end| authority.get(..=end))
            .unwrap_or(authority);
    }
    authority.split(':').next().unwrap_or(authority)
}

pub(crate) fn is_localhost(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
}

fn is_json_content_type(value: Option<&HeaderValue>) -> bool {
    value
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// The response is JSON, so `Accept` (when present) must allow it.
fn accepts_json(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::ACCEPT).iter().peekable();
    if values.peek().is_none() {
        return true;
    }
    values
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|item| item.split(';').next())
        .map(str::trim)
        .any(|mime| {
            mime.eq_ignore_ascii_case("application/json")
                || mime.eq_ignore_ascii_case("application/*")
                || mime == "*/*"
        })
}

fn preflight(origin: Option<HeaderValue>, request_headers: &HeaderMap) -> Response<HttpBody> {
    let mut response = plain(StatusCode::NO_CONTENT, origin.as_ref());
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST"),
    );
    let allowed = request_headers
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static(ALLOWED_REQUEST_HEADERS));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, allowed);
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    response
}

fn with_cors(response: &mut Response<HttpBody>, origin: Option<&HeaderValue>) {
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        // Browser clients must read OAuth challenges and legacy sessions.
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("www-authenticate, mcp-session-id"),
        );
    }
}

/// Why a request body could not be read.
enum BodyError {
    /// Over the message limit (`413`).
    TooLarge,
    /// The memory budget for bodies is spent (`503`).
    Busy,
    /// The connection failed while reading (`400`).
    Read,
}

/// Reads a body up to `limit` bytes, reserving each frame's size in
/// `budget` as it arrives. Returns the body and its reservation.
async fn read_body<B>(
    body: B,
    limit: usize,
    budget: Arc<Semaphore>,
) -> Result<(Bytes, Option<OwnedSemaphorePermit>), BodyError>
where
    B: Body,
{
    let mut body = std::pin::pin!(body);
    let mut buffer = bytes::BytesMut::new();
    let mut reserved: Option<OwnedSemaphorePermit> = None;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BodyError::Read)?;
        let Ok(data) = frame.into_data() else {
            continue; // trailers
        };
        let len = bytes::Buf::remaining(&data);
        if len == 0 {
            continue;
        }
        if buffer.len().saturating_add(len) > limit {
            return Err(BodyError::TooLarge);
        }
        let size = u32::try_from(len).map_err(|_| BodyError::TooLarge)?;
        let permit = Arc::clone(&budget)
            .try_acquire_many_owned(size)
            .map_err(|_| BodyError::Busy)?;
        match &mut reserved {
            Some(held) => held.merge(permit),
            None => reserved = Some(permit),
        }
        bytes::BufMut::put(&mut buffer, data);
    }
    Ok((buffer.freeze(), reserved))
}

/// `503` with `Retry-After`: the server is at capacity.
fn busy(cors: Option<&HeaderValue>) -> Response<HttpBody> {
    let mut response = rpc_error(
        StatusCode::SERVICE_UNAVAILABLE,
        None,
        &McpError::internal_with_message("The server is busy; retry later."),
        cors,
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

/// A `401`/`403`/`400`/`503` answer to a request that failed authorization.
fn challenge_response(
    oauth: &OAuth,
    challenge: &Challenge,
    cors: Option<&HeaderValue>,
) -> Response<HttpBody> {
    let error = match challenge {
        Challenge::Unavailable => McpError::internal_with_message(challenge.message()),
        _ => McpError::invalid_request(challenge.message()),
    };
    let mut response = rpc_error(challenge.status(), None, &error, cors);
    if let Some(value) = oauth.www_authenticate(challenge) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    if *challenge == Challenge::Unavailable {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    }
    response
}

fn plain(status: StatusCode, cors: Option<&HeaderValue>) -> Response<HttpBody> {
    let mut response = Response::new(HttpBody::full(Bytes::new()));
    *response.status_mut() = status;
    with_cors(&mut response, cors);
    response
}

fn json(status: StatusCode, body: Vec<u8>, cors: Option<&HeaderValue>) -> Response<HttpBody> {
    let mut response = Response::new(HttpBody::full(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    with_cors(&mut response, cors);
    response
}

fn rpc_error(
    status: StatusCode,
    id: Option<RequestId>,
    error: &McpError,
    cors: Option<&HeaderValue>,
) -> Response<HttpBody> {
    json(status, RpcResponse::error(id, error.clone()).to_vec(), cors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_sentinel() {
        assert_eq!(decode_header_value("us-west1").unwrap(), "us-west1");
        assert_eq!(
            decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?=").unwrap(),
            "Hello, 世界"
        );
        assert_eq!(
            decode_header_value("=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?=").unwrap(),
            "=?base64?literal?="
        );
        assert!(decode_header_value("=?base64?***?=").is_err());
    }

    #[test]
    fn origin_rules() {
        let localhost = AllowedOrigins::Localhost;
        let ok = |o: &'static str| origin_allowed(&localhost, &HeaderValue::from_static(o));
        assert!(ok("http://localhost:6274"));
        assert!(ok("http://127.0.0.1"));
        assert!(ok("https://[::1]:8443"));
        assert!(!ok("http://localhost.evil.com"));
        assert!(!ok("http://evil.com"));
        assert!(!ok("null"));
    }

    fn parts(host: Option<&'static str>, uri: &str) -> http::request::Parts {
        let mut builder = Request::builder().uri(uri);
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[test]
    fn host_rules() {
        let local = parts(Some("127.0.0.1:8000"), "/mcp");
        assert!(host_allowed(&AllowedHosts::Localhost, &local));
        let evil = parts(Some("attacker.example"), "/mcp");
        assert!(!host_allowed(&AllowedHosts::Localhost, &evil));
        assert!(host_allowed(
            &AllowedHosts::List(vec!["attacker.example".into()]),
            &evil
        ));
        // HTTP/2: no Host header, authority in the URI.
        let h2 = parts(None, "http://localhost:8000/mcp");
        assert!(host_allowed(&AllowedHosts::Localhost, &h2));
        assert!(!host_allowed(
            &AllowedHosts::Localhost,
            &parts(None, "/mcp")
        ));
    }

    #[test]
    fn accept_rules() {
        let mut headers = HeaderMap::new();
        assert!(accepts_json(&headers));
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        assert!(accepts_json(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        assert!(!accepts_json(&headers));
    }
}
