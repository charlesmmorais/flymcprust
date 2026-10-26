//! Built-in verifier for JWT access tokens (RFC 9068), signed with keys the
//! authorization server publishes as a JSON Web Key Set (JWKS).
//!
//! Only asymmetric signatures are supported (RSA, RSA-PSS, ECDSA P-256 and
//! P-384, Ed25519); `none` and HMAC algorithms do not exist here, so they
//! cannot be accepted by mistake. Keys come only from the configured source:
//! the `jku`, `x5u` and `jwk` header parameters of a token are ignored.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty, Limited};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde_json::{Map, Value};

use super::{Principal, TokenError, TokenVerifier, VerifyFuture};
use crate::transport::oauth::canonical_url;

/// Largest metadata or JWKS document accepted from the network.
const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
/// Most keys kept from one JWKS.
const MAX_KEYS: usize = 64;
/// Default for how often keys may be refetched when tokens name unknown keys.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// Keys older than this are refreshed on the next verification.
const MAX_KEY_AGE: Duration = Duration::from_secs(3600);

/// A JWS signature algorithm accepted by [`JwtVerifier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Algorithm {
    /// RSASSA-PKCS1-v1_5 with SHA-256 (`RS256`).
    Rs256,
    /// RSASSA-PKCS1-v1_5 with SHA-384 (`RS384`).
    Rs384,
    /// RSASSA-PKCS1-v1_5 with SHA-512 (`RS512`).
    Rs512,
    /// RSASSA-PSS with SHA-256 (`PS256`).
    Ps256,
    /// RSASSA-PSS with SHA-384 (`PS384`).
    Ps384,
    /// RSASSA-PSS with SHA-512 (`PS512`).
    Ps512,
    /// ECDSA on P-256 with SHA-256 (`ES256`).
    Es256,
    /// ECDSA on P-384 with SHA-384 (`ES384`).
    Es384,
    /// Ed25519 (`EdDSA` or `Ed25519`).
    EdDsa,
}

impl Algorithm {
    /// Every supported algorithm (the default allowlist).
    pub const ALL: [Self; 9] = [
        Self::Rs256,
        Self::Rs384,
        Self::Rs512,
        Self::Ps256,
        Self::Ps384,
        Self::Ps512,
        Self::Es256,
        Self::Es384,
        Self::EdDsa,
    ];

    /// The JWS name (`alg`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Rs384 => "RS384",
            Self::Rs512 => "RS512",
            Self::Ps256 => "PS256",
            Self::Ps384 => "PS384",
            Self::Ps512 => "PS512",
            Self::Es256 => "ES256",
            Self::Es384 => "ES384",
            Self::EdDsa => "EdDSA",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "RS256" => Self::Rs256,
            "RS384" => Self::Rs384,
            "RS512" => Self::Rs512,
            "PS256" => Self::Ps256,
            "PS384" => Self::Ps384,
            "PS512" => Self::Ps512,
            "ES256" => Self::Es256,
            "ES384" => Self::Es384,
            // RFC 9864 adds the fully-specified name.
            "EdDSA" | "Ed25519" => Self::EdDsa,
            _ => return None,
        })
    }
}

/// Public key material of one JWK.
#[derive(Clone, PartialEq, Eq)]
enum Material {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    P256(Vec<u8>),
    P384(Vec<u8>),
    Ed25519(Vec<u8>),
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    kid: Option<String>,
    alg: Option<Algorithm>,
    material: Material,
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.material {
            Material::Rsa { .. } => "RSA",
            Material::P256(_) => "P-256",
            Material::P384(_) => "P-384",
            Material::Ed25519(_) => "Ed25519",
        };
        f.debug_struct("Key")
            .field("kid", &self.kid)
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

impl Key {
    /// Whether this key may verify a signature made with `alg`.
    fn fits(&self, alg: Algorithm) -> bool {
        if self.alg.is_some_and(|a| a != alg) {
            return false;
        }
        matches!(
            (&self.material, alg),
            (
                Material::Rsa { .. },
                Algorithm::Rs256
                    | Algorithm::Rs384
                    | Algorithm::Rs512
                    | Algorithm::Ps256
                    | Algorithm::Ps384
                    | Algorithm::Ps512
            ) | (Material::P256(_), Algorithm::Es256)
                | (Material::P384(_), Algorithm::Es384)
                | (Material::Ed25519(_), Algorithm::EdDsa)
        )
    }

