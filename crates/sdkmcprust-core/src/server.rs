//! The MCP server: builder, immutable registry and request dispatch.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::FutureExt as _;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::auth::AuthContext;
use crate::completion::{Completer, CompletionRequest};
use crate::context::{Ctx, RequestMeta, TransportContext, TypeMap};
use crate::error::McpError;
use crate::extension::{self, Extension};
use crate::hints::{self, Once};
use crate::jsonrpc::{self, Message, Rejection, Request, Response};
use crate::legacy::{LegacyConfig, LegacySession};
use crate::limits::Limits;
use crate::mirror::{self, MirroredParam};
use crate::mrtr::{Args, MIN_REQUEST_STATE_KEY_BYTES, Round, RoundKey, StateCodec};
use crate::notify::{Notifier, NotifyChannel};
use crate::prompt::{GetFn, Prompt, PromptArguments, PromptDef};
use crate::resource::{ReadFn, Resource, ResourceDef, ResourceKind};
use crate::spec::{self, CacheScope, RequestId, methods};
use crate::tool::{CallFn, StateRequirement, Tool, ToolDef, ToolError, ToolOutput};
use crate::uri_template::{self, UriTemplate, UriVars};

/// The `resultType` of final results.
const COMPLETE: &str = "complete";

/// Errors detected while building a server.
///
/// `Display` gives the problem in one line; [`BuildError::help`] says how to
/// fix it. `Debug` prints both, so `fn main() -> Result<..>` with
/// `.build()?` shows the fix too.
#[derive(thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// Two tools share a name.
    #[error("duplicate tool name `{0}`")]
    DuplicateTool(String),
    /// A tool name breaks the specification's naming rules.
    #[error("invalid tool name `{0}`: use 1-128 characters from A-Z, a-z, 0-9, `_`, `-` and `.`")]
    InvalidToolName(String),
    /// A handler needs shared state that was not registered.
    #[error("`{handler}` needs shared state of type `{type_name}`, which is not registered")]
    MissingState {
        /// The tool or prompt name, or the resource URI.
        handler: String,
        /// The missing type.
        type_name: &'static str,
    },
    /// A resource URI or URI template is invalid.
    #[error("invalid resource `{uri}`: {reason}")]
    InvalidResource {
        /// The URI or URI template.
        uri: String,
        /// What is wrong with it.
        reason: String,
    },
    /// Two resources share a URI, or two templates share a URI template.
    #[error("duplicate resource `{0}`")]
    DuplicateResource(String),
    /// Two prompts share a name.
    #[error("duplicate prompt name `{0}`")]
    DuplicatePrompt(String),
    /// A prompt name is empty or contains control characters.
    #[error("invalid prompt name `{0}`")]
    InvalidPromptName(String),
    /// A completion provider is attached to an unknown argument.
    #[error("`{owner}` has a completion provider for unknown argument `{argument}`")]
    InvalidCompletion {
        /// The prompt name or URI template.
        owner: String,
        /// The argument or template variable.
        argument: String,
    },
    /// An `x-mcp-header` annotation breaks the specification's constraints.
    #[error("tool `{tool}` has an invalid `x-mcp-header` annotation: {reason}")]
    InvalidHeaderAnnotation {
        /// The tool name.
        tool: String,
        /// The violated constraint.
        reason: String,
    },
    /// An extension identifier is malformed or registered twice.
    #[error("invalid or duplicate extension identifier `{0}`")]
    InvalidExtension(String),
    /// A limit is out of range.
    #[error("invalid limit: {0}")]
    InvalidLimit(&'static str),
    /// The `requestState` signing key is shorter than
    /// [`MIN_REQUEST_STATE_KEY_BYTES`].
    #[error("the requestState key must be at least {MIN_REQUEST_STATE_KEY_BYTES} bytes")]
    WeakRequestStateKey,
    /// No random `requestState` key could be generated.
    #[error("could not generate a random requestState key: {0}")]
    RandomUnavailable(String),
    /// An MCP Apps interface or a tool's `ui` is invalid.
    #[error("invalid MCP Apps interface `{uri}`: {reason}")]
    InvalidApp {
        /// The `ui://` URI (or the tool name).
        uri: String,
        /// What is wrong.
        reason: String,
    },
    /// A handler declares a malformed OAuth scope.
    #[error("`{handler}` declares an invalid scope `{scope}`")]
    InvalidScope {
        /// The tool or prompt name, or the resource URI.
        handler: String,
        /// The scope.
        scope: String,
    },
    /// A pre-serialized response could not be built.
    #[error("could not serialize {0}")]
    Serialize(&'static str, #[source] serde_json::Error),
}

impl BuildError {
    /// How to fix the problem.
    #[must_use]
    pub fn help(&self) -> String {
        match self {
            Self::DuplicateTool(name) => format!(
                "rename one of the tools with #[tool(name = \"...\")], or check that `{name}` \
                 is not registered twice (for example with `.tool::<..>()` and `.auto_tools()`)"
            ),
            Self::InvalidToolName(_) => "tool names travel in HTTP headers; rename the tool with \
                 #[tool(name = \"...\")] using only those characters"
                .to_owned(),
            Self::MissingState { type_name, .. } => format!(
                "register a value before `.build()`: `Server::builder(..).state(value)`, \
                 where `value: {type_name}`"
            ),
            Self::InvalidResource { .. } => "fixed URIs need a scheme, such as `config://app` or \
                 `file:///notes.txt`; templates support `{var}`, `{+var}`, `{/var}` and a final \
                 `{?query}` (see the `UriTemplate` docs)"
                .to_owned(),
            Self::DuplicateResource(_) => {
                "each URI and each URI template can be registered only once".to_owned()
            }
            Self::DuplicatePrompt(_) => {
                "rename one of the prompts with #[prompt(name = \"...\")]".to_owned()
            }
            Self::InvalidPromptName(_) => {
                "use a non-empty name without control characters".to_owned()
            }
            Self::InvalidCompletion { owner, .. } => format!(
                "#[complete(..)] and `.complete(..)` must name an argument (prompts) or a \
                 template variable (resources) of `{owner}`"
            ),
            Self::InvalidHeaderAnnotation { .. } => "`x-mcp-header` takes an HTTP header name \
                 (letters, digits and `-`) on a string, integer or boolean property reached \
                 only through `properties`"
                .to_owned(),
            Self::InvalidExtension(_) => "extension identifiers look like \
                 `com.example/my-extension` and can be registered only once"
                .to_owned(),
            Self::InvalidLimit(_) => "limits must be positive; see `max_message_bytes`, \
                 `max_json_depth`, `tool_timeout` and `max_concurrent_calls` on the builder"
                .to_owned(),
            Self::WeakRequestStateKey => format!(
                "use at least {MIN_REQUEST_STATE_KEY_BYTES} random bytes from your secret store, \
                 and the same key on every instance"
            ),
            Self::RandomUnavailable(_) => "the operating system did not provide randomness; set \
                 a key explicitly with `.request_state_key(..)`"
                .to_owned(),
            Self::InvalidApp { .. } => "MCP Apps interfaces are fixed `ui://` resources marked \
                 with #[resource(\"ui://..\", app)] (or `ResourceDef::app`), and \
                 #[tool(ui = \"ui://..\")] must name one of them; CSP origins look like \
                 `https://api.example.com`"
                .to_owned(),
            Self::InvalidScope { .. } => "scopes are non-empty and use visible ASCII except `\"` \
                 and `\\`, without spaces (for example `files:read`)"
                .to_owned(),
            Self::Serialize(..) => "a definition built by hand contains a value JSON cannot \
                 represent (such as a NaN number); if it came from a macro, please report a bug"
                .to_owned(),
        }
    }
}

impl std::fmt::Debug for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}\n  = help: {}", self.help())
    }
}

