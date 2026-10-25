//! OAuth 2.1 resource server for the Streamable HTTP transport.
//!
//! With [`OAuth`] in [`HttpConfig`](super::http::HttpConfig::oauth), every
//! request needs `Authorization: Bearer <token>`:
//!
//! - no token: `401` with `WWW-Authenticate: Bearer resource_metadata="…"`
//!   (plus the scopes to ask for), which starts the client's OAuth flow;
//! - an invalid or expired token: `401` with `error="invalid_token"`;
//! - a valid token without the scopes the request needs: `403` with
//!   `error="insufficient_scope"` and every scope the request needs.
//!
//! The Protected Resource Metadata document (RFC 9728) tells clients which
//! authorization server issues tokens for this server. [`serve`](super::http::serve)
//! publishes it at the well-known URIs; other frameworks mount
//! [`OAuth::metadata_service`] at [`OAuth::metadata_paths`].

use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderValue};
use http::{Method, Request, Response, StatusCode};
use serde_json::{Map, Value, json};

use super::http::HttpBody;
use crate::auth::{AuthContext, ScopeImplies, TokenVerifier};

/// The well-known URI suffix of RFC 9728.
const WELL_KNOWN: &str = "/.well-known/oauth-protected-resource";

/// Longest bearer token accepted (larger ones are rejected unverified).
const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// Default time allowed to verify a token.
const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// OAuth 2.1 resource-server settings. Build with [`OAuth::builder`].
///
/// Cheap to clone.
#[derive(Clone)]
pub struct OAuth {
    inner: Arc<Inner>,
}

struct Inner {
    resource: String,
    metadata_url: String,
    metadata_paths: Vec<String>,
    metadata_body: Bytes,
    /// `scope` of the `401` challenge.
    challenge_scope: Option<String>,
    required_scopes: Vec<String>,
    verifier: Box<dyn TokenVerifier>,
    implies: Option<ScopeImplies>,
    verify_timeout: Duration,
}

impl fmt::Debug for OAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth")
            .field("resource", &self.inner.resource)
            .field("metadata_url", &self.inner.metadata_url)
            .field("required_scopes", &self.inner.required_scopes)
            .finish_non_exhaustive()
    }
}

impl OAuth {
    /// Starts the settings of the MCP server identified by `resource`, its
    /// canonical URI (for example `https://mcp.example.com/mcp`, the URL
    /// clients connect to). Tokens are checked by `verifier`, which must
    /// only accept tokens issued for `resource` (audience).
    #[must_use]
    pub fn builder(resource: impl Into<String>, verifier: impl TokenVerifier) -> OAuthBuilder {
        OAuthBuilder {
            resource: resource.into(),
            verifier: Box::new(verifier),
            authorization_servers: Vec::new(),
            scopes_supported: Vec::new(),
            required_scopes: Vec::new(),
            implies: None,
            resource_name: None,
            verify_timeout: DEFAULT_VERIFY_TIMEOUT,
        }
    }

