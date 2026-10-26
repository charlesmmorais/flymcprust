//! MCP Apps (`io.modelcontextprotocol/ui`): tools that render an
//! interactive HTML interface in the host.
//!
//! A server pairs a tool with a UI resource:
//!
//! - the resource has a `ui://` URI and the `text/html;profile=mcp-app` MIME
//!   type, and returns an HTML5 document. Mark it with
//!   [`ResourceDef::app`](crate::ResourceDef::app) (or `#[resource(.., app)]`),
//!   which also describes what the sandboxed page may load ([`AppUi`]);
//! - the tool names the resource with [`ToolDef::ui`](crate::ToolDef::ui)
//!   (or `#[tool(ui = "ui://..")]`), and may restrict who can call it with
//!   [`ToolDef::visibility`](crate::ToolDef::visibility).
//!
//! The server declares the extension in `server/discover` automatically.
//! Tools with a UI must still return meaningful `content`: hosts without
//! MCP Apps support show that text, and the model reads it. Check
//! [`Ctx::client_supports_apps`](crate::Ctx::client_supports_apps) to adapt
//! a result.

use serde_json::{Map, Value, json};

use crate::context::Ctx;

/// The extension identifier.
pub const EXTENSION_ID: &str = "io.modelcontextprotocol/ui";

/// The MIME type of UI resources.
pub const MIME_TYPE: &str = "text/html;profile=mcp-app";

/// The URI scheme of UI resources.
pub(crate) const URI_SCHEME: &str = "ui://";

/// Who may call a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Visibility {
    /// The model (the tool is in the agent's tool list).
    Model,
    /// The app's interface (the page can call it through the host).
    App,
}

impl Visibility {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::App => "app",
        }
    }
}

/// A browser capability a UI asks the host for. Hosts may refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// The camera.
    Camera,
    /// The microphone.
    Microphone,
    /// The device's location.
    Geolocation,
    /// Writing to the clipboard.
    ClipboardWrite,
}

impl Permission {
    /// The wire key.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::Geolocation => "geolocation",
            Self::ClipboardWrite => "clipboardWrite",
        }
    }
}

/// How the host should sandbox a UI resource (`_meta.ui`).
///
/// By default the page may load nothing from the network, embed no frames
/// and gets no extra permissions: declare every origin it needs. Hosts
/// enforce this with a Content Security Policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppUi {
    connect_domains: Vec<String>,
    resource_domains: Vec<String>,
    frame_domains: Vec<String>,
    base_uri_domains: Vec<String>,
    permissions: Vec<Permission>,
    domain: Option<String>,
    prefers_border: Option<bool>,
}

fn strings<I, S>(items: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    items.into_iter().map(Into::into).collect()
}

impl AppUi {
    /// No network access, no frames, no permissions.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Origins the page may connect to with `fetch`, XHR or WebSocket
    /// (`connect-src`), e.g. `https://api.example.com`.
    #[must_use]
    pub fn connect_domains<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.connect_domains = strings(origins);
        self
    }

    /// Origins of scripts, styles, images, fonts and media; wildcard
    /// subdomains such as `https://*.example.com` are allowed.
    #[must_use]
    pub fn resource_domains<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.resource_domains = strings(origins);
        self
    }

    /// Origins the page may embed in frames (`frame-src`).
    #[must_use]
    pub fn frame_domains<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.frame_domains = strings(origins);
        self
    }

    /// Origins allowed in the document's `<base>` element (`base-uri`).
    #[must_use]
    pub fn base_uri_domains<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.base_uri_domains = strings(origins);
        self
    }

    /// Browser capabilities the page asks for.
    #[must_use]
    pub fn permissions(mut self, permissions: impl IntoIterator<Item = Permission>) -> Self {
        self.permissions = permissions.into_iter().collect();
        self
    }

    /// A dedicated origin for the page; the format is host-specific (see
    /// the host's documentation).
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Whether the host should draw a border around the page.
    #[must_use]
    pub fn prefers_border(mut self, border: bool) -> Self {
        self.prefers_border = Some(border);
        self
    }

    /// Checks the origins: hosts paste them into a Content Security
    /// Policy, so separators or quotes could widen it.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let lists = [
            ("connect", &self.connect_domains),
            ("resource", &self.resource_domains),
            ("frame", &self.frame_domains),
            ("base URI", &self.base_uri_domains),
        ];
        for (kind, origins) in lists {
            for origin in origins {
                if !is_origin(origin) {
                    return Err(format!(
                        "invalid {kind} origin `{origin}`: expected `scheme://host[:port]`, \
                         such as `https://api.example.com` or `https://*.example.com`"
                    ));
                }
            }
        }
        if let Some(domain) = &self.domain
            && (domain.is_empty() || !domain.chars().all(is_safe_char))
        {
            return Err(format!("invalid domain `{domain}`"));
        }
        Ok(())
    }

    /// The `_meta.ui` object of the resource.
    pub(crate) fn to_meta(&self) -> Value {
        let mut ui = Map::new();
        let mut csp = Map::new();
        for (key, origins) in [
            ("connectDomains", &self.connect_domains),
            ("resourceDomains", &self.resource_domains),
            ("frameDomains", &self.frame_domains),
            ("baseUriDomains", &self.base_uri_domains),
        ] {
            if !origins.is_empty() {
                csp.insert(key.to_owned(), json!(origins));
            }
        }
        if !csp.is_empty() {
            ui.insert("csp".to_owned(), Value::Object(csp));
        }
        if !self.permissions.is_empty() {
            let permissions: Map<String, Value> = self
                .permissions
                .iter()
                .map(|p| (p.as_str().to_owned(), json!({})))
                .collect();
            ui.insert("permissions".to_owned(), Value::Object(permissions));
        }
        if let Some(domain) = &self.domain {
            ui.insert("domain".to_owned(), Value::from(domain.clone()));
        }
        if let Some(border) = self.prefers_border {
            ui.insert("prefersBorder".to_owned(), Value::from(border));
        }
        Value::Object(ui)
    }
}