/// Builds a [`Server`].
#[must_use = "call `.build()` to create the server"]
pub struct ServerBuilder {
    info: spec::Implementation,
    instructions: Option<String>,
    tools: Vec<ToolDef>,
    resources: Vec<ResourceDef>,
    prompts: Vec<PromptDef>,
    state: TypeMap,
    extensions: Vec<Box<dyn Extension>>,
    limits: Limits,
    list_ttl: Duration,
    discover_ttl: Duration,
    cache_scope: CacheScope,
    include_server_info: bool,
    request_state_key: Option<Vec<u8>>,
    request_state_ttl: Duration,
    legacy_clients: bool,
    legacy_session_ttl: Duration,
}

impl std::fmt::Debug for ServerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerBuilder")
            .field("info", &self.info)
            .field("tools", &self.tools.len())
            .field("resources", &self.resources.len())
            .field("prompts", &self.prompts.len())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ServerBuilder {
    /// Starts a server with its name and version (reported as `serverInfo`).
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            info: spec::Implementation::new(name.into(), version.into()),
            instructions: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            state: TypeMap::default(),
            extensions: Vec::new(),
            limits: Limits::default(),
            list_ttl: Duration::from_secs(300),
            discover_ttl: Duration::from_secs(3600),
            cache_scope: CacheScope::Public,
            include_server_info: true,
            request_state_key: None,
            request_state_ttl: Duration::from_secs(600),
            legacy_clients: true,
            legacy_session_ttl: Duration::from_secs(24 * 3600),
        }
    }

    /// Sets the human-readable title in `serverInfo`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.info.title = Some(title.into());
        self
    }

    /// Sets the natural-language guidance returned by `server/discover`.
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Registers a tool declared with `#[tool]` (or implementing [`Tool`]).
    pub fn tool<T: Tool>(mut self) -> Self {
        self.tools.push(T::definition());
        self
    }

    /// Registers every `#[tool]` linked into the binary (feature
    /// `auto-register`). Tools can still be added explicitly; duplicates are
    /// reported by [`build`](Self::build).
    #[cfg(feature = "auto-register")]
    pub fn auto_tools(mut self) -> Self {
        for tool in inventory::iter::<crate::auto::AutoTool> {
            self.tools.push(tool.definition());
        }
        self
    }

    /// Registers a tool definition built by hand.
    pub fn tool_def(mut self, def: ToolDef) -> Self {
        self.tools.push(def);
        self
    }

    /// Registers a resource or resource template declared with `#[resource]`
    /// (or implementing [`Resource`]).
    pub fn resource<T: Resource>(mut self) -> Self {
        self.resources.push(T::definition());
        self
    }

    /// Registers every `#[resource]` linked into the binary (feature
    /// `auto-register`).
    #[cfg(feature = "auto-register")]
    pub fn auto_resources(mut self) -> Self {
        for resource in inventory::iter::<crate::auto::AutoResource> {
            self.resources.push(resource.definition());
        }
        self
    }

    /// Registers a resource definition built by hand.
    pub fn resource_def(mut self, def: ResourceDef) -> Self {
        self.resources.push(def);
        self
    }

    /// Registers a prompt declared with `#[prompt]` (or implementing
    /// [`Prompt`]).
    pub fn prompt<T: Prompt>(mut self) -> Self {
        self.prompts.push(T::definition());
        self
    }

    /// Registers every `#[prompt]` linked into the binary (feature
    /// `auto-register`).
    #[cfg(feature = "auto-register")]
    pub fn auto_prompts(mut self) -> Self {
        for prompt in inventory::iter::<crate::auto::AutoPrompt> {
            self.prompts.push(prompt.definition());
        }
        self
    }

    /// Registers a prompt definition built by hand.
    pub fn prompt_def(mut self, def: PromptDef) -> Self {
        self.prompts.push(def);
        self
    }

    /// Registers shared state, available to handlers through `State<T>`.
    pub fn state<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.state.insert(Arc::new(value));
        self
    }

    /// Registers a protocol extension.
    pub fn extension(mut self, extension: impl Extension) -> Self {
        self.extensions.push(Box::new(extension));
        self
    }

    /// Replaces all limits at once.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Maximum size of one message in bytes. Default: 4 MiB.
    pub fn max_message_bytes(mut self, bytes: usize) -> Self {
        self.limits.max_message_bytes = bytes;
        self
    }

    /// Maximum JSON nesting depth. Default: 64 (at most 128).
    pub fn max_json_depth(mut self, depth: usize) -> Self {
        self.limits.max_json_depth = depth;
        self
    }

    /// Maximum duration of one tool call, resource read or prompt. Default:
    /// 30 s.
    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.limits.tool_timeout = timeout;
        self
    }

    /// Maximum number of concurrent handler invocations (tools, resources and
    /// prompts). Default: 64.
    pub fn max_concurrent_calls(mut self, calls: usize) -> Self {
        self.limits.max_concurrent_calls = calls;
        self
    }

    /// Freshness hint (`ttlMs`) for `tools/list`, `resources/list`,
    /// `resources/templates/list` and `prompts/list`. Default: 5 minutes.
    pub fn list_ttl(mut self, ttl: Duration) -> Self {
        self.list_ttl = ttl;
        self
    }

    /// Freshness hint (`ttlMs`) for `server/discover`. Default: 1 hour.
    pub fn discover_ttl(mut self, ttl: Duration) -> Self {
        self.discover_ttl = ttl;
        self
    }

    /// `cacheScope` of list and discovery results. Default: `public`. Use
    /// `private` when results depend on the caller's authorization.
    pub fn cache_scope(mut self, scope: CacheScope) -> Self {
        self.cache_scope = scope;
        self
    }

    /// Secret key that signs `requestState` (HMAC-SHA256), at least
    /// [`MIN_REQUEST_STATE_KEY_BYTES`] bytes. Default: a random key per
    /// process, which is enough for a single instance. Set the same key on
    /// every instance when requests may be retried on another instance
    /// (load balancing), and keep it secret.
    pub fn request_state_key(mut self, key: impl Into<Vec<u8>>) -> Self {
        self.request_state_key = Some(key.into());
        self
    }

    /// How long a client may take to answer an input request (the lifetime
    /// of `requestState`). Default: 10 minutes.
    pub fn request_state_ttl(mut self, ttl: Duration) -> Self {
        self.request_state_ttl = ttl;
        self
    }

    /// Whether clients of the older protocol versions (2025-11-25 and
    /// 2025-06-18, which open with `initialize`) are served. Default: `true`.
    /// Handlers do not change: the SDK adapts the protocol, including
    /// `ctx.elicit`, which becomes a request to the client.
    pub fn legacy_clients(mut self, enabled: bool) -> Self {
        self.legacy_clients = enabled;
        self
    }

    /// Lifetime of the signed `MCP-Session-Id` given to legacy HTTP clients;
    /// they reconnect with `initialize` after it. Default: 24 hours.
    pub fn legacy_session_ttl(mut self, ttl: Duration) -> Self {
        self.legacy_session_ttl = ttl;
        self
    }

    /// Whether results carry `io.modelcontextprotocol/serverInfo` in `_meta`
    /// (recommended by the specification). Default: `true`.
    pub fn include_server_info(mut self, include: bool) -> Self {
        self.include_server_info = include;
        self
    }

    /// Validates the configuration and builds the server.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] for duplicate or invalid names and URIs,
    /// missing state, invalid completion providers, invalid extensions,
    /// out-of-range limits, or serialization failures of the pre-built
    /// responses.
    pub fn build(self) -> Result<Server, BuildError> {
        if self.limits.max_message_bytes == 0 {
            return Err(BuildError::InvalidLimit(
                "max_message_bytes must be positive",
            ));
        }
        if self.limits.max_json_depth == 0 {
            return Err(BuildError::InvalidLimit("max_json_depth must be positive"));
        }
        if self.limits.max_concurrent_calls == 0 {
            return Err(BuildError::InvalidLimit(
                "max_concurrent_calls must be positive",
            ));
        }
        if self.limits.tool_timeout.is_zero() {
            return Err(BuildError::InvalidLimit("tool_timeout must be positive"));
        }

        let mut tools = self.tools;
        // Deterministic order, as the specification recommends for caching.
        tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));
        for pair in tools.windows(2) {
            if let [a, b] = pair
                && a.tool.name == b.tool.name
            {
                return Err(BuildError::DuplicateTool(a.tool.name.clone()));
            }
        }
        for def in &mut tools {
            if def.ui.is_some() || def.visibility.is_some() {
                if def.visibility.as_ref().is_some_and(Vec::is_empty) {
                    return Err(BuildError::InvalidApp {
                        uri: def.tool.name.clone(),
                        reason: "`visibility` must name `model`, `app` or both".to_owned(),
                    });
                }
                crate::apps::tool_meta(
                    &mut def.tool.meta,
                    def.ui.as_deref(),
                    def.visibility.as_deref(),
                );
            }
        }
        let mut mirrored = Vec::with_capacity(tools.len());
        for def in &tools {
            if !is_valid_tool_name(&def.tool.name) {
                return Err(BuildError::InvalidToolName(def.tool.name.clone()));
            }
            check_state(&self.state, &def.required_state, &def.tool.name)?;
            check_scopes(&def.scopes, &def.tool.name)?;
            let params = mirror::collect(&def.tool.input_schema.extra).map_err(|reason| {
                BuildError::InvalidHeaderAnnotation {
                    tool: def.tool.name.clone(),
                    reason,
                }
            })?;
            mirrored.push(params);
        }

        let state_key = match self.request_state_key {
            Some(key) if key.len() < MIN_REQUEST_STATE_KEY_BYTES => {
                return Err(BuildError::WeakRequestStateKey);
            }
            Some(key) => key,
            None => {
                let mut key = vec![0; MIN_REQUEST_STATE_KEY_BYTES];
                getrandom::fill(&mut key)
                    .map_err(|e| BuildError::RandomUnavailable(e.to_string()))?;
                key
            }
        };
        let state_codec = StateCodec::new(state_key, self.request_state_ttl);

        let resources = build_resources(self.resources, &self.state)?;
        let prompts = build_prompts(self.prompts, &self.state)?;

        // A tool's interface must be one of the registered app resources.
        for def in &tools {
            if let Some(uri) = &def.ui {
                let found = resources
                    .statics
                    .iter()
                    .any(|r| r.uri == *uri && r.app.is_some());
                if !found {
                    return Err(BuildError::InvalidApp {
                        uri: uri.clone(),
                        reason: format!(
                            "tool `{}` shows it, but no resource with this URI is marked as an \
                             app",
                            def.tool.name
                        ),
                    });
                }
            }
        }
        let uses_apps = tools
            .iter()
            .any(|t| t.ui.is_some() || t.visibility.is_some())
            || resources.statics.iter().any(|r| r.app.is_some());

        let mut extensions = BTreeMap::new();
        for ext in &self.extensions {
            let id = ext.id();
            if !extension::is_valid_id(id) || extensions.contains_key(id) {
                return Err(BuildError::InvalidExtension(id.to_owned()));
            }
            extensions.insert(id.to_owned(), ext.settings());
        }
        if uses_apps {
            // No server settings are defined for MCP Apps.
            extensions
                .entry(crate::apps::EXTENSION_ID.to_owned())
                .or_insert_with(Map::new);
        }

        let result_meta = self.include_server_info.then(|| spec::ResultMetaObject {
            server_info: Some(self.info.clone()),
            extra: Map::new(),
        });

        let has_completions = resources.templates.iter().any(|t| !t.completers.is_empty())
            || prompts.registry.iter().any(|p| !p.completers.is_empty());
        let has_resources = !resources.statics.is_empty() || !resources.templates.is_empty();
        let has_prompts = !prompts.registry.is_empty();
        #[allow(deprecated)] // `logging`: deprecated (SEP-2577) but still specified.
        let capabilities = spec::ServerCapabilities {
            tools: Some(spec::ServerCapabilitiesTools { list_changed: None }),
            resources: has_resources.then_some(spec::ServerCapabilitiesResources {
                list_changed: None,
                subscribe: None,
            }),
            prompts: has_prompts.then_some(spec::ServerCapabilitiesPrompts { list_changed: None }),
            completions: has_completions.then(Map::new),
            // Log messages are sent only to clients that ask for them.
            logging: Some(Map::new()),
            extensions: (!extensions.is_empty()).then_some(extensions),
            ..spec::ServerCapabilities::default()
        };

        let supported_versions = vec![spec::PROTOCOL_VERSION.to_owned()];

        let legacy = LegacyConfig::new(
            self.legacy_clients,
            self.legacy_session_ttl,
            &capabilities,
            &self.info,
            self.instructions.as_deref(),
        );

        let discover = spec::DiscoverResult {
            meta: result_meta.clone(),
            cache_scope: self.cache_scope,
            capabilities,
            instructions: self.instructions,
            result_type: COMPLETE.to_owned(),
            supported_versions: supported_versions.clone(),
            ttl_ms: millis(self.discover_ttl),
        };
        let discover =
            to_raw(&discover).map_err(|e| BuildError::Serialize("server/discover", e))?;

        let list = spec::ListToolsResult {
            meta: result_meta.clone(),
            cache_scope: self.cache_scope,
            next_cursor: None,
            result_type: COMPLETE.to_owned(),
            tools: tools.iter().map(|t| t.tool.clone()).collect(),
            ttl_ms: millis(self.list_ttl),
        };
        let tools_list = to_raw(&list).map_err(|e| BuildError::Serialize("tools/list", e))?;
        let list_ttl_ms = millis(self.list_ttl);

        let resources_list = if has_resources {
            Some(
                to_raw(&spec::ListResourcesResult {
                    meta: result_meta.clone(),
                    cache_scope: self.cache_scope,
                    next_cursor: None,
                    resources: resources.listed,
                    result_type: COMPLETE.to_owned(),
                    ttl_ms: list_ttl_ms,
                })
                .map_err(|e| BuildError::Serialize("resources/list", e))?,
            )
        } else {
            None
        };
        let templates_list = if has_resources {
            Some(
                to_raw(&spec::ListResourceTemplatesResult {
                    meta: result_meta.clone(),
                    cache_scope: self.cache_scope,
                    next_cursor: None,
                    resource_templates: resources.listed_templates,
                    result_type: COMPLETE.to_owned(),
                    ttl_ms: list_ttl_ms,
                })
                .map_err(|e| BuildError::Serialize("resources/templates/list", e))?,
            )
        } else {
            None
        };
        let prompts_list = if has_prompts {
            Some(
                to_raw(&spec::ListPromptsResult {
                    meta: result_meta.clone(),
                    cache_scope: self.cache_scope,
                    next_cursor: None,
                    prompts: prompts.listed,
                    result_type: COMPLETE.to_owned(),
                    ttl_ms: list_ttl_ms,
                })
                .map_err(|e| BuildError::Serialize("prompts/list", e))?,
            )
        } else {
            None
        };

        let registry = tools
            .into_iter()
            .zip(mirrored)
            .map(|(t, mirrored)| RegisteredTool {
                name: t.tool.name,
                call: t.call,
                mirrored,
                scopes: t.scopes.into(),
            })
            .collect::<Box<[RegisteredTool]>>();
        let has_scopes = registry.iter().any(|t| !t.scopes.is_empty())
            || resources.statics.iter().any(|r| !r.scopes.is_empty())
            || resources.templates.iter().any(|t| !t.scopes.is_empty())
            || prompts.registry.iter().any(|p| !p.scopes.is_empty());

        Ok(Server {
            inner: Arc::new(Inner {
                tools: registry,
                has_scopes,
                resources: resources.statics,
                templates: resources.templates,
                prompts: prompts.registry,
                has_completions,
                state_codec,
                legacy,
                legacy_hint: Once::new(),
                state_hint: Once::new(),
                discover,
                tools_list,
                resources_list,
                templates_list,
                prompts_list,
                result_meta,
                supported_versions,
                state: Arc::new(self.state),
                permits: Arc::new(Semaphore::new(self.limits.max_concurrent_calls)),
                limits: self.limits,
            }),
        })
    }
}