    /// The canonical URI of the server.
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.inner.resource
    }

    /// The URL of the Protected Resource Metadata document, sent in
    /// `WWW-Authenticate` challenges.
    #[must_use]
    pub fn metadata_url(&self) -> &str {
        &self.inner.metadata_url
    }

    /// The paths at which clients look for the metadata document: the
    /// path-specific well-known URI first, then the root one.
    #[must_use]
    pub fn metadata_paths(&self) -> &[String] {
        &self.inner.metadata_paths
    }

    /// A `tower::Service` that answers with the metadata document. Mount it
    /// at each of [`metadata_paths`](Self::metadata_paths) when not using
    /// [`serve`](super::http::serve).
    #[must_use]
    pub fn metadata_service(&self) -> ProtectedResourceMetadata {
        ProtectedResourceMetadata {
            body: self.inner.metadata_body.clone(),
        }
    }

    /// Authenticates a request from its headers.
    pub(crate) async fn authenticate(&self, headers: &HeaderMap) -> Result<AuthContext, Challenge> {
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let Some(value) = values.next() else {
            return Err(Challenge::Missing);
        };
        if values.next().is_some() {
            return Err(Challenge::Malformed);
        }
        let Some((scheme, token)) = value.to_str().ok().and_then(|v| v.split_once(' ')) else {
            return Err(Challenge::Missing);
        };
        if !scheme.eq_ignore_ascii_case("bearer") {
            // Another scheme: tell the client which one we expect.
            return Err(Challenge::Missing);
        }
        let token = token.trim_matches(' ');
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES || !is_b64token(token) {
            return Err(Challenge::InvalidToken);
        }
        let verified =
            tokio::time::timeout(self.inner.verify_timeout, self.inner.verifier.verify(token))
                .await;
        let principal = match verified {
            Ok(Ok(principal)) => principal,
            Ok(Err(error)) => {
                tracing::debug!(reason = error.reason(), "rejected an access token");
                return Err(Challenge::InvalidToken);
            }
            Err(_elapsed) => {
                tracing::warn!(
                    "token verification took longer than {:?}\n  = help: the verifier may be \
                     waiting on the authorization server; raise OAuthBuilder::verify_timeout(..) \
                     or check its availability",
                    self.inner.verify_timeout
                );
                return Err(Challenge::Unavailable);
            }
        };
        let auth = AuthContext::new(principal, self.inner.implies);
        let missing = auth.missing(&self.inner.required_scopes);
        if !missing.is_empty() {
            tracing::debug!(
                ?missing,
                "an access token lacks the server's required scopes"
            );
            return Err(Challenge::InsufficientScope(
                self.inner.required_scopes.join(" "),
            ));
        }
        Ok(auth)
    }

    /// The `WWW-Authenticate` value for a challenge.
    pub(crate) fn www_authenticate(&self, challenge: &Challenge) -> Option<HeaderValue> {
        let mut value = format!("Bearer resource_metadata=\"{}\"", self.inner.metadata_url);
        match challenge {
            Challenge::Missing | Challenge::Malformed | Challenge::Unavailable => {
                if let Some(scope) = &self.inner.challenge_scope {
                    value.push_str(&format!(", scope=\"{scope}\""));
                }
                if matches!(challenge, Challenge::Malformed) {
                    value.push_str(", error=\"invalid_request\"");
                }
            }
            Challenge::InvalidToken => {
                if let Some(scope) = &self.inner.challenge_scope {
                    value.push_str(&format!(", scope=\"{scope}\""));
                }
                value.push_str(", error=\"invalid_token\"");
            }
            Challenge::InsufficientScope(scope) => {
                value.push_str(&format!(
                    ", scope=\"{scope}\", error=\"insufficient_scope\""
                ));
            }
        }
        HeaderValue::from_str(&value).ok()
    }
}

/// Why a request was not authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Challenge {
    /// No bearer token (`401`).
    Missing,
    /// Several `Authorization` headers (`400`).
    Malformed,
    /// The token failed verification (`401`).
    InvalidToken,
    /// The token lacks these space-separated scopes (`403`).
    InsufficientScope(String),
    /// The verifier did not answer in time (`503`).
    Unavailable,
}

impl Challenge {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Missing | Self::InvalidToken => StatusCode::UNAUTHORIZED,
            Self::Malformed => StatusCode::BAD_REQUEST,
            Self::InsufficientScope(_) => StatusCode::FORBIDDEN,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The client-facing message of the JSON-RPC error body.
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Missing => "Authorization required",
            Self::Malformed => "Malformed Authorization header",
            Self::InvalidToken => "Invalid or expired access token",
            Self::InsufficientScope(_) => "Insufficient scope",
            Self::Unavailable => "Authorization is temporarily unavailable; retry later.",
        }
    }
}

/// Builder of [`OAuth`].
pub struct OAuthBuilder {
    resource: String,
    verifier: Box<dyn TokenVerifier>,
    authorization_servers: Vec<String>,
    scopes_supported: Vec<String>,
    required_scopes: Vec<String>,
    implies: Option<ScopeImplies>,
    resource_name: Option<String>,
    verify_timeout: Duration,
}

impl fmt::Debug for OAuthBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthBuilder")
            .field("resource", &self.resource)
            .field("authorization_servers", &self.authorization_servers)
            .finish_non_exhaustive()
    }
}

impl OAuthBuilder {
    /// Adds an authorization server (its issuer URL) that issues tokens for
    /// this server. At least one is required.
    #[must_use]
    pub fn authorization_server(mut self, issuer: impl Into<String>) -> Self {
        self.authorization_servers.push(issuer.into());
        self
    }

    /// The scopes listed in the metadata document (`scopes_supported`): the
    /// minimal set for basic use. Handler scopes are requested later, when
    /// a `403` asks for them.
    #[must_use]
    pub fn scopes_supported<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes_supported = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Scopes every request needs; tokens without them get `403`.
    #[must_use]
    pub fn required_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.required_scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Declares a scope hierarchy: `implies(granted, required)` returns
    /// whether a granted scope satisfies a required one (for example,
    /// `files` implying `files:read`). The specification requires servers
    /// to honor their hierarchies.
    #[must_use]
    pub fn scope_implies(mut self, implies: ScopeImplies) -> Self {
        self.implies = Some(implies);
        self
    }

