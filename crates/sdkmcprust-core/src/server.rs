//! The MCP server: builder, immutable registry and request dispatch.

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

use crate::context::{Ctx, RequestMeta, TransportContext, TypeMap};
use crate::error::{McpError, truncate};
use crate::extension::{self, Extension};
use crate::jsonrpc::{self, Message, Rejection, Request, Response};
use crate::limits::Limits;
use crate::spec::{self, CacheScope, RequestId, methods};
use crate::tool::{CallFn, Tool, ToolDef, ToolOutput};

/// The `resultType` of final results.
const COMPLETE: &str = "complete";

/// Errors detected while building a server.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// Two tools share a name.
    #[error("duplicate tool name `{0}`")]
    DuplicateTool(String),
    /// A tool name breaks the specification's naming rules.
    #[error("invalid tool name `{0}`: use 1-128 characters from A-Z, a-z, 0-9, `_`, `-` and `.`")]
    InvalidToolName(String),
    /// A tool needs shared state that was not registered.
    #[error("tool `{tool}` needs state of type `{type_name}`; register it with `.state(..)`")]
    MissingState {
        /// The tool name.
        tool: String,
        /// The missing type.
        type_name: &'static str,
    },
    /// An extension identifier is malformed or registered twice.
    #[error("invalid or duplicate extension identifier `{0}`")]
    InvalidExtension(String),
    /// A limit is out of range.
    #[error("invalid limit: {0}")]
    InvalidLimit(&'static str),
    /// A pre-serialized response could not be built.
    #[error("could not serialize {0}")]
    Serialize(&'static str, #[source] serde_json::Error),
}

/// Builds a [`Server`].
#[must_use = "call `.build()` to create the server"]
pub struct ServerBuilder {
    info: spec::Implementation,
    instructions: Option<String>,
    tools: Vec<ToolDef>,
    state: TypeMap,
    extensions: Vec<Box<dyn Extension>>,
    limits: Limits,
    list_ttl: Duration,
    discover_ttl: Duration,
    cache_scope: CacheScope,
    include_server_info: bool,
}

impl std::fmt::Debug for ServerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerBuilder")
            .field("info", &self.info)
            .field("tools", &self.tools.len())
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
            state: TypeMap::default(),
            extensions: Vec::new(),
            limits: Limits::default(),
            list_ttl: Duration::from_secs(300),
            discover_ttl: Duration::from_secs(3600),
            cache_scope: CacheScope::Public,
            include_server_info: true,
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

    /// Registers a tool definition built by hand.
    pub fn tool_def(mut self, def: ToolDef) -> Self {
        self.tools.push(def);
        self
    }

    /// Registers shared state, available to tools through `State<T>`.
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

    /// Maximum duration of one tool call. Default: 30 s.
    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.limits.tool_timeout = timeout;
        self
    }

    /// Maximum number of concurrent tool calls. Default: 64.
    pub fn max_concurrent_calls(mut self, calls: usize) -> Self {
        self.limits.max_concurrent_calls = calls;
        self
    }

    /// Freshness hint (`ttlMs`) for `tools/list`. Default: 5 minutes.
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
    /// Returns a [`BuildError`] for duplicate or invalid tool names, missing
    /// state, invalid extensions, out-of-range limits, or serialization
    /// failures of the pre-built responses.
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
        for def in &tools {
            if !is_valid_tool_name(&def.tool.name) {
                return Err(BuildError::InvalidToolName(def.tool.name.clone()));
            }
            for req in &def.required_state {
                if !self.state.contains(req.type_id) {
                    return Err(BuildError::MissingState {
                        tool: def.tool.name.clone(),
                        type_name: req.type_name,
                    });
                }
            }
        }

        let mut extensions = Map::new();
        for ext in &self.extensions {
            let id = ext.id();
            if !extension::is_valid_id(id) || extensions.contains_key(id) {
                return Err(BuildError::InvalidExtension(id.to_owned()));
            }
            extensions.insert(id.to_owned(), Value::Object(ext.settings()));
        }

        let result_meta = self.include_server_info.then(|| spec::ResultMetaObject {
            server_info: Some(self.info.clone()),
            extra: Map::new(),
        });

        let capabilities = spec::ServerCapabilities {
            tools: Some(spec::ServerCapabilitiesTools { list_changed: None }),
            extensions: (!extensions.is_empty()).then(|| {
                extensions
                    .into_iter()
                    .filter_map(|(k, v)| match v {
                        Value::Object(o) => Some((k, o)),
                        _ => None,
                    })
                    .collect()
            }),
            ..spec::ServerCapabilities::default()
        };

        let supported_versions = vec![spec::PROTOCOL_VERSION.to_owned()];

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

        let registry = tools
            .into_iter()
            .map(|t| RegisteredTool {
                name: t.tool.name,
                call: t.call,
            })
            .collect();

        Ok(Server {
            inner: Arc::new(Inner {
                tools: registry,
                discover,
                tools_list,
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
    discover: Box<RawValue>,
    tools_list: Box<RawValue>,
    result_meta: Option<spec::ResultMetaObject>,
    supported_versions: Vec<String>,
    state: Arc<TypeMap>,
    permits: Arc<Semaphore>,
    limits: Limits,
}

struct RegisteredTool {
    name: String,
    call: CallFn,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field(
                "tools",
                &self.inner.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
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
                match self.dispatch(req, transport, cancellation).await {
                    Ok(Some(result)) => Some(Response::Result { id, result }),
                    Ok(None) => None,
                    Err(error) => Some(Response::error(Some(id), error)),
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
                let p: MetaOnly = parse_params(params)?;
                self.validate_meta(p.meta)?;
                Ok(Some(self.inner.discover.clone()))
            }
            methods::TOOLS_LIST => {
                let p: ListParams<'_> = parse_params(params)?;
                self.validate_meta(p.meta)?;
                if p.cursor.is_some() {
                    // The whole list fits in one page, so no cursor is valid.
                    return Err(McpError::invalid_params("Invalid cursor"));
                }
                Ok(Some(self.inner.tools_list.clone()))
            }
            methods::TOOLS_CALL => {
                let p: CallParams<'_> = parse_params(params)?;
                let meta = self.validate_meta(p.meta)?;
                self.call_tool(req.id, &p.name, p.arguments, meta, transport, cancellation)
                    .await
            }
            "initialize" => Err(McpError::method_not_found_explained(format!(
                "This server implements the stateless MCP protocol (versions: {}); \
                     `initialize` is not supported",
                self.inner.supported_versions.join(", ")
            ))),
            other => Err(McpError::method_not_found(other)),
        }
    }

    fn validate_meta(
        &self,
        meta: Option<spec::RequestMetaObject>,
    ) -> Result<RequestMeta, McpError> {
        let meta = meta.ok_or_else(|| {
            McpError::invalid_params(
                "Missing `_meta` with `io.modelcontextprotocol/protocolVersion` and \
                 `io.modelcontextprotocol/clientCapabilities`",
            )
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
        })
    }

    async fn call_tool(
        &self,
        id: RequestId,
        name: &str,
        arguments: Option<&RawValue>,
        meta: RequestMeta,
        transport: TransportContext,
        cancellation: CancellationToken,
    ) -> Result<Option<Box<RawValue>>, McpError> {
        let tool = self
            .inner
            .tools
            .binary_search_by(|t| t.name.as_str().cmp(name))
            .ok()
            .and_then(|i| self.inner.tools.get(i))
            .ok_or_else(|| McpError::invalid_params(format!("Unknown tool: {}", truncate(name))))?;

        let ctx = Ctx::new(
            id,
            meta,
            cancellation.clone(),
            transport,
            Arc::clone(&self.inner.state),
        );

        let outcome = match (tool.call)(arguments, ctx) {
            Err(error) => Err(error),
            Ok(future) => {
                // Bound concurrency; the semaphore is never closed.
                let Ok(_permit) = self.inner.permits.acquire().await else {
                    return Err(McpError::internal());
                };
                let guarded = std::panic::AssertUnwindSafe(future).catch_unwind();
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return Ok(None),
                    timed = tokio::time::timeout(self.inner.limits.tool_timeout, guarded) => {
                        match timed {
                            Ok(Ok(result)) => result,
                            Ok(Err(_panic)) => Err(crate::tool::ToolError::panicked()),
                            Err(_elapsed) => Err(crate::tool::ToolError::timeout()),
                        }
                    }
                }
            }
        };

        let mut result = match outcome {
            Ok(ToolOutput::Complete(result)) => result,
            Err(error) => error.into_call_result(&tool.name),
        };
        result.result_type = COMPLETE.to_owned();
        if let Some(server_meta) = &self.inner.result_meta {
            match &mut result.meta {
                Some(meta) => meta.server_info.clone_from(&server_meta.server_info),
                None => result.meta = Some(server_meta.clone()),
            }
        }
        to_raw(&result).map(Some).map_err(|error| {
            tracing::error!(tool = %tool.name, %error, "failed to serialize tool result");
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

#[derive(Deserialize)]
struct MetaOnly {
    #[serde(rename = "_meta", default)]
    meta: Option<spec::RequestMetaObject>,
}

#[derive(Deserialize)]
struct ListParams<'a> {
    #[serde(rename = "_meta", default)]
    meta: Option<spec::RequestMetaObject>,
    #[serde(borrow, default)]
    cursor: Option<std::borrow::Cow<'a, str>>,
}

#[derive(Deserialize)]
struct CallParams<'a> {
    #[serde(rename = "_meta", default)]
    meta: Option<spec::RequestMetaObject>,
    name: String,
    #[serde(borrow, default)]
    arguments: Option<&'a RawValue>,
}

fn parse_params<'a, T: Deserialize<'a>>(params: Option<&'a RawValue>) -> Result<T, McpError> {
    let raw = params.map_or("{}", RawValue::get);
    serde_json::from_str(raw).map_err(|e| McpError::invalid_params(format!("Invalid params: {e}")))
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