    fn verify(&self, alg: Algorithm, message: &[u8], sig: &[u8]) -> bool {
        let rsa = |params: &'static signature::RsaParameters, n: &[u8], e: &[u8]| {
            RsaPublicKeyComponents { n, e }
                .verify(params, message, sig)
                .is_ok()
        };
        match (&self.material, alg) {
            (Material::Rsa { n, e }, alg) => match alg {
                Algorithm::Rs256 => rsa(&signature::RSA_PKCS1_2048_8192_SHA256, n, e),
                Algorithm::Rs384 => rsa(&signature::RSA_PKCS1_2048_8192_SHA384, n, e),
                Algorithm::Rs512 => rsa(&signature::RSA_PKCS1_2048_8192_SHA512, n, e),
                Algorithm::Ps256 => rsa(&signature::RSA_PSS_2048_8192_SHA256, n, e),
                Algorithm::Ps384 => rsa(&signature::RSA_PSS_2048_8192_SHA384, n, e),
                Algorithm::Ps512 => rsa(&signature::RSA_PSS_2048_8192_SHA512, n, e),
                _ => false,
            },
            (Material::P256(point), Algorithm::Es256) => {
                UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point)
                    .verify(message, sig)
                    .is_ok()
            }
            (Material::P384(point), Algorithm::Es384) => {
                UnparsedPublicKey::new(&signature::ECDSA_P384_SHA384_FIXED, point)
                    .verify(message, sig)
                    .is_ok()
            }
            (Material::Ed25519(x), Algorithm::EdDsa) => {
                UnparsedPublicKey::new(&signature::ED25519, x)
                    .verify(message, sig)
                    .is_ok()
            }
            _ => false,
        }
    }
}

fn b64(value: Option<&Value>) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value?.as_str()?).ok()
}

/// Parses one JWK; `None` for keys that cannot verify signatures here
/// (encryption keys, unknown types or algorithms, malformed values).
fn parse_key(jwk: &Value) -> Option<Key> {
    let field = |name: &str| jwk.get(name);
    if field("use").is_some_and(|u| u.as_str() != Some("sig")) {
        return None;
    }
    let alg = match field("alg") {
        None => None,
        Some(alg) => Some(Algorithm::from_name(alg.as_str()?)?),
    };
    let material = match field("kty")?.as_str()? {
        "RSA" => {
            let n = b64(field("n"))?;
            let e = b64(field("e"))?;
            // At least 2048 bits (leading zero bytes aside).
            let bits = n.iter().skip_while(|b| **b == 0).count() * 8;
            if bits < 2048 || e.is_empty() {
                return None;
            }
            Material::Rsa { n, e }
        }
        "EC" => {
            let x = b64(field("x"))?;
            let y = b64(field("y"))?;
            let size = match field("crv")?.as_str()? {
                "P-256" => 32,
                "P-384" => 48,
                _ => return None,
            };
            if x.len() != size || y.len() != size {
                return None;
            }
            let mut point = Vec::with_capacity(1 + 2 * size);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            if size == 32 {
                Material::P256(point)
            } else {
                Material::P384(point)
            }
        }
        "OKP" => {
            if field("crv")?.as_str()? != "Ed25519" {
                return None;
            }
            let x = b64(field("x"))?;
            if x.len() != 32 {
                return None;
            }
            Material::Ed25519(x)
        }
        _ => return None,
    };
    Some(Key {
        kid: field("kid").and_then(Value::as_str).map(str::to_owned),
        alg,
        material,
    })
}

/// The usable signing keys of a JWKS document.
fn parse_jwks(document: &Value) -> Vec<Key> {
    document
        .get("keys")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(parse_key).take(MAX_KEYS).collect())
        .unwrap_or_default()
}

/// A token split into its parts, with the header checked.
struct Parsed<'a> {
    alg: Algorithm,
    kid: Option<String>,
    signing_input: &'a str,
    signature: Vec<u8>,
    payload: &'a str,
}

fn parse_token<'a>(token: &'a str, allowed: &[Algorithm]) -> Result<Parsed<'a>, TokenError> {
    let malformed = || TokenError::new("malformed JWT");
    let (signing_input, signature) = token.rsplit_once('.').ok_or_else(malformed)?;
    let (header, payload) = signing_input.split_once('.').ok_or_else(malformed)?;
    if payload.contains('.') {
        // Five parts: an encrypted token (JWE).
        return Err(TokenError::new("encrypted tokens are not supported"));
    }
    let header: Map<String, Value> = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(malformed)?;
    if header.contains_key("crit") {
        // RFC 7515 §4.1.11: unknown critical extensions must be rejected.
        return Err(TokenError::new(
            "critical header parameters are not supported",
        ));
    }
    let name = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    let alg = Algorithm::from_name(name)
        .filter(|alg| allowed.contains(alg))
        .ok_or_else(|| TokenError::new(format!("algorithm not allowed: {name:.16}")))?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| malformed())?;
    Ok(Parsed {
        alg,
        kid: header.get("kid").and_then(Value::as_str).map(str::to_owned),
        signing_input,
        signature,
        payload,
    })
}