/// Characters that cannot break out of a CSP source expression.
fn is_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '*' | '_' | '[' | ']')
}

/// `scheme://host[:port]`, with an optional `*.` wildcard label.
fn is_origin(origin: &str) -> bool {
    let Some((scheme, host)) = origin.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let host_ok = !host.is_empty()
        && host.chars().all(is_safe_char)
        && host.trim_start_matches("*.").chars().all(|c| c != '*');
    scheme_ok && host_ok
}

/// The `_meta` entries of a tool with a UI or a restricted visibility.
pub(crate) fn tool_meta(
    meta: &mut Option<Map<String, Value>>,
    resource_uri: Option<&str>,
    visibility: Option<&[Visibility]>,
) {
    if resource_uri.is_none() && visibility.is_none() {
        return;
    }
    let mut ui = Map::new();
    if let Some(uri) = resource_uri {
        ui.insert("resourceUri".to_owned(), Value::from(uri));
    }
    if let Some(visibility) = visibility {
        ui.insert(
            "visibility".to_owned(),
            json!(visibility.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
        );
    }
    let meta = meta.get_or_insert_with(Map::new);
    meta.insert("ui".to_owned(), Value::Object(ui));
    if let Some(uri) = resource_uri {
        // Deprecated flat key, still read by hosts that predate `_meta.ui`;
        // the specification drops it before its final release.
        meta.insert("ui/resourceUri".to_owned(), Value::from(uri));
    }
}

/// Repeats an interface's `_meta.ui` in its read results and sets the MIME
/// type when the handler left it out.
pub(crate) fn mark_contents(contents: &mut [crate::resource::ResourceContents], ui: &Value) {
    use crate::spec::ReadResourceResultContentsItem as Item;
    for item in contents {
        let (meta, mime) = match item {
            Item::Text(t) => (&mut t.meta, &mut t.mime_type),
            Item::Blob(b) => (&mut b.meta, &mut b.mime_type),
        };
        mime.get_or_insert_with(|| MIME_TYPE.to_owned());
        meta.get_or_insert_with(Map::new)
            .entry("ui")
            .or_insert_with(|| ui.clone());
    }
}

impl Ctx {
    /// Whether the client can render MCP Apps interfaces (it declared the
    /// extension with the `text/html;profile=mcp-app` MIME type). A tool
    /// can return a richer text answer when it cannot.
    #[must_use]
    pub fn client_supports_apps(&self) -> bool {
        self.client_capabilities()
            .extensions
            .as_ref()
            .and_then(|e| e.get(EXTENSION_ID))
            .and_then(|settings| settings.get("mimeTypes"))
            .and_then(Value::as_array)
            .is_some_and(|types| types.iter().any(|t| t.as_str() == Some(MIME_TYPE)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_shapes() {
        let ui = AppUi::new()
            .connect_domains(["https://api.example.com"])
            .resource_domains(["https://*.cdn.example.com"])
            .permissions([Permission::ClipboardWrite])
            .prefers_border(true);
        assert!(ui.validate().is_ok());
        assert_eq!(
            ui.to_meta(),
            json!({
                "csp": {
                    "connectDomains": ["https://api.example.com"],
                    "resourceDomains": ["https://*.cdn.example.com"],
                },
                "permissions": { "clipboardWrite": {} },
                "prefersBorder": true,
            })
        );
        assert_eq!(AppUi::new().to_meta(), json!({}));

        let mut meta = None;
        tool_meta(&mut meta, Some("ui://x/view"), Some(&[Visibility::App]));
        assert_eq!(
            Value::Object(meta.unwrap()),
            json!({
                "ui": { "resourceUri": "ui://x/view", "visibility": ["app"] },
                "ui/resourceUri": "ui://x/view",
            })
        );
    }

    #[test]
    fn origins_cannot_widen_the_policy() {
        for bad in [
            "api.example.com",
            "https://a.example.com; script-src *",
            "https://a.example.com 'unsafe-inline'",
            "https://a.example.com/path",
            "https://*",
            "https://a.*.example.com",
            "",
        ] {
            assert!(
                AppUi::new().connect_domains([bad]).validate().is_err(),
                "{bad}"
            );
        }
        for good in [
            "https://api.example.com",
            "wss://live.example.com:8443",
            "https://*.example.com",
            "http://localhost:3000",
        ] {
            assert!(
                AppUi::new().connect_domains([good]).validate().is_ok(),
                "{good}"
            );
        }
    }
}