    /// A human-readable name shown by clients (`resource_name`).
    #[must_use]
    pub fn resource_name(mut self, name: impl Into<String>) -> Self {
        self.resource_name = Some(name.into());
        self
    }

    /// Time allowed to verify a token (including fetching signing keys).
    /// Slower verifications get `503`. Default: 10 s.
    #[must_use]
    pub fn verify_timeout(mut self, timeout: Duration) -> Self {
        self.verify_timeout = timeout;
        self
    }

    /// Validates the settings.
    ///
    /// # Errors
    ///
    /// Fails when the resource or an authorization server is not a valid
    /// URL (`https`, or `http` on localhost), when no authorization server
    /// is given, or when a scope is malformed.
    pub fn build(self) -> Result<OAuth, OAuthError> {
        let resource =
            canonical_url(&self.resource).map_err(|reason| OAuthError::InvalidResource {
                resource: self.resource.clone(),
                reason,
            })?;
        if self.authorization_servers.is_empty() {
            return Err(OAuthError::NoAuthorizationServer);
        }
        for issuer in &self.authorization_servers {
            canonical_url(issuer).map_err(|reason| OAuthError::InvalidAuthorizationServer {
                issuer: issuer.clone(),
                reason,
            })?;
        }
        for scope in self.scopes_supported.iter().chain(&self.required_scopes) {
            if !crate::auth::is_valid_scope(scope) {
                return Err(OAuthError::InvalidScope(scope.clone()));
            }
            if scope == "offline_access" {
                return Err(OAuthError::OfflineAccess);
            }
        }
        if self.verify_timeout.is_zero() {
            return Err(OAuthError::InvalidTimeout);
        }

        let (origin, path) = split_origin(&resource);
        let path = path.trim_end_matches('/');
        let metadata_url = format!("{origin}{WELL_KNOWN}{path}");
        let mut metadata_paths = vec![format!("{WELL_KNOWN}{path}")];
        if !path.is_empty() {
            metadata_paths.push(WELL_KNOWN.to_owned());
        }

        let mut metadata = Map::new();
        metadata.insert("resource".to_owned(), Value::from(resource.clone()));
        metadata.insert(
            "authorization_servers".to_owned(),
            json!(self.authorization_servers),
        );
        if !self.scopes_supported.is_empty() {
            metadata.insert("scopes_supported".to_owned(), json!(self.scopes_supported));
        }
        metadata.insert("bearer_methods_supported".to_owned(), json!(["header"]));
        if let Some(name) = self.resource_name {
            metadata.insert("resource_name".to_owned(), Value::from(name));
        }
        let metadata_body = serde_json::to_vec(&Value::Object(metadata))
            .map_err(|_| OAuthError::InvalidResource {
                resource: resource.clone(),
                reason: "could not serialize the metadata".to_owned(),
            })?
            .into();

        let challenge = if self.required_scopes.is_empty() {
            &self.scopes_supported
        } else {
            &self.required_scopes
        };
        let challenge_scope = (!challenge.is_empty()).then(|| challenge.join(" "));

        Ok(OAuth {
            inner: Arc::new(Inner {
                resource,
                metadata_url,
                metadata_paths,
                metadata_body,
                challenge_scope,
                required_scopes: self.required_scopes,
                verifier: self.verifier,
                implies: self.implies,
                verify_timeout: self.verify_timeout,
            }),
        })
    }
}

/// Invalid [`OAuth`] settings.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OAuthError {
    /// The resource is not a valid canonical URI.
    #[error("invalid resource `{resource}`: {reason}")]
    InvalidResource {
        /// The configured value.
        resource: String,
        /// What is wrong with it.
        reason: String,
    },
    /// An authorization server URL is invalid.
    #[error("invalid authorization server `{issuer}`: {reason}")]
    InvalidAuthorizationServer {
        /// The configured value.
        issuer: String,
        /// What is wrong with it.
        reason: String,
    },
    /// No authorization server was given.
    #[error("no authorization server")]
    NoAuthorizationServer,
    /// A scope contains characters OAuth does not allow.
    #[error("invalid scope `{0}`")]
    InvalidScope(String),
    /// `offline_access` was listed.
    #[error("`offline_access` is not a resource scope")]
    OfflineAccess,
    /// The verification timeout is zero.
    #[error("the token verification timeout must be positive")]
    InvalidTimeout,
}