/// A built MCP server. Cheap to clone; all clones share one registry.
///
/// `Server` implements [`tower_service::Service<McpRequest>`], so tower
/// middleware (timeouts, rate limits, tracing) can wrap it.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
}

struct Inner {
    /// Sorted by name.
    tools: Box<[RegisteredTool]>,
    /// Sorted by URI.
    resources: Box<[RegisteredResource]>,
    /// Sorted by name, then URI template; the first match wins.
    templates: Box<[RegisteredTemplate]>,
    /// Sorted by name.
    prompts: Box<[RegisteredPrompt]>,
    has_completions: bool,
    /// Whether any handler declares OAuth scopes.
    has_scopes: bool,
    state_codec: StateCodec,
    legacy: LegacyConfig,
    legacy_hint: Once,
    state_hint: Once,
    discover: Box<RawValue>,
    tools_list: Box<RawValue>,
    resources_list: Option<Box<RawValue>>,
    templates_list: Option<Box<RawValue>>,
    prompts_list: Option<Box<RawValue>>,
    result_meta: Option<spec::ResultMetaObject>,
    supported_versions: Vec<String>,
    state: Arc<TypeMap>,
    permits: Arc<Semaphore>,
    limits: Limits,
}

struct RegisteredTool {
    name: String,
    call: CallFn,
    mirrored: Vec<MirroredParam>,
    scopes: Box<[String]>,
}