/// Where the keys come from.
enum Source {
    /// Keys given at build time.
    Static,
    /// Keys fetched over HTTPS; `jwks_uri` is discovered when not given.
    Remote(Fetcher),
}

struct RefreshState {
    last_attempt: Option<Instant>,
    jwks_uri: Option<String>,
}

struct Inner {
    issuer: String,
    audience: String,
    algorithms: Vec<Algorithm>,
    leeway: u64,
    min_refresh: Duration,
    keys: RwLock<(Arc<[Key]>, Option<Instant>)>,
    source: Source,
    refresh: tokio::sync::Mutex<RefreshState>,
}

/// Verifies JWT access tokens: signature (with the issuer's published
/// keys), issuer, audience, expiry and not-before.
///
/// The token's `sub` becomes the [`Principal`] subject, and its `scope`
/// (space-separated) or `scp` claim the granted scopes.
///
/// ```no_run
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use flymcprust_core::auth::JwtVerifier;
/// use flymcprust_core::transport::http::OAuth;
///
/// let resource = "https://mcp.example.com/mcp";
/// let verifier = JwtVerifier::builder("https://auth.example.com", resource).build()?;
/// let oauth = OAuth::builder(resource, verifier)
///     .authorization_server("https://auth.example.com")
///     .build()?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct JwtVerifier {
    inner: Arc<Inner>,
}

impl fmt::Debug for JwtVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtVerifier")
            .field("issuer", &self.inner.issuer)
            .field("audience", &self.inner.audience)
            .field("algorithms", &self.inner.algorithms)
            .finish_non_exhaustive()
    }
}

impl JwtVerifier {
    /// Starts a verifier for tokens issued by `issuer` (exactly as in the
    /// tokens' `iss` claim) for `audience` (this server's canonical URI,
    /// the `resource` of [`OAuth`](crate::transport::http::OAuth)).
    ///
    /// By default the signing keys are discovered from the issuer's
    /// metadata (RFC 8414, then OpenID Connect Discovery).
    #[must_use]
    pub fn builder(issuer: impl Into<String>, audience: impl Into<String>) -> JwtVerifierBuilder {
        JwtVerifierBuilder {
            issuer: issuer.into(),
            audience: audience.into(),
            jwks_uri: None,
            jwks: None,
            algorithms: Algorithm::ALL.to_vec(),
            leeway: Duration::from_secs(60),
            http_timeout: Duration::from_secs(5),
            min_refresh: MIN_REFRESH_INTERVAL,
        }
    }

