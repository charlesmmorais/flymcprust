//! Resource definitions and contents.
//!
//! Resources are usually declared with the `#[resource]` macro, which
//! implements [`Resource`] for a marker type. [`ResourceDef`] can also be built
//! by hand.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;

use crate::completion::{CompleteFn, Completer};
use crate::context::Ctx;
use crate::spec::{self, BlobResourceContents, CacheScope, TextResourceContents};
use crate::tool::{IntoToolError, Json, StateRequirement, ToolError};
use crate::uri_template::UriVars;

/// One item of a `resources/read` result.
pub type ResourceContents = spec::ReadResourceResultContentsItem;

/// The future returned by a resource read.
pub type ResourceFuture =
    Pin<Box<dyn Future<Output = Result<Vec<ResourceContents>, ToolError>> + Send + 'static>>;

/// Entry point of a resource: receives the requested URI and, for templates,
/// the variables extracted from it.
pub type ReadFn = fn(uri: &str, vars: &UriVars, ctx: Ctx) -> Result<ResourceFuture, ToolError>;

/// A resource that can be registered with
/// [`ServerBuilder::resource`](crate::ServerBuilder::resource).
pub trait Resource: 'static {
    /// Builds the resource definition. Called once, at registration.
    fn definition() -> ResourceDef;
}

#[derive(Debug, Clone)]
pub(crate) enum ResourceKind {
    Static(spec::Resource),
    Template(spec::ResourceTemplate),
}

/// A registered resource or resource template.
#[derive(Debug, Clone)]
pub struct ResourceDef {
    pub(crate) kind: ResourceKind,
    pub(crate) read: ReadFn,
    pub(crate) required_state: Vec<StateRequirement>,
    pub(crate) ttl: Duration,
    pub(crate) cache_scope: CacheScope,
    pub(crate) completers: Vec<Completer>,
    pub(crate) scopes: Vec<String>,
}

impl ResourceDef {
    fn with_kind(kind: ResourceKind, read: ReadFn) -> Self {
        Self {
            kind,
            read,
            required_state: Vec::new(),
            ttl: Duration::ZERO,
            cache_scope: CacheScope::Private,
            completers: Vec::new(),
            scopes: Vec::new(),
        }
    }

    /// Declares the OAuth scopes a caller needs (all of them). Enforced when
    /// the HTTP transport requires OAuth; the client gets `403` with an
    /// `insufficient_scope` challenge listing these scopes.
    #[must_use]
    pub fn scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// The OAuth scopes declared with [`scopes`](Self::scopes).
    #[must_use]
    pub fn required_scopes(&self) -> &[String] {
        &self.scopes
    }

    /// A resource with a fixed URI (listed by `resources/list`).
    #[must_use]
    pub fn new(resource: spec::Resource, read: ReadFn) -> Self {
        Self::with_kind(ResourceKind::Static(resource), read)
    }

    /// A resource template (listed by `resources/templates/list`). The
    /// `uriTemplate` must use the supported RFC 6570 subset (see
    /// [`UriTemplate`](crate::UriTemplate)).
    #[must_use]
    pub fn template(template: spec::ResourceTemplate, read: ReadFn) -> Self {
        Self::with_kind(ResourceKind::Template(template), read)
    }

    /// Declares that the resource needs shared state of type `T`.
    #[must_use]
    pub fn require_state<T: Send + Sync + 'static>(mut self) -> Self {
        self.required_state.push(StateRequirement::of::<T>());
        self
    }

    /// Freshness hint (`ttlMs`) of read results. Default: zero (always
    /// re-fetch).
    #[must_use]
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// `cacheScope` of read results. Default: `private`, since contents often
    /// depend on the caller. Use `public` for data identical for everyone.
    #[must_use]
    pub fn cache_scope(mut self, scope: CacheScope) -> Self {
        self.cache_scope = scope;
        self
    }

    /// Attaches a completion provider to a template variable.
    #[must_use]
    pub fn complete(mut self, variable: impl Into<String>, complete: CompleteFn) -> Self {
        self.completers.push(Completer {
            argument: variable.into(),
            complete,
        });
        self
    }

    /// The URI (static resources) or URI template.
    #[must_use]
    pub fn uri(&self) -> &str {
        match &self.kind {
            ResourceKind::Static(r) => &r.uri,
            ResourceKind::Template(t) => &t.uri_template,
        }
    }

    /// Whether this is a resource template.
    #[must_use]
    pub fn is_template(&self) -> bool {
        matches!(self.kind, ResourceKind::Template(_))
    }
}