struct RegisteredResource {
    uri: String,
    read: ReadFn,
    ttl_ms: u64,
    cache_scope: CacheScope,
    scopes: Box<[String]>,
    /// `_meta.ui` of an MCP Apps interface, repeated in read results.
    app: Option<Value>,
}

struct RegisteredTemplate {
    template: UriTemplate,
    read: ReadFn,
    ttl_ms: u64,
    cache_scope: CacheScope,
    completers: Vec<Completer>,
    scopes: Box<[String]>,
}

struct RegisteredPrompt {
    name: String,
    description: Option<String>,
    get: GetFn,
    completers: Vec<Completer>,
    scopes: Box<[String]>,
}

struct BuiltResources {
    statics: Box<[RegisteredResource]>,
    templates: Box<[RegisteredTemplate]>,
    listed: Vec<spec::Resource>,
    listed_templates: Vec<spec::ResourceTemplate>,
}

struct BuiltPrompts {
    registry: Box<[RegisteredPrompt]>,
    listed: Vec<spec::Prompt>,
}

fn check_state(
    state: &TypeMap,
    required: &[StateRequirement],
    handler: &str,
) -> Result<(), BuildError> {
    match required.iter().find(|r| !state.contains(r.type_id)) {
        Some(missing) => Err(BuildError::MissingState {
            handler: handler.to_owned(),
            type_name: missing.type_name,
        }),
        None => Ok(()),
    }
}

fn check_scopes(scopes: &[String], handler: &str) -> Result<(), BuildError> {
    match scopes.iter().find(|s| !crate::auth::is_valid_scope(s)) {
        Some(scope) => Err(BuildError::InvalidScope {
            handler: handler.to_owned(),
            scope: scope.clone(),
        }),
        None => Ok(()),
    }
}

fn check_completers(
    completers: &[Completer],
    owner: &str,
    arguments: &[&str],
) -> Result<(), BuildError> {
    match completers
        .iter()
        .find(|c| !arguments.contains(&c.argument.as_str()))
    {
        Some(unknown) => Err(BuildError::InvalidCompletion {
            owner: owner.to_owned(),
            argument: unknown.argument.clone(),
        }),
        None => Ok(()),
    }
}

