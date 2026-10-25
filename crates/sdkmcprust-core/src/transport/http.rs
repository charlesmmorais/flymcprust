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
//! 4. At most [`HttpConfig::max_in_flight`] requests are processed at once
//!    (503 beyond that); the body is read up to the message limit (413)
//!    within [`HttpConfig::body_timeout`] (408).
//! 5. `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` and recognized
//!    `Mcp-Param-*` headers must match the body (400, `-32020`).
//! 6. The message is dispatched; notifications get `202`, unknown methods
//!    `404`, malformed requests `400`, everything else `200`.
//!
//! `Mcp-Session-Id` and `Last-Event-ID` are ignored: the protocol is
//! stateless and streams are not resumable. Dropping the response future
//! (client disconnect) cancels the request.

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
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::context::TransportContext;
use crate::error::{ErrorKind, McpError};
use crate::hints::{self, Once};
use crate::jsonrpc::{self, Message, Rejection, Response as RpcResponse};
use crate::mirror::ParamKind;
use crate::server::{McpRequest, Server};
use crate::spec::{RequestId, methods};

/// The HTTP response body type.
pub type HttpBody = Full<Bytes>;

const MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const MCP_METHOD: &str = "mcp-method";
const MCP_NAME: &str = "mcp-name";
const MCP_PARAM_PREFIX: &str = "mcp-param-";
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    origins: AllowedOrigins,
    hosts: AllowedHosts,
    header_timeout: Duration,
    body_timeout: Duration,
    max_in_flight: usize,
    max_connections: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            origins: AllowedOrigins::Localhost,
            hosts: AllowedHosts::Localhost,
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(30),
            max_in_flight: 256,
            max_connections: 1024,
        }
    }
}

impl HttpConfig {
    /// Secure defaults: localhost origins and hosts only.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Maximum number of requests being read or processed at once, which
    /// bounds the memory used for request bodies (each up to the message
    /// limit). Further requests get `503`. Default: 256.
    #[must_use]
    pub fn max_in_flight(mut self, requests: usize) -> Self {
        self.max_in_flight = requests.max(1);
        self
    }

    /// Maximum number of open connections in [`serve`]; further connections
    /// wait in the operating system's accept queue. Default: 1024.
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
    hints: Arc<Hints>,
}

/// Which developer hints were already logged.
#[derive(Debug, Default)]
struct Hints {
    origin: Once,
    host: Once,
    busy: Once,
    slow_body: Once,
    too_large: Once,
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
        Self {
            server,
            in_flight: Arc::new(Semaphore::new(config.max_in_flight)),
            config: Arc::new(config),
            hints: Arc::default(),
        }
    }

    async fn handle<B>(self, request: Request<B>) -> Response<HttpBody>
    where
        B: Body,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let (parts, body) = request.into_parts();

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
        let Ok(_permit) = Arc::clone(&self.in_flight).try_acquire_owned() else {
            hints::warn_once(
                &self.hints.busy,
                format_args!(
                    "rejected a request: {} requests already in flight (HTTP 503)",
                    self.config.max_in_flight
                ),
                "raise HttpConfig::max_in_flight(..) if the machine has room, or run more \
                 instances",
            );
            let mut response = rpc_error(
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                &McpError::internal_with_message("The server is busy; retry later."),
                cors.as_ref(),
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return response;
        };

        let limits = self.server.limits();
        let read = Limited::new(body, limits.max_message_bytes).collect();
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
        let bytes = match read {
            Ok(collected) => collected.to_bytes(),
            Err(error) => {
                let (status, message) = if error.downcast_ref::<LengthLimitError>().is_some() {
                    hints::warn_once(
                        &self.hints.too_large,
                        format_args!(
                            "rejected a request body over the {}-byte message limit (HTTP 413)",
                            limits.max_message_bytes
                        ),
                        "raise ServerBuilder::max_message_bytes(..) if clients legitimately \
                         send larger arguments",
                    );
                    (StatusCode::PAYLOAD_TOO_LARGE, "Message too large")
                } else {
                    (StatusCode::BAD_REQUEST, "Could not read the request body")
                };
                return rpc_error(
                    status,
                    None,
                    &McpError::invalid_request(message),
                    cors.as_ref(),
                );
            }
        };

        let message = match jsonrpc::parse(&bytes, limits) {
            Ok(message) => message,
            Err(Rejection::Error { id, error }) => {
                return rpc_error(status_for(&error), id, &error, cors.as_ref());
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

        // A legacy client's `initialize` carries none of the modern headers;
        // let the server answer it with an error naming the supported
        // versions (the spec recommends that diagnostic for legacy clients).
        if let Message::Request(request) = &message
            && request.method != "initialize"
            && let Err(error) = validate_headers(&self.server, &parts.headers, request)
        {
            return rpc_error(
                StatusCode::BAD_REQUEST,
                Some(request.id.clone()),
                &error,
                cors.as_ref(),
            );
        }

        let mut transport = TransportContext::new();
        transport.insert(parts);
        let response = self
            .server
            .handle(McpRequest {
                message,
                transport,
                cancellation: CancellationToken::new(),
            })
            .await;

        match response {
            None => plain(StatusCode::ACCEPTED, cors.as_ref()),
            Some(response) => {
                let status = response.as_error().map_or(StatusCode::OK, status_for);
                json(status, response.to_vec(), cors.as_ref())
            }
        }
    }
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
        let hyper_service =
            hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                let mut service = service.clone();
                let path = Arc::clone(&path);
                async move {
                    if request.uri().path() == &*path {
                        tower_service::Service::call(&mut service, request).await
                    } else {
                        Ok(plain(StatusCode::NOT_FOUND, None))
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
}

/// Checks the mirrored request headers against the body (SEP-2243).
fn validate_headers(
    server: &Server,
    headers: &HeaderMap,
    request: &jsonrpc::Request,
) -> Result<(), McpError> {
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
        return Ok(());
    };

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
    Ok(())
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
    let host = strip_port(host);
    match allowed {
        AllowedHosts::List(list) => list.iter().any(|h| h.eq_ignore_ascii_case(host)),
        _ => is_localhost(host),
    }
}

fn strip_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        // `[::1]:8080` → `[::1]`
        return authority
            .find(']')
            .and_then(|end| authority.get(..=end))
            .unwrap_or(authority);
    }
    authority.split(':').next().unwrap_or(authority)
}

fn is_localhost(host: &str) -> bool {
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
    }
}

fn plain(status: StatusCode, cors: Option<&HeaderValue>) -> Response<HttpBody> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    with_cors(&mut response, cors);
    response
}

fn json(status: StatusCode, body: Vec<u8>, cors: Option<&HeaderValue>) -> Response<HttpBody> {
    let mut response = Response::new(Full::new(Bytes::from(body)));
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