    async fn verify_token(&self, token: &str) -> Result<Principal, TokenError> {
        let inner = &self.inner;
        let parsed = parse_token(token, &inner.algorithms)?;

        let mut keys = self.keys(false).await;
        if candidates(&keys, &parsed).is_empty() && matches!(inner.source, Source::Remote(_)) {
            // Perhaps the issuer rotated its keys.
            keys = self.keys(true).await;
        }
        let candidates = candidates(&keys, &parsed);
        if candidates.is_empty() {
            return Err(TokenError::new("no key matches the token"));
        }
        let message = parsed.signing_input.as_bytes();
        if !candidates
            .iter()
            .any(|key| key.verify(parsed.alg, message, &parsed.signature))
        {
            return Err(TokenError::new("bad signature"));
        }

        let claims: Map<String, Value> = URL_SAFE_NO_PAD
            .decode(parsed.payload)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| TokenError::new("malformed claims"))?;
        self.check_claims(claims)
    }

    fn check_claims(&self, claims: Map<String, Value>) -> Result<Principal, TokenError> {
        let inner = &self.inner;
        if claims.get("iss").and_then(Value::as_str) != Some(inner.issuer.as_str()) {
            return Err(TokenError::new("wrong issuer"));
        }
        let audience_ok = match claims.get("aud") {
            Some(Value::String(aud)) => *aud == inner.audience,
            Some(Value::Array(auds)) => auds
                .iter()
                .any(|a| a.as_str() == Some(inner.audience.as_str())),
            _ => false,
        };
        if !audience_ok {
            return Err(TokenError::new("wrong audience"));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let time = |name: &str| -> Result<Option<u64>, TokenError> {
            match claims.get(name) {
                None => Ok(None),
                Some(value) => value
                    .as_f64()
                    .filter(|t| t.is_finite() && *t >= 0.0)
                    // Whole seconds; fractions only matter within the leeway.
                    .map(|t| Some(t as u64))
                    .ok_or_else(|| TokenError::new(format!("malformed `{name}`"))),
            }
        };
        let exp = time("exp")?.ok_or_else(|| TokenError::new("missing `exp`"))?;
        if now >= exp.saturating_add(inner.leeway) {
            return Err(TokenError::new("expired"));
        }
        if time("nbf")?.is_some_and(|nbf| now.saturating_add(inner.leeway) < nbf) {
            return Err(TokenError::new("not yet valid"));
        }
        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| TokenError::new("missing `sub`"))?
            .to_owned();

        let mut scopes: Vec<String> = Vec::new();
        if let Some(scope) = claims.get("scope").and_then(Value::as_str) {
            scopes.extend(scope.split_whitespace().map(str::to_owned));
        }
        match claims.get("scp") {
            Some(Value::String(scp)) => scopes.extend(scp.split_whitespace().map(str::to_owned)),
            Some(Value::Array(scp)) => {
                scopes.extend(scp.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            _ => {}
        }
        Ok(Principal::new(subject)
            .with_issuer(inner.issuer.clone())
            .with_scopes(scopes)
            .with_claims(claims))
    }

    /// The current keys, refreshed when stale or (`force`) when a token
    /// names an unknown key. Fetch failures keep the previous keys.
    async fn keys(&self, force: bool) -> Arc<[Key]> {
        let inner = &self.inner;
        let current = || {
            inner
                .keys
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        };
        let (keys, fetched) = current();
        let Source::Remote(fetcher) = &inner.source else {
            return keys;
        };
        let stale = fetched.is_none_or(|at| at.elapsed() >= MAX_KEY_AGE);
        if !stale && !force {
            return keys;
        }

        // One fetch at a time. A routine refresh does not wait for one in
        // progress (the current keys still work); a token naming an unknown
        // key waits for its result.
        let mut state = match inner.refresh.try_lock() {
            Ok(state) => state,
            Err(_) if !force && !keys.is_empty() => return keys,
            Err(_) => inner.refresh.lock().await,
        };
        let (keys, fetched) = current();
        let fresh_enough = fetched.is_some_and(|at| at.elapsed() < MAX_KEY_AGE);
        if (fresh_enough && !force)
            || state
                .last_attempt
                .is_some_and(|at| at.elapsed() < inner.min_refresh)
        {
            return keys;
        }
        state.last_attempt = Some(Instant::now());
        match self.fetch_keys(fetcher, &mut state).await {
            Ok(new_keys) => {
                let new_keys: Arc<[Key]> = new_keys.into();
                *inner.keys.write().unwrap_or_else(PoisonError::into_inner) =
                    (Arc::clone(&new_keys), Some(Instant::now()));
                new_keys
            }
            Err(error) => {
                tracing::warn!(
                    issuer = %inner.issuer,
                    "could not fetch the issuer's signing keys: {error}\n  = help: check that \
                     the authorization server is reachable and serves its metadata and JWKS; \
                     or set JwtVerifierBuilder::jwks_uri(..)"
                );
                keys
            }
        }
    }

    async fn fetch_keys(
        &self,
        fetcher: &Fetcher,
        state: &mut RefreshState,
    ) -> Result<Vec<Key>, String> {
        let jwks_uri = match &state.jwks_uri {
            Some(uri) => uri.clone(),
            None => {
                let uri = discover_jwks_uri(fetcher, &self.inner.issuer).await?;
                state.jwks_uri = Some(uri.clone());
                uri
            }
        };
        let document = fetcher.get_json(&jwks_uri).await?;
        let keys = parse_jwks(&document);
        if keys.is_empty() {
            return Err(format!("{jwks_uri} has no usable signing keys"));
        }
        Ok(keys)
    }
}

impl TokenVerifier for JwtVerifier {
    fn verify<'a>(&'a self, token: &'a str) -> VerifyFuture<'a> {
        Box::pin(self.verify_token(token))
    }
}

fn candidates<'k>(keys: &'k [Key], token: &Parsed<'_>) -> Vec<&'k Key> {
    keys.iter()
        .filter(|key| key.fits(token.alg))
        .filter(|key| match &token.kid {
            Some(kid) => key.kid.as_deref() == Some(kid.as_str()),
            None => true,
        })
        .collect()
}

/// The metadata URLs to try for an issuer, in the order the MCP
/// specification gives (RFC 8414 first, then OpenID Connect Discovery).
fn metadata_urls(issuer: &str) -> Vec<String> {
    let after_scheme = issuer.find("://").map_or(0, |i| i + 3);
    let path_start = issuer
        .get(after_scheme..)
        .and_then(|rest| rest.find('/'))
        .map_or(issuer.len(), |i| i + after_scheme);
    let (origin, path) = issuer.split_at(path_start);
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    }
}