fn build_resources(defs: Vec<ResourceDef>, state: &TypeMap) -> Result<BuiltResources, BuildError> {
    let mut statics = Vec::new();
    let mut templates = Vec::new();
    let mut listed = Vec::new();
    let mut listed_templates = Vec::new();
    for def in defs {
        check_state(state, &def.required_state, def.uri())?;
        check_scopes(&def.scopes, def.uri())?;
        let ttl_ms = millis(def.ttl);
        match def.kind {
            ResourceKind::Static(resource) => {
                if !uri_template::is_absolute_uri(&resource.uri) {
                    return Err(BuildError::InvalidResource {
                        uri: resource.uri,
                        reason: "expected an absolute URI such as `scheme://path`".to_owned(),
                    });
                }
                check_completers(&def.completers, &resource.uri, &[])?;
                let app = match &def.app {
                    None => None,
                    Some(ui) => {
                        let invalid = |reason: String| BuildError::InvalidApp {
                            uri: resource.uri.clone(),
                            reason,
                        };
                        if !resource.uri.starts_with(crate::apps::URI_SCHEME) {
                            return Err(invalid("the URI must use the `ui://` scheme".to_owned()));
                        }
                        ui.validate().map_err(invalid)?;
                        Some(ui.to_meta())
                    }
                };
                statics.push(RegisteredResource {
                    uri: resource.uri.clone(),
                    read: def.read,
                    ttl_ms,
                    cache_scope: def.cache_scope,
                    scopes: def.scopes.into(),
                    app,
                });
                listed.push(resource);
            }
            ResourceKind::Template(template) => {
                if def.app.is_some() {
                    return Err(BuildError::InvalidApp {
                        uri: template.uri_template,
                        reason: "an interface is a fixed `ui://` resource, not a template"
                            .to_owned(),
                    });
                }
                let parsed = UriTemplate::parse(&template.uri_template).map_err(|e| {
                    BuildError::InvalidResource {
                        uri: template.uri_template.clone(),
                        reason: e.to_string(),
                    }
                })?;
                let variables: Vec<&str> = parsed.variables().collect();
                check_completers(&def.completers, &template.uri_template, &variables)?;
                templates.push((
                    template.name.clone(),
                    RegisteredTemplate {
                        template: parsed,
                        read: def.read,
                        ttl_ms,
                        cache_scope: def.cache_scope,
                        completers: def.completers,
                        scopes: def.scopes.into(),
                    },
                ));
                listed_templates.push(template);
            }
        }
    }
    statics.sort_by(|a, b| a.uri.cmp(&b.uri));
    if let Some(pair) = statics
        .windows(2)
        .find(|w| matches!(w, [a, b] if a.uri == b.uri))
        && let Some(first) = pair.first()
    {
        return Err(BuildError::DuplicateResource(first.uri.clone()));
    }
    listed.sort_by(|a, b| a.uri.cmp(&b.uri));
    templates.sort_by(|(na, a), (nb, b)| {
        na.cmp(nb)
            .then_with(|| a.template.as_str().cmp(b.template.as_str()))
    });
    for (i, (_, t)) in templates.iter().enumerate() {
        if templates
            .iter()
            .skip(i + 1)
            .any(|(_, other)| other.template.as_str() == t.template.as_str())
        {
            return Err(BuildError::DuplicateResource(
                t.template.as_str().to_owned(),
            ));
        }
    }
    listed_templates.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.uri_template.cmp(&b.uri_template))
    });
    Ok(BuiltResources {
        statics: statics.into(),
        templates: templates.into_iter().map(|(_, t)| t).collect(),
        listed,
        listed_templates,
    })
}

