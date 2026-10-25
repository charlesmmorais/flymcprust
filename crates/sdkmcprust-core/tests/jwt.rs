//! `JwtVerifier` against a local authorization server: metadata discovery,
//! JWKS fetching, key rotation, refresh limits, and the full OAuth flow
//! through the HTTP transport.
#![cfg(feature = "oauth")]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SystemRandom;
use ring::signature::{self, Ed25519KeyPair, KeyPair as _, RsaKeyPair};
use sdkmcprust_core::TokenVerifier;
use sdkmcprust_core::auth::JwtVerifier;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

const AUDIENCE: &str = "http://localhost:8000/mcp";
const RSA_KEY: &[u8] = include_bytes!("fixtures/test-rsa-2048.der");

/// A minimal authorization server: RFC 8414 metadata and a JWKS that the
/// test can replace.
struct AuthServer {
    issuer: String,
    jwks: Arc<Mutex<Value>>,
    jwks_fetches: Arc<AtomicUsize>,
    /// The `issuer` the metadata claims (to test mix-up protection).
    claimed_issuer: Arc<Mutex<Option<String>>>,
}

impl AuthServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let jwks = Arc::new(Mutex::new(json!({ "keys": [] })));
        let jwks_fetches = Arc::new(AtomicUsize::new(0));
        let claimed_issuer = Arc::new(Mutex::new(None::<String>));
        let (iss, keys, fetches, claimed) = (
            issuer.clone(),
            Arc::clone(&jwks),
            Arc::clone(&jwks_fetches),
            Arc::clone(&claimed_issuer),
        );
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (iss, keys, fetches, claimed) = (
                    iss.clone(),
                    Arc::clone(&keys),
                    Arc::clone(&fetches),
                    Arc::clone(&claimed),
                );
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..n]);
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
                    let (status, body) = match path.as_str() {
                        "/.well-known/oauth-authorization-server" => {
                            let named = claimed.lock().unwrap().clone().unwrap_or(iss.clone());
                            (
                                "200 OK",
                                json!({ "issuer": named, "jwks_uri": format!("{iss}/jwks") }),
                            )
                        }
                        "/jwks" => {
                            fetches.fetch_add(1, Ordering::SeqCst);
                            ("200 OK", keys.lock().unwrap().clone())
                        }
                        _ => ("404 Not Found", json!({})),
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self {
            issuer,
            jwks,
            jwks_fetches,
            claimed_issuer,
        }
    }

    fn verifier(&self) -> JwtVerifier {
        JwtVerifier::builder(&self.issuer, AUDIENCE)
            .min_refresh_interval(Duration::ZERO)
            .build()
            .unwrap()
    }
}

fn enc(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
}

fn claims(issuer: &str) -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({ "iss": issuer, "aud": AUDIENCE, "sub": "alice", "exp": now + 300, "scope": "mcp" })
}

fn rsa_key() -> RsaKeyPair {
    RsaKeyPair::from_der(RSA_KEY).unwrap()
}

fn rsa_jwk(kid: &str) -> Value {
    let key = rsa_key();
    let public: signature::RsaPublicKeyComponents<Vec<u8>> = key.public().into();
    json!({
        "kty": "RSA", "kid": kid, "use": "sig",
        "n": URL_SAFE_NO_PAD.encode(&public.n),
        "e": URL_SAFE_NO_PAD.encode(&public.e),
    })
}

fn rsa_token(alg: &str, kid: &str, claims: &Value) -> String {
    let padding: &'static dyn signature::RsaEncoding = match alg {
        "RS256" => &signature::RSA_PKCS1_SHA256,
        "PS256" => &signature::RSA_PSS_SHA256,
        "RS512" => &signature::RSA_PKCS1_SHA512,
        _ => panic!("unsupported test algorithm"),
    };
    let input = format!(
        "{}.{}",
        enc(&json!({ "alg": alg, "kid": kid })),
        enc(claims)
    );
    let key = rsa_key();
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(padding, &SystemRandom::new(), input.as_bytes(), &mut sig)
        .unwrap();
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig))
}

fn ed_key() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

fn ed_jwk(key: &Ed25519KeyPair, kid: &str) -> Value {
    json!({ "kty": "OKP", "crv": "Ed25519", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(key.public_key().as_ref()) })
}

fn ed_token(key: &Ed25519KeyPair, kid: &str, claims: &Value) -> String {
    let input = format!(
        "{}.{}",
        enc(&json!({ "alg": "EdDSA", "kid": kid })),
        enc(claims)
    );
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).as_ref())
    )
}