async fn discover_jwks_uri(fetcher: &Fetcher, issuer: &str) -> Result<String, String> {
    let mut last_error = String::from("no metadata URL");
    for url in metadata_urls(issuer) {
        let metadata = match fetcher.get_json(&url).await {
            Ok(metadata) => metadata,
            Err(error) => {
                last_error = error;
                continue;
            }
        };
        // RFC 8414 §3.3: the document must name the issuer it was fetched for.
        if metadata.get("issuer").and_then(Value::as_str) != Some(issuer) {
            return Err(format!("{url} names another issuer"));
        }
        let uri = metadata
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{url} has no `jwks_uri`"))?;
        canonical_url(uri).map_err(|reason| format!("invalid `jwks_uri` {uri}: {reason}"))?;
        if issuer.starts_with("https://") && !uri.starts_with("https://") {
            return Err(format!("{url} names a `jwks_uri` without https"));
        }
        return Ok(uri.to_owned());
    }
    Err(last_error)
}

type Client = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
>;

/// Fetches JSON documents over HTTPS (HTTP only on localhost), without
/// following redirects, with a size limit and a timeout.
struct Fetcher {
    client: Client,
    timeout: Duration,
}

impl Fetcher {
    fn new(timeout: Duration) -> Result<Self, JwtError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(provider)
            .map_err(|error| JwtError::Tls(error.to_string()))?
            .https_or_http()
            .enable_http1()
            .build();
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector);
        Ok(Self { client, timeout })
    }

    async fn get_json(&self, url: &str) -> Result<Value, String> {
        // Plain HTTP only for localhost (canonical_url enforces it).
        canonical_url(url).map_err(|reason| format!("{url}: {reason}"))?;
        let request = http::Request::get(url)
            .header(http::header::ACCEPT, "application/json")
            .header(http::header::USER_AGENT, "flymcprust")
            .body(Empty::new())
            .map_err(|error| format!("{url}: {error}"))?;
        let fetch = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|error| format!("{url}: {error}"))?;
            if response.status() != http::StatusCode::OK {
                return Err(format!("{url}: HTTP {}", response.status()));
            }
            let body = Limited::new(response.into_body(), MAX_DOCUMENT_BYTES)
                .collect()
                .await
                .map_err(|error| format!("{url}: {error}"))?
                .to_bytes();
            serde_json::from_slice(&body).map_err(|error| format!("{url}: invalid JSON: {error}"))
        };
        tokio::time::timeout(self.timeout, fetch)
            .await
            .map_err(|_| format!("{url}: no answer within {:?}", self.timeout))?
    }
}

/// Builder of [`JwtVerifier`].
#[derive(Debug)]
pub struct JwtVerifierBuilder {
    issuer: String,
    audience: String,
    jwks_uri: Option<String>,
    jwks: Option<String>,
    algorithms: Vec<Algorithm>,
    leeway: Duration,
    http_timeout: Duration,
    min_refresh: Duration,
}

impl JwtVerifierBuilder {
    /// Fetches keys from this URL instead of discovering it from the
    /// issuer's metadata.
    #[must_use]
    pub fn jwks_uri(mut self, uri: impl Into<String>) -> Self {
        self.jwks_uri = Some(uri.into());
        self
    }

    /// Uses these keys (a JWKS document, `{"keys": [..]}`) and never fetches
    /// any. Rotating them then requires a restart.
    #[must_use]
    pub fn jwks(mut self, document: impl Into<String>) -> Self {
        self.jwks = Some(document.into());
        self
    }

    /// The accepted signature algorithms. Default: all of [`Algorithm::ALL`].
    #[must_use]
    pub fn algorithms(mut self, algorithms: impl IntoIterator<Item = Algorithm>) -> Self {
        self.algorithms = algorithms.into_iter().collect();
        self
    }

    /// Tolerated clock difference for `exp` and `nbf`. Default: 60 s.
    #[must_use]
    pub fn leeway(mut self, leeway: Duration) -> Self {
        self.leeway = leeway;
        self
    }

    /// Time allowed for each metadata or JWKS request. Default: 5 s.
    #[must_use]
    pub fn http_timeout(mut self, timeout: Duration) -> Self {
        self.http_timeout = timeout;
        self
    }