fn build_prompts(mut defs: Vec<PromptDef>, state: &TypeMap) -> Result<BuiltPrompts, BuildError> {
    defs.sort_by(|a, b| a.prompt.name.cmp(&b.prompt.name));
    if let Some(pair) = defs
        .windows(2)
        .find(|w| matches!(w, [a, b] if a.prompt.name == b.prompt.name))
        && let Some(first) = pair.first()
    {
        return Err(BuildError::DuplicatePrompt(first.prompt.name.clone()));
    }
    let mut registry = Vec::with_capacity(defs.len());
    let mut listed = Vec::with_capacity(defs.len());
    for def in defs {
        let name = &def.prompt.name;
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(BuildError::InvalidPromptName(name.clone()));
        }
        check_state(state, &def.required_state, name)?;
        check_scopes(&def.scopes, name)?;
        let arguments: Vec<&str> = def
            .prompt
            .arguments
            .iter()
            .flatten()
            .map(|a| a.name.as_str())
            .collect();
        check_completers(&def.completers, name, &arguments)?;
        registry.push(RegisteredPrompt {
            name: name.clone(),
            description: def.prompt.description.clone(),
            get: def.get,
            completers: def.completers,
            scopes: def.scopes.into(),
        });
        listed.push(def.prompt);
    }
    Ok(BuiltPrompts {
        registry: registry.into(),
        listed,
    })
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field(
                "tools",
                &self.inner.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .field(
                "resources",
                &self
                    .inner
                    .resources
                    .iter()
                    .map(|r| &r.uri)
                    .collect::<Vec<_>>(),
            )
            .field(
                "prompts",
                &self
                    .inner
                    .prompts
                    .iter()
                    .map(|p| &p.name)
                    .collect::<Vec<_>>(),
            )
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

/// A parsed message plus transport-provided context.
#[derive(Debug)]
pub struct McpRequest {
    /// The message.
    pub message: Message,
    /// Values attached by the transport and middleware.
    pub transport: TransportContext,
    /// Cancelled when the client cancels the request.
    pub cancellation: CancellationToken,
}

impl McpRequest {
    /// Wraps a message with an empty transport context.
    #[must_use]
    pub fn new(message: Message) -> Self {
        Self {
            message,
            transport: TransportContext::default(),
            cancellation: CancellationToken::new(),
        }
    }
}

impl Server {
    /// Starts building a server.
    pub fn builder(name: impl Into<String>, version: impl Into<String>) -> ServerBuilder {
        ServerBuilder::new(name, version)
    }

    /// The configured limits.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    /// Parameters of `tool` mirrored into `Mcp-Param-*` HTTP headers, or
    /// `None` if the tool does not exist.
    #[must_use]
    pub fn mirrored_params(&self, tool: &str) -> Option<&[MirroredParam]> {
        self.find_tool(tool).map(|t| t.mirrored.as_slice())
    }

    fn find_tool(&self, name: &str) -> Option<&RegisteredTool> {
        self.inner
            .tools
            .binary_search_by(|t| t.name.as_str().cmp(name))
            .ok()
            .and_then(|i| self.inner.tools.get(i))
    }

    /// Whether any handler declares OAuth scopes.
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    pub(crate) fn has_scopes(&self) -> bool {
        self.inner.has_scopes
    }

    pub(crate) fn legacy(&self) -> &LegacyConfig {
        &self.inner.legacy
    }

    pub(crate) fn state_codec(&self) -> &StateCodec {
        &self.inner.state_codec
    }

    /// The protocol versions this server accepts.
    #[must_use]
    pub fn supported_versions(&self) -> &[String] {
        &self.inner.supported_versions
    }

    /// Parses raw bytes and handles the message. Returns the response to
    /// send, or `None` for notifications, cancelled requests and stray
    /// responses.
    pub async fn handle_bytes(
        &self,
        bytes: &[u8],
        transport: TransportContext,
        cancellation: CancellationToken,
    ) -> Option<Response> {
        match jsonrpc::parse(bytes, &self.inner.limits) {
            Ok(message) => {
                self.handle(McpRequest {
                    message,
                    transport,
                    cancellation,
                })
                .await
            }
            Err(Rejection::Error { id, error }) => Some(Response::error(id, error)),
            Err(Rejection::UnexpectedResponse) => {
                tracing::debug!("ignoring a JSON-RPC response sent by the client");
                None
            }
        }
    }

    /// Handles a parsed message.
    pub async fn handle(&self, request: McpRequest) -> Option<Response> {
        let McpRequest {
            message,
            transport,
            cancellation,
        } = request;
        match message {
            // Client notifications have no effect on a stateless server;
            // cancellation is handled by the transports.
            Message::Notification(_) => None,
            Message::Request(req) => {
                let id = req.id.clone();
                // Handler futures are already isolated; this also covers
                // synchronous user code (argument parsers, completion
                // providers) and any bug in the dispatch path.
                let dispatched =
                    std::panic::AssertUnwindSafe(self.dispatch(req, transport, cancellation))
                        .catch_unwind()
                        .await;
                match dispatched {
                    Ok(Ok(Some(result))) => Some(Response::Result { id, result }),
                    Ok(Ok(None)) => None,
                    Ok(Err(error)) => Some(Response::error(Some(id), error)),
                    Err(_panic) => {
                        tracing::error!("request handling panicked");
                        Some(Response::error(Some(id), McpError::internal()))
                    }
                }
            }
        }
    }

    async fn dispatch(
        &self,
        req: Request,
        transport: TransportContext,
        cancellation: CancellationToken,
    ) -> Result<Option<Box<RawValue>>, McpError> {
        let params = req.params.as_deref();
        match req.method.as_str() {
            methods::SERVER_DISCOVER => {
                let p: MetaOnly<'_> = parse_params(params)?;
                self.validate_meta(p.meta, &transport)?;
                Ok(Some(self.inner.discover.clone()))
            }
            methods::TOOLS_LIST => self.list(
                &req.method,
                params,
                Some(&*self.inner.tools_list),
                &transport,
            ),
            methods::TOOLS_CALL => {
                let p: CallParams<'_> = parse_params(params)?;
                let meta = self.validate_meta(p.meta, &transport)?;
                let auth = transport.get::<AuthContext>();
                let key = RoundKey {
                    method: methods::TOOLS_CALL,
                    target: &p.name,
                    arguments: p.arguments.map_or(Args::None, |a| Args::Raw(a.get())),
                    principal: auth.as_deref().map(|a| a.digest.as_str()),
                };
                let round = self.round(&key, p.input_responses, p.request_state.as_deref())?;
                let ctx = self.ctx(req.id, meta, &cancellation, transport, &round);
                self.call_tool(&p.name, p.arguments, ctx, &cancellation)
                    .await
                    .or_else(|e| self.input_required(e, &round, &key))
            }
            methods::RESOURCES_LIST => self.list(
                &req.method,
                params,
                self.inner.resources_list.as_deref(),
                &transport,
            ),
            methods::RESOURCES_TEMPLATES_LIST => self.list(
                &req.method,
                params,
                self.inner.templates_list.as_deref(),
                &transport,
            ),
            methods::RESOURCES_READ if self.inner.resources_list.is_some() => {
                let p: ReadParams<'_> = parse_params(params)?;
                let meta = self.validate_meta(p.meta, &transport)?;
                let auth = transport.get::<AuthContext>();
                let key = RoundKey {
                    method: methods::RESOURCES_READ,
                    target: &p.uri,
                    arguments: Args::None,
                    principal: auth.as_deref().map(|a| a.digest.as_str()),
                };
                let round = self.round(&key, p.input_responses, p.request_state.as_deref())?;
                let ctx = self.ctx(req.id, meta, &cancellation, transport, &round);
                self.read_resource(&p.uri, ctx, &cancellation)
                    .await
                    .or_else(|e| self.input_required(e, &round, &key))
            }
            methods::PROMPTS_LIST => self.list(
                &req.method,
                params,
                self.inner.prompts_list.as_deref(),
                &transport,
            ),
            methods::PROMPTS_GET if self.inner.prompts_list.is_some() => {
                let p: GetPromptParams<'_> = parse_params(params)?;
                let meta = self.validate_meta(p.meta, &transport)?;
                let arguments = PromptArguments::new(p.arguments.unwrap_or_default());
                let auth = transport.get::<AuthContext>();
                let key = RoundKey {
                    method: methods::PROMPTS_GET,
                    target: &p.name,
                    arguments: Args::Map(arguments.as_map()),
                    principal: auth.as_deref().map(|a| a.digest.as_str()),
                };
                let round = self.round(&key, p.input_responses, p.request_state.as_deref())?;
                let ctx = self.ctx(req.id, meta, &cancellation, transport, &round);
                self.get_prompt(&p.name, &arguments, ctx, &cancellation)
                    .await
                    .or_else(|e| self.input_required(e, &round, &key))
            }
            methods::COMPLETION_COMPLETE if self.inner.has_completions => {
                let mut p: CompleteParams<'_> = parse_params(params)?;
                self.validate_meta(p.meta.take(), &transport)?;
                self.complete(p).map(Some)
            }
            "initialize" if self.inner.legacy.enabled => self
                .legacy_initialize(params)
                .map(|(result, _)| Some(result)),
            "initialize" => {
                hints::warn_once(
                    &self.inner.legacy_hint,
                    format_args!(
                        "a client tried to connect with `initialize` (MCP 2025-11-25 or older), \
                         but this server only accepts {}",
                        self.inner.supported_versions.join(", ")
                    ),
                    "legacy clients are disabled on this server; enable them with \
                     ServerBuilder::legacy_clients(true), or use a client that supports MCP \
                     2026-07-28",
                );
                Err(McpError::method_not_found_explained(format!(
                    "This server implements the stateless MCP protocol (versions: {}); \
                     `initialize` is not supported",
                    self.inner.supported_versions.join(", ")
                )))
            }
            other => Err(McpError::method_not_found(other)),
        }
    }

    /// Answers a list request from its pre-serialized body. `None` means the
    /// server does not offer this kind of item.
    fn list(
        &self,
        method: &str,
        params: Option<&RawValue>,
        body: Option<&RawValue>,
        transport: &TransportContext,
    ) -> Result<Option<Box<RawValue>>, McpError> {
        let Some(body) = body else {
            return Err(McpError::method_not_found(method));
        };
        let p: ListParams<'_> = parse_params(params)?;
        self.validate_meta(p.meta, transport)?;
        if p.cursor.is_some() {
            // Every list fits in one page, so no cursor is valid.
            return Err(McpError::invalid_params("Invalid cursor"));
        }
        Ok(Some(body.to_owned()))
    }

    #[allow(deprecated)] // `log_level`: deprecated (SEP-2577) but still specified.
    fn validate_meta(
        &self,
        meta: Option<&RawValue>,
        transport: &TransportContext,
    ) -> Result<RequestMeta, McpError> {
        // Legacy clients declared everything at `initialize`.
        if let Some(session) = transport.get::<LegacySession>() {
            let mut request_meta = session.request_meta();
            // Legacy `_meta` may carry a progress token.
            request_meta.progress_token = meta
                .and_then(|raw| serde_json::from_str::<LegacyMeta>(raw.get()).ok())
                .and_then(|m| m.progress_token);
            return Ok(request_meta);
        }
        let missing = || {
            McpError::invalid_params(
                "Missing `_meta` with `io.modelcontextprotocol/protocolVersion` and \
                 `io.modelcontextprotocol/clientCapabilities`",
            )
        };
        let meta: spec::RequestMetaObject = serde_json::from_str(meta.ok_or_else(missing)?.get())
            .map_err(|e| {
            McpError::invalid_params(format!(
                "Invalid `_meta`: {}",
                crate::error::sanitize(&e.to_string())
            ))
        })?;
        if !self
            .inner
            .supported_versions
            .contains(&meta.protocol_version)
        {
            return Err(McpError::unsupported_protocol_version(
                &meta.protocol_version,
                &self.inner.supported_versions,
            ));
        }
        Ok(RequestMeta {
            protocol_version: meta.protocol_version,
            client_capabilities: meta.client_capabilities,
            client_info: meta.client_info,
            progress_token: meta.progress_token,
            log_level: meta.log_level,
        })
    }

    fn ctx(
        &self,
        id: RequestId,
        meta: RequestMeta,
        cancellation: &CancellationToken,
        transport: TransportContext,
        round: &Arc<Round>,
    ) -> Ctx {
        // Only when the client asked for notifications and the transport
        // can deliver them.
        let notifier = transport
            .get::<NotifyChannel>()
            .and_then(|channel| {
                Notifier::new(&channel, meta.progress_token.clone(), meta.log_level)
            })
            .map(Arc::new);
        Ctx::new(
            id,
            meta,
            cancellation.clone(),
            transport,
            Arc::clone(&self.inner.state),
            Some(Arc::clone(round)),
            notifier,
        )
    }

    /// Starts an MRTR round: verifies `requestState` and collects answers.
    fn round(
        &self,
        key: &RoundKey<'_>,
        input_responses: Option<Value>,
        request_state: Option<&str>,
    ) -> Result<Arc<Round>, McpError> {
        let has_state = request_state.is_some();
        Round::start(&self.inner.state_codec, key, input_responses, request_state)
            .map(Arc::new)
            .inspect_err(|_| {
                if has_state {
                    hints::warn_once(
                        &self.inner.state_hint,
                        format_args!("rejected a requestState that failed verification"),
                        "if this server runs as several instances, give all of them the same \
                         key with ServerBuilder::request_state_key(..); otherwise the client \
                         sent an expired or modified state (or retried another request)",
                    );
                }
            })
    }

    /// Turns the "input required" signal into an `InputRequiredResult`;
    /// other errors pass through.
    fn input_required(
        &self,
        error: HandlerError,
        round: &Round,
        key: &RoundKey<'_>,
    ) -> Result<Option<Box<RawValue>>, McpError> {
        match error {
            HandlerError::InputRequired => {
                let result = round.input_required(
                    &self.inner.state_codec,
                    key,
                    self.inner.result_meta.as_ref(),
                )?;
                to_raw(&result).map(Some).map_err(|_| McpError::internal())
            }
            HandlerError::Rpc(error) => Err(error),
        }
    }

    /// Runs a handler future under the concurrency limit, the timeout and
    /// panic isolation. `Ok(None)` means the request was cancelled.
    async fn run<T>(
        &self,
        future: Pin<Box<dyn Future<Output = Result<T, ToolError>> + Send>>,
        cancellation: &CancellationToken,
    ) -> Result<Option<Result<T, ToolError>>, McpError> {
        // Bound concurrency; the semaphore is never closed.
        let Ok(_permit) = self.inner.permits.acquire().await else {
            return Err(McpError::internal());
        };
        let guarded = std::panic::AssertUnwindSafe(future).catch_unwind();
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Ok(None),
            timed = tokio::time::timeout(self.inner.limits.tool_timeout, guarded) => {
                Ok(Some(match timed {
                    Ok(Ok(result)) => result,
                    Ok(Err(_panic)) => Err(ToolError::panicked()),
                    Err(_elapsed) => Err(ToolError::timeout()),
                }))
            }
        }
    }

    fn with_server_info(&self, meta: &mut Option<spec::ResultMetaObject>) {
        if let Some(server_meta) = &self.inner.result_meta {
            match meta {
                Some(meta) => meta.server_info.clone_from(&server_meta.server_info),
                None => *meta = Some(server_meta.clone()),
            }
        }
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<&RawValue>,
        ctx: Ctx,
        cancellation: &CancellationToken,
    ) -> Result<Option<Box<RawValue>>, HandlerError> {
        let tool = self
            .find_tool(name)
            .ok_or_else(|| McpError::unknown_tool(name))?;
        authorize(&tool.scopes, &ctx)?;

        let notifier = ctx.notifier_handle();
        let outcome = match (tool.call)(arguments, ctx) {
            Err(error) => Err(error),
            Ok(future) => match self.run(future, cancellation).await? {
                None => return Ok(None),
                Some(outcome) => outcome,
            },
        };
        if let Some(notifier) = notifier {
            notifier.finish().await;
        }

        let mut result = match outcome {
            Ok(ToolOutput::Complete(result)) => result,
            Err(error) if error.is_input_required() => return Err(HandlerError::InputRequired),
            Err(error) => match error.into_protocol() {
                Ok(protocol_error) => return Err(protocol_error.into()),
                Err(error) => error.into_call_result(&tool.name),
            },
        };
        result.result_type = COMPLETE.to_owned();
        self.with_server_info(&mut result.meta);
        Ok(to_raw(&result).map(Some).map_err(|error| {
            tracing::error!(tool = %tool.name, %error, "failed to serialize tool result");
            McpError::internal()
        })?)
    }

    /// Finds the handler for a URI: an exact static resource first, then the
    /// first matching template.
    #[allow(clippy::type_complexity)]
    fn find_resource(
        &self,
        uri: &str,
    ) -> Option<(ReadFn, UriVars, u64, CacheScope, &[String], Option<&Value>)> {
        if let Some(r) = self
            .inner
            .resources
            .binary_search_by(|r| r.uri.as_str().cmp(uri))
            .ok()
            .and_then(|i| self.inner.resources.get(i))
        {
            return Some((
                r.read,
                UriVars::default(),
                r.ttl_ms,
                r.cache_scope,
                &r.scopes,
                r.app.as_ref(),
            ));
        }
        self.inner.templates.iter().find_map(|t| {
            t.template
                .matches(uri)
                .map(|vars| (t.read, vars, t.ttl_ms, t.cache_scope, &*t.scopes, None))
        })
    }

    async fn read_resource(
        &self,
        uri: &str,
        ctx: Ctx,
        cancellation: &CancellationToken,
    ) -> Result<Option<Box<RawValue>>, HandlerError> {
        let (read, vars, ttl_ms, cache_scope, scopes, app) = self
            .find_resource(uri)
            .ok_or_else(|| McpError::resource_not_found(uri))?;
        authorize(scopes, &ctx)?;
        let notifier = ctx.notifier_handle();
        let outcome = match read(uri, &vars, ctx) {
            Err(error) => Err(error),
            Ok(future) => match self.run(future, cancellation).await? {
                None => return Ok(None),
                Some(outcome) => outcome,
            },
        };
        if let Some(notifier) = notifier {
            notifier.finish().await;
        }
        let mut contents =
            outcome.map_err(|e| HandlerError::from_tool(e, "resource", Some(uri)))?;
        if let Some(ui) = app {
            crate::apps::mark_contents(&mut contents, ui);
        }
        let mut result =
            spec::ReadResourceResult::new(cache_scope, contents, COMPLETE.to_owned(), ttl_ms);
        self.with_server_info(&mut result.meta);
        Ok(to_raw(&result).map(Some).map_err(|error| {
            tracing::error!(uri, %error, "failed to serialize resource contents");
            McpError::internal()
        })?)
    }

    fn find_prompt(&self, name: &str) -> Option<&RegisteredPrompt> {
        self.inner
            .prompts
            .binary_search_by(|p| p.name.as_str().cmp(name))
            .ok()
            .and_then(|i| self.inner.prompts.get(i))
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: &PromptArguments,
        ctx: Ctx,
        cancellation: &CancellationToken,
    ) -> Result<Option<Box<RawValue>>, HandlerError> {
        let prompt = self
            .find_prompt(name)
            .ok_or_else(|| McpError::unknown_prompt(name))?;
        authorize(&prompt.scopes, &ctx)?;
        let notifier = ctx.notifier_handle();
        let outcome = match (prompt.get)(arguments, ctx) {
            Err(error) => Err(error),
            Ok(future) => match self.run(future, cancellation).await? {
                None => return Ok(None),
                Some(outcome) => outcome,
            },
        };
        if let Some(notifier) = notifier {
            notifier.finish().await;
        }
        let mut result = outcome.map_err(|e| HandlerError::from_tool(e, &prompt.name, None))?;
        result.result_type = COMPLETE.to_owned();
        if result.description.is_none() {
            result.description.clone_from(&prompt.description);
        }
        self.with_server_info(&mut result.meta);
        Ok(to_raw(&result).map(Some).map_err(|error| {
            tracing::error!(prompt = %prompt.name, %error, "failed to serialize prompt");
            McpError::internal()
        })?)
    }

    fn complete(&self, p: CompleteParams<'_>) -> Result<Box<RawValue>, McpError> {
        let completers = match &p.reference {
            CompletionRef::Prompt { name } => {
                &self
                    .find_prompt(name)
                    .ok_or_else(|| McpError::unknown_prompt(name))?
                    .completers
            }
            CompletionRef::Resource { uri } => {
                &self
                    .inner
                    .templates
                    .iter()
                    .find(|t| t.template.as_str() == uri)
                    .ok_or_else(|| McpError::resource_not_found(uri))?
                    .completers
            }
        };
        let context = p.context.and_then(|c| c.arguments).unwrap_or_default();
        let request = CompletionRequest {
            argument: &p.argument.name,
            value: &p.argument.value,
            context: &context,
        };
        let completion = match completers.iter().find(|c| c.argument == p.argument.name) {
            Some(completer) => {
                let complete = completer.complete;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| complete(&request)))
                    .map_err(|_panic| {
                        tracing::error!(argument = %p.argument.name, "completion provider panicked");
                        McpError::internal()
                    })?
            }
            None => crate::completion::Completion::default(),
        };
        let mut result = spec::CompleteResult::new(completion.into_spec(), COMPLETE.to_owned());
        self.with_server_info(&mut result.meta);
        to_raw(&result).map_err(|error| {
            tracing::error!(%error, "failed to serialize completion");
            McpError::internal()
        })
    }
}