impl OAuthError {
    /// How to fix the error.
    #[must_use]
    pub fn help(&self) -> &'static str {
        match self {
            Self::InvalidResource { .. } => {
                "use the URL clients connect to, such as `https://mcp.example.com/mcp`: https \
                 (http only on localhost), no fragment, no user name"
            }
            Self::InvalidAuthorizationServer { .. } => {
                "use the issuer URL of the authorization server, such as \
                 `https://auth.example.com` (https; http only on localhost)"
            }
            Self::NoAuthorizationServer => {
                "add the issuer that grants tokens for this server with \
                 `.authorization_server(\"https://auth.example.com\")`"
            }
            Self::InvalidScope(_) => {
                "scopes are non-empty and use visible ASCII except `\"` and `\\`, without spaces"
            }
            Self::OfflineAccess => {
                "refresh tokens are the client's concern: remove `offline_access` (the \
                 specification says servers should not list it)"
            }
            Self::InvalidTimeout => "use a timeout such as `Duration::from_secs(10)`",
        }
    }
}

impl fmt::Debug for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}\n  = help: {}", self.help())
    }
}

/// Serves the Protected Resource Metadata document (RFC 9728) on `GET`.
/// The document is public, so any origin may read it.
#[derive(Clone, Debug)]
pub struct ProtectedResourceMetadata {
    body: Bytes,
}

impl ProtectedResourceMetadata {
    pub(crate) fn respond(&self, method: &Method) -> Response<HttpBody> {
        let mut response = match *method {
            Method::GET | Method::HEAD => {
                let mut response = Response::new(HttpBody::full(self.body.clone()));
                let headers = response.headers_mut();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                headers.insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600"),
                );
                response
            }
            Method::OPTIONS => {
                let mut response = Response::new(HttpBody::full(Bytes::new()));
                *response.status_mut() = StatusCode::NO_CONTENT;
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET"),
                );
                response
            }
            _ => {
                let mut response = Response::new(HttpBody::full(Bytes::new()));
                *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
                response
                    .headers_mut()
                    .insert(header::ALLOW, HeaderValue::from_static("GET"));
                response
            }
        };
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        response
    }
}

impl<B> tower_service::Service<Request<B>> for ProtectedResourceMetadata {
    type Response = Response<HttpBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let response = self.respond(request.method());
        Box::pin(async move { Ok(response) })
    }
}

/// RFC 6750 `b64token`: the characters a bearer token may contain.
fn is_b64token(token: &str) -> bool {
    let body = token.trim_end_matches('=');
    !body.is_empty()
        && body.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        })
}

/// Validates an absolute `https` URL (or `http` on localhost) and lowercases
/// its scheme and host, as RFC 8707 canonical URIs are.
pub(crate) fn canonical_url(url: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "missing scheme (expected `https://`)".to_owned())?;
    let scheme = scheme.to_ascii_lowercase();
    if url.contains('#') {
        return Err("must not contain a fragment".to_owned());
    }
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
    {
        return Err("contains spaces, quotes or control characters".to_owned());
    }
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() {
        return Err("missing host".to_owned());
    }
    if authority.contains('@') {
        return Err("must not contain a user name".to_owned());
    }
    let authority = authority.to_ascii_lowercase();
    let host = super::http::strip_port(&authority);
    match scheme.as_str() {
        "https" => {}
        "http" if super::http::is_localhost(host) => {}
        "http" => return Err("must use https (http is only allowed on localhost)".to_owned()),
        _ => return Err("must use https".to_owned()),
    }
    Ok(format!("{scheme}://{authority}{tail}"))
}