    /// Minimum time between two key fetches triggered by tokens that name
    /// unknown keys, so forged tokens cannot make the server hammer the
    /// authorization server. Default: 30 s.
    #[must_use]
    pub fn min_refresh_interval(mut self, interval: Duration) -> Self {
        self.min_refresh = interval;
        self
    }

    /// Validates the settings.
    ///
    /// # Errors
    ///
    /// Fails when the issuer or `jwks_uri` is not a valid URL (`https`, or
    /// `http` on localhost), the audience is empty, no algorithm is allowed,
    /// static keys contain no usable signing key, or the system's TLS roots
    /// cannot be loaded.
    pub fn build(self) -> Result<JwtVerifier, JwtError> {
        canonical_url(&self.issuer).map_err(|reason| JwtError::InvalidIssuer {
            issuer: self.issuer.clone(),
            reason,
        })?;
        if self.audience.trim().is_empty() {
            return Err(JwtError::EmptyAudience);
        }
        if self.algorithms.is_empty() {
            return Err(JwtError::NoAlgorithms);
        }
        if self.http_timeout.is_zero() {
            return Err(JwtError::InvalidTimeout);
        }
        let (keys, source, jwks_uri) = match self.jwks {
            Some(document) => {
                let document: Value = serde_json::from_str(&document)
                    .map_err(|error| JwtError::InvalidJwks(error.to_string()))?;
                let keys = parse_jwks(&document);
                if keys.is_empty() {
                    return Err(JwtError::InvalidJwks("no usable signing key".to_owned()));
                }
                (keys, Source::Static, None)
            }
            None => {
                if let Some(uri) = &self.jwks_uri {
                    canonical_url(uri).map_err(|reason| JwtError::InvalidJwksUri {
                        uri: uri.clone(),
                        reason,
                    })?;
                }
                (
                    Vec::new(),
                    Source::Remote(Fetcher::new(self.http_timeout)?),
                    self.jwks_uri,
                )
            }
        };
        let fetched = matches!(source, Source::Static).then(Instant::now);
        Ok(JwtVerifier {
            inner: Arc::new(Inner {
                issuer: self.issuer,
                audience: self.audience,
                algorithms: self.algorithms,
                leeway: self.leeway.as_secs(),
                min_refresh: self.min_refresh,
                keys: RwLock::new((keys.into(), fetched)),
                source,
                refresh: tokio::sync::Mutex::new(RefreshState {
                    last_attempt: None,
                    jwks_uri,
                }),
            }),
        })
    }
}

/// Invalid [`JwtVerifier`] settings.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum JwtError {
    /// The issuer is not a valid URL.
    #[error("invalid issuer `{issuer}`: {reason}")]
    InvalidIssuer {
        /// The configured value.
        issuer: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The JWKS URL is invalid.
    #[error("invalid jwks_uri `{uri}`: {reason}")]
    InvalidJwksUri {
        /// The configured value.
        uri: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The audience is empty.
    #[error("empty audience")]
    EmptyAudience,
    /// No algorithm is allowed.
    #[error("no signature algorithm is allowed")]
    NoAlgorithms,
    /// The static JWKS is invalid.
    #[error("invalid JWKS: {0}")]
    InvalidJwks(String),
    /// The HTTP timeout is zero.
    #[error("the HTTP timeout must be positive")]
    InvalidTimeout,
    /// The system's TLS root certificates could not be loaded.
    #[error("could not load the system's TLS roots: {0}")]
    Tls(String),
}

impl JwtError {
    /// How to fix the error.
    #[must_use]
    pub fn help(&self) -> &'static str {
        match self {
            Self::InvalidIssuer { .. } => {
                "use the issuer exactly as it appears in the tokens' `iss` claim, such as \
                 `https://auth.example.com` (https; http only on localhost)"
            }
            Self::InvalidJwksUri { .. } => {
                "use the authorization server's JWKS URL (https; http only on localhost)"
            }
            Self::EmptyAudience => {
                "use this server's canonical URI, the same value given to OAuth::builder"
            }
            Self::NoAlgorithms => "allow at least one algorithm, or keep the default",
            Self::InvalidJwks(_) => {
                "pass a JWKS document such as {\"keys\": [{\"kty\": \"RSA\", \"n\": .., \"e\": ..}]} \
                 with signing keys (RSA of at least 2048 bits, P-256, P-384 or Ed25519)"
            }
            Self::InvalidTimeout => "use a timeout such as `Duration::from_secs(5)`",
            Self::Tls(_) => {
                "install the operating system's CA certificates (for example the \
                 `ca-certificates` package), or pass static keys with `.jwks(..)`"
            }
        }
    }
}