impl tower_service::Service<McpRequest> for Server {
    type Response = Option<Response>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: McpRequest) -> Self::Future {
        let server = self.clone();
        Box::pin(async move { Ok(server.handle(request).await) })
    }
}

/// The legacy `_meta` fields the adapter uses.
#[derive(Deserialize)]
struct LegacyMeta {
    #[serde(rename = "progressToken", default)]
    progress_token: Option<spec::ProgressToken>,
}

#[derive(Deserialize)]
struct MetaOnly<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct ListParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
    #[serde(borrow, default)]
    cursor: Option<std::borrow::Cow<'a, str>>,
}

#[derive(Deserialize)]
struct CallParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
    name: String,
    #[serde(borrow, default)]
    arguments: Option<&'a RawValue>,
    #[serde(rename = "inputResponses", default)]
    input_responses: Option<Value>,
    #[serde(rename = "requestState", default)]
    request_state: Option<String>,
}

#[derive(Deserialize)]
struct ReadParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
    #[serde(borrow)]
    uri: std::borrow::Cow<'a, str>,
    #[serde(rename = "inputResponses", default)]
    input_responses: Option<Value>,
    #[serde(rename = "requestState", default)]
    request_state: Option<String>,
}

#[derive(Deserialize)]
struct GetPromptParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
    name: String,
    #[serde(default)]
    arguments: Option<BTreeMap<String, String>>,
    #[serde(rename = "inputResponses", default)]
    input_responses: Option<Value>,
    #[serde(rename = "requestState", default)]
    request_state: Option<String>,
}