/// Splits a canonical URL into origin and path (without the query).
fn split_origin(url: &str) -> (&str, &str) {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    let path_start = url
        .get(after_scheme..)
        .and_then(|rest| rest.find(['/', '?']))
        .map_or(url.len(), |i| i + after_scheme);
    let (origin, rest) = url.split_at(path_start);
    let path = rest.split('?').next().unwrap_or_default();
    (origin, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Principal, TokenError};

    fn verifier(token: &str) -> Result<Principal, TokenError> {
        match token {
            "good" => Ok(Principal::new("alice").with_scopes(["mcp"])),
            "bare" => Ok(Principal::new("bob")),
            _ => Err(TokenError::new("unknown token")),
        }
    }

    fn oauth() -> OAuth {
        OAuth::builder("https://MCP.example.com/mcp", verifier)
            .authorization_server("https://auth.example.com")
            .scopes_supported(["mcp"])
            .required_scopes(["mcp"])
            .build()
            .unwrap()
    }

    fn headers(auth: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in auth {
            headers.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn metadata_locations() {
        let oauth = oauth();
        assert_eq!(oauth.resource(), "https://mcp.example.com/mcp");
        assert_eq!(
            oauth.metadata_url(),
            "https://mcp.example.com/.well-known/oauth-protected-resource/mcp"
        );
        assert_eq!(
            oauth.metadata_paths(),
            [
                "/.well-known/oauth-protected-resource/mcp",
                "/.well-known/oauth-protected-resource"
            ]
        );
        let root = OAuth::builder("https://mcp.example.com", verifier)
            .authorization_server("https://auth.example.com")
            .build()
            .unwrap();
        assert_eq!(
            root.metadata_paths(),
            ["/.well-known/oauth-protected-resource"]
        );
        let body: Value = serde_json::from_slice(&oauth.inner.metadata_body).unwrap();
        assert_eq!(
            body,
            json!({
                "resource": "https://mcp.example.com/mcp",
                "authorization_servers": ["https://auth.example.com"],
                "scopes_supported": ["mcp"],
                "bearer_methods_supported": ["header"],
            })
        );
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let build = |resource: &str| {
            OAuth::builder(resource, verifier)
                .authorization_server("https://auth.example.com")
                .build()
        };
        assert!(build("http://mcp.example.com/mcp").is_err());
        assert!(build("mcp.example.com").is_err());
        assert!(build("https://mcp.example.com/#x").is_err());
        assert!(build("https://user@mcp.example.com").is_err());
        assert!(build("http://localhost:8000/mcp").is_ok());
        assert_eq!(
            OAuth::builder("https://a.example", verifier)
                .build()
                .unwrap_err(),
            OAuthError::NoAuthorizationServer
        );
        let bad_scope = OAuth::builder("https://a.example", verifier)
            .authorization_server("https://auth.example.com")
            .scopes_supported(["a b"])
            .build();
        assert!(matches!(bad_scope, Err(OAuthError::InvalidScope(_))));
        let offline = OAuth::builder("https://a.example", verifier)
            .authorization_server("https://auth.example.com")
            .scopes_supported(["offline_access"])
            .build();
        assert_eq!(offline.unwrap_err(), OAuthError::OfflineAccess);
    }

    #[tokio::test]
    async fn challenges() {
        let oauth = oauth();
        assert!(oauth.authenticate(&headers(&["Bearer good"])).await.is_ok());
        assert!(oauth.authenticate(&headers(&["bearer good"])).await.is_ok());
        assert_eq!(
            oauth.authenticate(&headers(&[])).await.unwrap_err(),
            Challenge::Missing
        );
        assert_eq!(
            oauth
                .authenticate(&headers(&["Basic Zm9vOmJhcg=="]))
                .await
                .unwrap_err(),
            Challenge::Missing
        );
        assert_eq!(
            oauth
                .authenticate(&headers(&["Bearer good", "Bearer good"]))
                .await
                .unwrap_err(),
            Challenge::Malformed
        );
        for bad in ["Bearer nope", "Bearer ", "Bearer a\"b", "Bearer =="] {
            assert_eq!(
                oauth.authenticate(&headers(&[bad])).await.unwrap_err(),
                Challenge::InvalidToken,
                "{bad}"
            );
        }
        assert_eq!(
            oauth
                .authenticate(&headers(&["Bearer bare"]))
                .await
                .unwrap_err(),
            Challenge::InsufficientScope("mcp".to_owned())
        );

        let value = |c: &Challenge| {
            oauth
                .www_authenticate(c)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            value(&Challenge::Missing),
            "Bearer resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource/mcp\", scope=\"mcp\""
        );
        assert!(value(&Challenge::InvalidToken).ends_with("error=\"invalid_token\""));
        assert!(
            value(&Challenge::InsufficientScope("a b".to_owned()))
                .ends_with("scope=\"a b\", error=\"insufficient_scope\"")
        );
    }

    #[tokio::test]
    async fn slow_verifiers_time_out() {
        struct Slow;
        impl TokenVerifier for Slow {
            fn verify<'a>(&'a self, _: &'a str) -> crate::auth::VerifyFuture<'a> {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Err(TokenError::new("late"))
                })
            }
        }
        let oauth = OAuth::builder("https://a.example", Slow)
            .authorization_server("https://auth.example.com")
            .verify_timeout(Duration::from_millis(10))
            .build()
            .unwrap();
        assert_eq!(
            oauth
                .authenticate(&headers(&["Bearer t"]))
                .await
                .unwrap_err(),
            Challenge::Unavailable
        );
    }

    #[test]
    fn metadata_service_answers_get_only() {
        let service = oauth().metadata_service();
        let get = service.respond(&Method::GET);
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            service.respond(&Method::POST).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