/// Conversion of a resource handler's return value into contents.
///
/// | Return type | Contents |
/// | --- | --- |
/// | `String`, `&'static str`, `Cow<'static, str>` | Text |
/// | `Vec<u8>` | Binary (base64 `blob`) |
/// | `Json<T>` | JSON text (`application/json` unless another MIME type is set) |
/// | `TextResourceContents`, `BlobResourceContents`, `ResourceContents`, `Vec<ResourceContents>` | As given |
/// | `Result<T, E>` | `T` as above; `E` as for tools |
///
/// The `uri` of generated contents is the requested URI, and their MIME type
/// is the one declared on the resource.
pub trait IntoResourceContents {
    /// Performs the conversion.
    ///
    /// # Errors
    ///
    /// Returns the handler's error, or an internal error if serialization
    /// fails.
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError>;
}

fn text(uri: &str, mime_type: Option<&str>, text: String) -> Vec<ResourceContents> {
    let mut contents = TextResourceContents::new(text, uri.to_owned());
    contents.mime_type = mime_type.map(str::to_owned);
    vec![ResourceContents::Text(contents)]
}

impl IntoResourceContents for String {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(text(uri, mime_type, self))
    }
}

impl IntoResourceContents for &'static str {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(text(uri, mime_type, self.to_owned()))
    }
}

impl IntoResourceContents for Cow<'static, str> {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(text(uri, mime_type, self.into_owned()))
    }
}

impl IntoResourceContents for Vec<u8> {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        let blob = base64::engine::general_purpose::STANDARD.encode(self);
        let mut contents = BlobResourceContents::new(blob, uri.to_owned());
        contents.mime_type = mime_type.map(str::to_owned);
        Ok(vec![ResourceContents::Blob(contents)])
    }
}

impl<T: Serialize> IntoResourceContents for Json<T> {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        let json = serde_json::to_string(&self.0).map_err(ToolError::internal)?;
        Ok(text(
            uri,
            Some(mime_type.unwrap_or("application/json")),
            json,
        ))
    }
}

impl IntoResourceContents for TextResourceContents {
    fn into_contents(self, _: &str, _: Option<&str>) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(vec![ResourceContents::Text(self)])
    }
}

impl IntoResourceContents for BlobResourceContents {
    fn into_contents(self, _: &str, _: Option<&str>) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(vec![ResourceContents::Blob(self)])
    }
}

impl IntoResourceContents for ResourceContents {
    fn into_contents(self, _: &str, _: Option<&str>) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(vec![self])
    }
}

impl IntoResourceContents for Vec<ResourceContents> {
    fn into_contents(self, _: &str, _: Option<&str>) -> Result<Vec<ResourceContents>, ToolError> {
        Ok(self)
    }
}

impl<T: IntoResourceContents, E: IntoToolError> IntoResourceContents for Result<T, E> {
    fn into_contents(
        self,
        uri: &str,
        mime_type: Option<&str>,
    ) -> Result<Vec<ResourceContents>, ToolError> {
        match self {
            Ok(value) => value.into_contents(uri, mime_type),
            Err(error) => Err(error.into_tool_error()),
        }
    }
}

/// Parses a template variable for a resource handler. A value that does not
/// parse means no such resource exists.
///
/// # Errors
///
/// Returns [`ToolError::not_found`] if the variable is missing or invalid.
pub fn parse_uri_var<T: std::str::FromStr>(vars: &UriVars, name: &str) -> Result<T, ToolError> {
    vars.get(name)
        .and_then(|v| v.parse().ok())
        .ok_or_else(ToolError::not_found)
}

/// Like [`parse_uri_var`] for optional (query) variables.
///
/// # Errors
///
/// Returns [`ToolError::not_found`] if the variable is present but invalid.
pub fn parse_optional_uri_var<T: std::str::FromStr>(
    vars: &UriVars,
    name: &str,
) -> Result<Option<T>, ToolError> {
    vars.get(name)
        .map(|v| v.parse().map_err(|_| ToolError::not_found()))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_conversions() {
        let t = "hi".into_contents("a://b", Some("text/plain")).unwrap();
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!([{ "uri": "a://b", "mimeType": "text/plain", "text": "hi" }])
        );
        let b = vec![1u8, 2, 3].into_contents("a://b", None).unwrap();
        assert_eq!(
            serde_json::to_value(&b).unwrap(),
            serde_json::json!([{ "uri": "a://b", "blob": "AQID" }])
        );
        let j = Json(serde_json::json!({ "x": 1 }))
            .into_contents("a://b", None)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&j).unwrap(),
            serde_json::json!([{ "uri": "a://b", "mimeType": "application/json", "text": "{\"x\":1}" }])
        );
    }
}