#[tokio::test]
async fn discovers_keys_and_verifies_rsa_tokens() {
    let server = AuthServer::start().await;
    *server.jwks.lock().unwrap() = json!({ "keys": [rsa_jwk("r1")] });
    let verifier = server.verifier();
    for alg in ["RS256", "PS256", "RS512"] {
        let token = rsa_token(alg, "r1", &claims(&server.issuer));
        let principal = verifier.verify(&token).await.unwrap();
        assert_eq!(principal.subject(), "alice");
    }
    // Keys are cached.
    assert_eq!(server.jwks_fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rotated_keys_are_picked_up() {
    let server = AuthServer::start().await;
    let old = ed_key();
    *server.jwks.lock().unwrap() = json!({ "keys": [ed_jwk(&old, "old")] });
    let verifier = server.verifier();
    let issuer = server.issuer.clone();
    verifier
        .verify(&ed_token(&old, "old", &claims(&issuer)))
        .await
        .unwrap();

    let new = ed_key();
    *server.jwks.lock().unwrap() = json!({ "keys": [ed_jwk(&new, "new")] });
    verifier
        .verify(&ed_token(&new, "new", &claims(&issuer)))
        .await
        .unwrap();
    assert_eq!(server.jwks_fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unknown_keys_cannot_force_repeated_fetches() {
    let server = AuthServer::start().await;
    let key = ed_key();
    *server.jwks.lock().unwrap() = json!({ "keys": [ed_jwk(&key, "k")] });
    // The default 30 s refresh interval.
    let verifier = JwtVerifier::builder(&server.issuer, AUDIENCE)
        .build()
        .unwrap();
    let issuer = server.issuer.clone();
    for i in 0..10 {
        let token = ed_token(&key, &format!("random-{i}"), &claims(&issuer));
        assert!(verifier.verify(&token).await.is_err());
    }
    assert_eq!(server.jwks_fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn metadata_naming_another_issuer_is_rejected() {
    let server = AuthServer::start().await;
    let key = ed_key();
    *server.jwks.lock().unwrap() = json!({ "keys": [ed_jwk(&key, "k")] });
    *server.claimed_issuer.lock().unwrap() = Some("https://honest.example".to_owned());
    let verifier = server.verifier();
    let token = ed_token(&key, "k", &claims(&server.issuer));
    let error = verifier.verify(&token).await.unwrap_err();
    assert_eq!(error.reason(), "no key matches the token");
    assert_eq!(server.jwks_fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unreachable_issuer_is_a_rejection_not_a_hang() {
    // Nothing listens on port 9 (discard) of localhost.
    let verifier = JwtVerifier::builder("http://127.0.0.1:9", AUDIENCE)
        .http_timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let key = ed_key();
    let token = ed_token(&key, "k", &claims("http://127.0.0.1:9"));
    let started = std::time::Instant::now();
    assert!(verifier.verify(&token).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn protects_an_http_server_end_to_end() {
    use bytes::Bytes;
    use http_body_util::Full;
    use sdkmcprust_core::transport::http::{HttpConfig, OAuth, StreamableHttpService};
    use tower_service::Service as _;

    let auth = AuthServer::start().await;
    *auth.jwks.lock().unwrap() = json!({ "keys": [rsa_jwk("r1")] });
    let oauth = OAuth::builder(AUDIENCE, auth.verifier())
        .authorization_server(&auth.issuer)
        .required_scopes(["mcp"])
        .build()
        .unwrap();
    let server = sdkmcprust_core::Server::builder("jwt", "1.0.0")
        .build()
        .unwrap();
    let mut service = StreamableHttpService::with_config(server, HttpConfig::new().oauth(oauth));
    let discover = |token: Option<String>| {
        let mut request = http::Request::post("/mcp")
            .header("host", "localhost:8000")
            .header("content-type", "application/json")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {
            "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                       "io.modelcontextprotocol/clientCapabilities": {} } } });
        request
            .body(Full::new(Bytes::from(body.to_string())))
            .unwrap()
    };

    let valid = rsa_token("RS256", "r1", &claims(&auth.issuer));
    let response = service.call(discover(Some(valid))).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);

    let mut other_audience = claims(&auth.issuer);
    other_audience["aud"] = json!("https://another-server.example.com");
    let response = service
        .call(discover(Some(rsa_token("RS256", "r1", &other_audience))))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);

    let response = service.call(discover(None)).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
}
