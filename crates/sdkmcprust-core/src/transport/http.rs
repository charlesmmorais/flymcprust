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
//! 4. The body is read up to the message limit (413).
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
use tokio_util::sync::CancellationToken;

use crate::context::TransportContext;
use crate::error::{ErrorKind, McpError};
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
/// Time allowed to receive a request's headers (slow-loris protection).
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            origins: AllowedOrigins::Localhost,
            hosts: AllowedHosts::Localhost,
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
}

/// Streamable HTTP as a `tower::Service`.
#[derive(Clone, Debug)]
pub struct StreamableHttpService {
    server: Server,
    config: Arc<HttpConfig>,
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
            config: Arc::new(config),
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
            Some(_) => {
                return rpc_error(
                    StatusCode::FORBIDDEN,
                    None,
                    &McpError::invalid_request("Origin not allowed"),
                    None,
                );
            }
        };
        if !host_allowed(&self.config.hosts, &parts) {
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

        let limits = self.server.limits();
        let bytes = match Limited::new(body, limits.max_message_bytes).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => {
                let (status, message) = if error.downcast_ref::<LengthLimitError>().is_some() {
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

        if let Message::Request(request) = &message
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
    pub async fn serve_http(self, addr: impl tokio::net::ToSocketAddrs) -> io::Result<()> {
        let listener = TcpListener::bind(addr).await?;
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
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto;
    use hyper_util::server::graceful::GracefulShutdown;

    if let Ok(addr) = listener.local_addr()
        && !addr.ip().is_loopback()
        && service.config.hosts == AllowedHosts::Localhost
    {
        tracing::warn!(
            %addr,
            "listening on a non-loopback address while only localhost Host headers are allowed"
        );
    }

    let path: Arc<str> = Arc::from(path);
    let grace = service.server.limits().tool_timeout;
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);

    loop {
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
        let connection =
            builder.serve_connection_with_upgrades(TokioIo::new(stream), hyper_service);
        let connection = graceful.watch(connection.into_owned());
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%peer, %error, "connection closed with an error");
            }
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
                    // Compared numerically: `42` and `42.0` are equal.
                    ParamKind::Integer => match (value.as_f64(), header.parse::<f64>()) {
                        (Some(a), Ok(b)) => (a - b).abs() < f64::EPSILON,
                        _ => false,
                    },
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