/// The failure of a handler that supports multi round-trip requests.
enum HandlerError {
    /// The handler asked the client for input.
    InputRequired,
    /// A JSON-RPC error.
    Rpc(McpError),
}

impl HandlerError {
    fn from_tool(error: ToolError, handler: &str, uri: Option<&str>) -> Self {
        if error.is_input_required() {
            Self::InputRequired
        } else {
            Self::Rpc(error.into_rpc_error(handler, uri))
        }
    }
}

impl From<McpError> for HandlerError {
    fn from(error: McpError) -> Self {
        Self::Rpc(error)
    }
}

#[derive(Deserialize)]
struct CompleteParams<'a> {
    #[serde(rename = "_meta", borrow, default)]
    meta: Option<&'a RawValue>,
    #[serde(rename = "ref")]
    reference: CompletionRef,
    argument: CompletionArgument,
    #[serde(default)]
    context: Option<CompletionContext>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum CompletionRef {
    #[serde(rename = "ref/prompt")]
    Prompt { name: String },
    #[serde(rename = "ref/resource")]
    Resource { uri: String },
}

#[derive(Deserialize)]
struct CompletionArgument {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct CompletionContext {
    #[serde(default)]
    arguments: Option<BTreeMap<String, String>>,
}

fn parse_params<'a, T: Deserialize<'a>>(params: Option<&'a RawValue>) -> Result<T, McpError> {
    let raw = params.map_or("{}", RawValue::get);
    serde_json::from_str(raw).map_err(|e| {
        McpError::invalid_params(format!(
            "Invalid params: {}",
            crate::error::sanitize(&e.to_string())
        ))
    })
}

fn to_raw<T: serde::Serialize>(value: &T) -> Result<Box<RawValue>, serde_json::Error> {
    serde_json::value::to_raw_value(value)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Tool names: 1 to 128 characters from `A-Z a-z 0-9 _ - .` (spec SHOULD,
/// enforced here so names are always safe as HTTP header values).
fn is_valid_tool_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Checks a handler's scopes against the request's access token. Without
/// OAuth (stdio, or HTTP without [`OAuth`](crate::transport::http::OAuth))
/// there is no token and nothing to check.
fn authorize(scopes: &[String], ctx: &Ctx) -> Result<(), McpError> {
    if scopes.is_empty() {
        return Ok(());
    }
    match ctx.transport().get::<AuthContext>() {
        Some(auth) => auth.check(scopes),
        None => Ok(()),
    }
}