impl fmt::Debug for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}\n  = help: {}", self.help())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair as _};
    use serde_json::json;

    const ISSUER: &str = "https://auth.example.com";
    const AUDIENCE: &str = "https://mcp.example.com/mcp";

    fn enc(value: &Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn claims() -> Value {
        json!({ "iss": ISSUER, "aud": AUDIENCE, "sub": "alice",
                "exp": now() + 300, "scope": "mcp files:read" })
    }

    struct Signer {
        ed: Ed25519KeyPair,
        ec: EcdsaKeyPair,
    }

    impl Signer {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let ed = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            let ec =
                EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .unwrap();
            Self {
                ed: Ed25519KeyPair::from_pkcs8(ed.as_ref()).unwrap(),
                ec: EcdsaKeyPair::from_pkcs8(
                    &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    ec.as_ref(),
                    &rng,
                )
                .unwrap(),
            }
        }

        fn jwks(&self) -> String {
            let point = self.ec.public_key().as_ref();
            json!({ "keys": [
                { "kty": "OKP", "crv": "Ed25519", "kid": "ed",
                  "x": URL_SAFE_NO_PAD.encode(self.ed.public_key().as_ref()) },
                { "kty": "EC", "crv": "P-256", "kid": "ec", "use": "sig",
                  "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                  "y": URL_SAFE_NO_PAD.encode(&point[33..65]) },
                { "kty": "EC", "crv": "P-256", "kid": "enc", "use": "enc",
                  "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                  "y": URL_SAFE_NO_PAD.encode(&point[33..65]) },
            ] })
            .to_string()
        }

        fn sign(&self, header: &Value, claims: &Value) -> String {
            let input = format!("{}.{}", enc(header), enc(claims));
            let sig = match header["alg"].as_str().unwrap() {
                "EdDSA" => self.ed.sign(input.as_bytes()).as_ref().to_vec(),
                "ES256" => self
                    .ec
                    .sign(&SystemRandom::new(), input.as_bytes())
                    .unwrap()
                    .as_ref()
                    .to_vec(),
                _ => b"forged".to_vec(),
            };
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig))
        }
    }

    fn verifier(signer: &Signer) -> JwtVerifier {
        JwtVerifier::builder(ISSUER, AUDIENCE)
            .jwks(signer.jwks())
            .build()
            .unwrap()
    }

    async fn check(v: &JwtVerifier, token: &str) -> Result<Principal, String> {
        v.verify(token).await.map_err(|e| e.reason().to_owned())
    }

    #[tokio::test]
    async fn valid_tokens_become_principals() {
        let signer = Signer::new();
        let v = verifier(&signer);
        for header in [
            json!({ "alg": "EdDSA", "kid": "ed", "typ": "at+jwt" }),
            json!({ "alg": "ES256", "kid": "ec" }),
            // No `kid`: any fitting key is tried.
            json!({ "alg": "ES256" }),
        ] {
            let principal = check(&v, &signer.sign(&header, &claims())).await.unwrap();
            assert_eq!(principal.subject(), "alice");
            assert_eq!(principal.issuer(), Some(ISSUER));
            assert!(principal.has_scope("files:read"));
            assert_eq!(principal.claims()["aud"], AUDIENCE);
        }
        // `scp` arrays (Entra ID) and audience arrays.
        let mut c = claims();
        c["scp"] = json!(["a", "b"]);
        c["aud"] = json!(["other", AUDIENCE]);
        let p = check(&v, &signer.sign(&json!({ "alg": "EdDSA" }), &c))
            .await
            .unwrap();
        assert!(p.has_scope("a") && p.has_scope("mcp"));
    }

    #[tokio::test]
    async fn bad_tokens_are_rejected() {
        let signer = Signer::new();
        let v = verifier(&signer);
        let ed = json!({ "alg": "EdDSA", "kid": "ed" });
        let with = |name: &str, value: Value| {
            let mut c = claims();
            c[name] = value;
            signer.sign(&ed, &c)
        };
        let cases = [
            (
                with("iss", json!("https://evil.example.com")),
                "wrong issuer",
            ),
            (
                with("aud", json!("https://other.example.com")),
                "wrong audience",
            ),
            (with("exp", json!(now() - 120)), "expired"),
            (with("nbf", json!(now() + 600)), "not yet valid"),
            (with("sub", json!("")), "missing `sub`"),
            (with("exp", json!("tomorrow")), "malformed `exp`"),
            (
                signer.sign(&json!({ "alg": "none" }), &claims()),
                "algorithm not allowed: none",
            ),
            (
                signer.sign(&json!({ "alg": "HS256" }), &claims()),
                "algorithm not allowed: HS256",
            ),
            (
                signer.sign(&json!({ "alg": "EdDSA", "kid": "missing" }), &claims()),
                "no key matches the token",
            ),
            // The encryption-only key is never used.
            (
                signer.sign(&json!({ "alg": "ES256", "kid": "enc" }), &claims()),
                "no key matches the token",
            ),
            (
                signer.sign(&json!({ "alg": "EdDSA", "crit": ["exp"] }), &claims()),
                "critical header parameters are not supported",
            ),
            ("a.b".to_owned(), "malformed JWT"),
            ("a.b.c.d.e".to_owned(), "encrypted tokens are not supported"),
        ];
        for (token, reason) in cases {
            assert_eq!(check(&v, &token).await.unwrap_err(), reason, "{token}");
        }

        // A valid token with a tampered payload.
        let token = signer.sign(&ed, &claims());
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut forged = claims();
        forged["sub"] = json!("admin");
        let forged = enc(&forged);
        parts[1] = &forged;
        assert_eq!(
            check(&v, &parts.join(".")).await.unwrap_err(),
            "bad signature"
        );

        // An ES256 signature presented as EdDSA (algorithm confusion).
        let es = signer.sign(&json!({ "alg": "ES256", "kid": "ec" }), &claims());
        let sig = es.rsplit_once('.').unwrap().1;
        let input = format!("{}.{}", enc(&json!({ "alg": "EdDSA" })), enc(&claims()));
        assert_eq!(
            check(&v, &format!("{input}.{sig}")).await.unwrap_err(),
            "bad signature"
        );
    }

    #[tokio::test]
    async fn the_allowlist_is_enforced() {
        let signer = Signer::new();
        let v = JwtVerifier::builder(ISSUER, AUDIENCE)
            .jwks(signer.jwks())
            .algorithms([Algorithm::Es256])
            .build()
            .unwrap();
        let token = signer.sign(&json!({ "alg": "EdDSA" }), &claims());
        assert_eq!(
            check(&v, &token).await.unwrap_err(),
            "algorithm not allowed: EdDSA"
        );
    }

    #[test]
    fn weak_and_foreign_keys_are_ignored() {
        let short_rsa =
            json!({ "kty": "RSA", "n": URL_SAFE_NO_PAD.encode([1u8; 128]), "e": "AQAB" });
        let rsa = json!({ "kty": "RSA", "n": URL_SAFE_NO_PAD.encode([1u8; 256]), "e": "AQAB" });
        let hmac = json!({ "kty": "oct", "k": "c2VjcmV0" });
        let x448 = json!({ "kty": "OKP", "crv": "X448", "x": "AA" });
        let unknown_alg = json!({ "kty": "RSA", "alg": "RS1", "n": URL_SAFE_NO_PAD.encode([1u8; 256]), "e": "AQAB" });
        let keys = parse_jwks(&json!({ "keys": [short_rsa, rsa, hmac, x448, unknown_alg] }));
        assert_eq!(keys.len(), 1);
        assert!(keys[0].fits(Algorithm::Ps256));
        assert!(!keys[0].fits(Algorithm::Es256));
    }

    #[test]
    fn settings_are_validated() {
        let build = |issuer: &str, audience: &str| {
            JwtVerifier::builder(issuer, audience)
                .jwks(r#"{"keys":[]}"#)
                .build()
        };
        assert!(matches!(
            build("http://auth.example.com", AUDIENCE),
            Err(JwtError::InvalidIssuer { .. })
        ));
        assert_eq!(build(ISSUER, " ").unwrap_err(), JwtError::EmptyAudience);
        assert!(matches!(
            build(ISSUER, AUDIENCE),
            Err(JwtError::InvalidJwks(_))
        ));
        assert_eq!(
            JwtVerifier::builder(ISSUER, AUDIENCE)
                .algorithms([])
                .build()
                .unwrap_err(),
            JwtError::NoAlgorithms
        );
    }

    #[test]
    fn rfc_example_keys_parse() {
        // The keys of the `verify_jwt` fuzz target.
        let keys = parse_jwks(&json!({ "keys": [
            { "kty": "OKP", "crv": "Ed25519", "kid": "ed",
              "x": "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo" },
            { "kty": "EC", "crv": "P-256", "kid": "ec",
              "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
              "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" },
        ] }));
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn discovery_urls_follow_the_specification() {
        assert_eq!(
            metadata_urls("https://auth.example.com"),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server",
                "https://auth.example.com/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            metadata_urls("https://auth.example.com/tenant1"),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
    }
}
