//! Authorization: the authenticated caller ([`Principal`]), access-token
//! verification ([`TokenVerifier`]) and scope checks.
//!
//! The Streamable HTTP transport acts as an OAuth 2.1 resource server when
//! configured with [`OAuth`](crate::transport::http::OAuth): every request
//! must carry a valid bearer token, and handlers that declare scopes (for
//! example `#[tool(scopes = ["files:write"])]`) run only when the token grants
//! them. stdio is not authorized this way: per the specification, it takes
//! credentials from its environment.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::context::Ctx;
use crate::error::McpError;

/// The caller authenticated by an access token.
#[derive(Debug, Clone, PartialEq)]
pub struct Principal {
    subject: String,
    issuer: Option<String>,
    scopes: BTreeSet<String>,
    claims: Map<String, Value>,
}

impl Principal {
    /// A principal identified by `subject` (the token's `sub`).
    #[must_use]
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            issuer: None,
            scopes: BTreeSet::new(),
            claims: Map::new(),
        }
    }

    /// Sets the token issuer.
    #[must_use]
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Sets the granted scopes.
    #[must_use]
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the other claims of the token.
    #[must_use]
    pub fn with_claims(mut self, claims: Map<String, Value>) -> Self {
        self.claims = claims;
        self
    }

    /// The subject (`sub`).
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The issuer (`iss`), when known.
    #[must_use]
    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    /// The granted scopes.
    #[must_use]
    pub fn scopes(&self) -> &BTreeSet<String> {
        &self.scopes
    }

    /// Whether `scope` was granted (exactly; see
    /// [`OAuthBuilder::scope_implies`](crate::transport::http::OAuthBuilder::scope_implies)
    /// for hierarchies).
    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    /// The token's other claims.
    #[must_use]
    pub fn claims(&self) -> &Map<String, Value> {
        &self.claims
    }

    /// A stable, non-reversible identifier of issuer and subject, used to
    /// bind `requestState` and legacy sessions to this principal.
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    pub(crate) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.issuer.as_deref().unwrap_or_default());
        hasher.update([0]);
        hasher.update(&self.subject);
        let digest = hasher.finalize();
        digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
    }
}

/// Why a token was rejected. The reason is logged on the server; clients
/// only learn that the token is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid access token: {reason}")]
pub struct TokenError {
    reason: String,
}

impl TokenError {
    /// A rejection with a reason for the server log.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// The reason (for logs; never sent to clients).
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The future returned by [`TokenVerifier::verify`].
pub type VerifyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Principal, TokenError>> + Send + 'a>>;

/// Checks an access token and identifies its holder.
///
/// Implementations **must** check that the token was issued for this server
/// (audience), is unexpired, and is authentic; the specification forbids
/// accepting tokens meant for other resources. The `oauth` feature provides
/// `JwtVerifier` for JWT access tokens; implement this trait for other
/// formats (for example token introspection).
pub trait TokenVerifier: Send + Sync + 'static {
    /// Verifies `token` (the value after `Bearer `).
    fn verify<'a>(&'a self, token: &'a str) -> VerifyFuture<'a>;
}

impl<F> TokenVerifier for F
where
    F: Fn(&str) -> Result<Principal, TokenError> + Send + Sync + 'static,
{
    fn verify<'a>(&'a self, token: &'a str) -> VerifyFuture<'a> {
        let result = self(token);
        Box::pin(async move { result })
    }
}

/// Whether `scope` is a valid OAuth scope token (RFC 6749 §3.3: visible
/// ASCII except `"` and `\`, no spaces).
pub(crate) fn is_valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// Decides whether a granted scope satisfies a required one (scope
/// hierarchies: `files` might imply `files:read`).
pub type ScopeImplies = fn(granted: &str, required: &str) -> bool;

/// The authorization of the current request, set by the HTTP transport when
/// OAuth is configured.
#[derive(Clone)]
pub(crate) struct AuthContext {
    pub(crate) principal: Arc<Principal>,
    pub(crate) implies: Option<ScopeImplies>,
    /// [`Principal::digest`], computed once.
    pub(crate) digest: String,
}

impl fmt::Debug for AuthContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthContext")
            .field("subject", &self.principal.subject())
            .finish_non_exhaustive()
    }
}

impl AuthContext {
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    pub(crate) fn new(principal: Principal, implies: Option<ScopeImplies>) -> Self {
        let digest = principal.digest();
        Self {
            principal: Arc::new(principal),
            implies,
            digest,
        }
    }

    /// The required scopes that the token does not grant.
    pub(crate) fn missing<'a>(&self, required: &'a [String]) -> Vec<&'a str> {
        required
            .iter()
            .filter(|needed| {
                !self.principal.scopes.iter().any(|granted| {
                    granted == *needed || self.implies.is_some_and(|f| f(granted, needed))
                })
            })
            .map(String::as_str)
            .collect()
    }

    /// Checks a handler's scopes; the error lists every scope it needs.
    pub(crate) fn check(&self, required: &[String]) -> Result<(), McpError> {
        if self.missing(required).is_empty() {
            Ok(())
        } else {
            Err(McpError::insufficient_scope(required))
        }
    }
}

impl Ctx {
    /// The caller authenticated by the HTTP transport's OAuth layer; `None`
    /// when the server does not require authorization (or over stdio).
    #[must_use]
    pub fn principal(&self) -> Option<Arc<Principal>> {
        self.transport()
            .get::<AuthContext>()
            .map(|auth| Arc::clone(&auth.principal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(scopes: &[&str], implies: Option<ScopeImplies>) -> AuthContext {
        AuthContext::new(
            Principal::new("u").with_scopes(scopes.iter().copied()),
            implies,
        )
    }

    #[test]
    fn missing_scopes_are_listed_together() {
        let required = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(auth(&["b"], None).missing(&required), ["a", "c"]);
        assert!(auth(&["a", "b", "c"], None).check(&required).is_ok());
    }

    #[test]
    fn scope_hierarchies() {
        fn implies(granted: &str, required: &str) -> bool {
            required.starts_with(&format!("{granted}:"))
        }
        let required = vec!["files:read".to_owned()];
        assert!(auth(&["files"], Some(implies)).check(&required).is_ok());
        assert!(auth(&["files"], None).check(&required).is_err());
    }

    #[test]
    fn digests_differ_by_issuer_and_subject() {
        let a = Principal::new("alice").with_issuer("https://a");
        let b = Principal::new("alice").with_issuer("https://b");
        assert_ne!(a.digest(), b.digest());
        assert_eq!(a.digest(), a.clone().digest());
        assert_eq!(a.digest().len(), 32);
    }
}
