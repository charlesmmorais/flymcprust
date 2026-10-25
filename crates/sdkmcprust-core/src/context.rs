//! Per-request context ([`Ctx`]), shared application state ([`State`]) and
//! transport-provided values ([`TransportContext`]).

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::spec::{self, RequestId};

/// A small type map: at most one value per type.
#[derive(Clone, Default)]
pub(crate) struct TypeMap {
    values: HashMap<TypeId, (Arc<dyn Any + Send + Sync>, &'static str)>,
}

impl TypeMap {
    pub(crate) fn insert<T: Send + Sync + 'static>(&mut self, value: Arc<T>) {
        self.values
            .insert(TypeId::of::<T>(), (value, type_name::<T>()));
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let (value, _) = self.values.get(&TypeId::of::<T>())?;
        Arc::clone(value).downcast::<T>().ok()
    }

    pub(crate) fn contains(&self, id: TypeId) -> bool {
        self.values.contains_key(&id)
    }
}

impl fmt::Debug for TypeMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set()
            .entries(self.values.values().map(|(_, name)| name))
            .finish()
    }
}

/// Values attached by the transport to a request (for example, authenticated
/// claims or HTTP headers). Tower middleware can populate it before the
/// request reaches the server.
#[derive(Clone, Default, Debug)]
pub struct TransportContext {
    values: TypeMap,
}

impl TransportContext {
    /// Creates an empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a value, replacing any previous value of the same type.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.values.insert(Arc::new(value));
    }

    /// Returns the value of type `T`, if present.
    #[must_use]
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.values.get::<T>()
    }
}

/// Protocol metadata carried by a request's `_meta`.
#[derive(Debug, Clone)]
pub(crate) struct RequestMeta {
    pub(crate) protocol_version: String,
    pub(crate) client_capabilities: spec::ClientCapabilities,
    pub(crate) client_info: Option<spec::Implementation>,
    pub(crate) progress_token: Option<spec::ProgressToken>,
}

/// Context of the request being handled. Declare a `Ctx` parameter in a tool
/// to receive it; it is excluded from the tool's input schema.
///
/// Cloning is cheap.
#[derive(Clone)]
pub struct Ctx {
    inner: Arc<CtxInner>,
}

struct CtxInner {
    request_id: RequestId,
    meta: RequestMeta,
    cancellation: CancellationToken,
    transport: TransportContext,
    state: Arc<TypeMap>,
}

impl Ctx {
    pub(crate) fn new(
        request_id: RequestId,
        meta: RequestMeta,
        cancellation: CancellationToken,
        transport: TransportContext,
        state: Arc<TypeMap>,
    ) -> Self {
        Self {
            inner: Arc::new(CtxInner {
                request_id,
                meta,
                cancellation,
                transport,
                state,
            }),
        }
    }

    /// The JSON-RPC id of the request.
    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.inner.request_id
    }

    /// The protocol version the client used for this request.
    #[must_use]
    pub fn protocol_version(&self) -> &str {
        &self.inner.meta.protocol_version
    }

    /// The capabilities the client declared for this request.
    #[must_use]
    pub fn client_capabilities(&self) -> &spec::ClientCapabilities {
        &self.inner.meta.client_capabilities
    }

    /// The client's self-reported identity. Informational only: never use it
    /// for security decisions.
    #[must_use]
    pub fn client_info(&self) -> Option<&spec::Implementation> {
        self.inner.meta.client_info.as_ref()
    }

    /// The progress token, when the client asked for progress notifications.
    #[must_use]
    pub fn progress_token(&self) -> Option<&spec::ProgressToken> {
        self.inner.meta.progress_token.as_ref()
    }

    /// Whether the client declared support for an extension.
    #[must_use]
    pub fn client_supports_extension(&self, id: &str) -> bool {
        self.inner
            .meta
            .client_capabilities
            .extensions
            .as_ref()
            .is_some_and(|e| e.contains_key(id))
    }

    /// A token cancelled when the client cancels the request (stdio
    /// `notifications/cancelled`, or the HTTP response stream closing).
    /// Long-running tools should stop work when it fires.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationToken {
        &self.inner.cancellation
    }

    /// Whether the request has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancellation.is_cancelled()
    }

    /// Values attached by the transport.
    #[must_use]
    pub fn transport(&self) -> &TransportContext {
        &self.inner.transport
    }

    /// Shared state registered with
    /// [`ServerBuilder::state`](crate::ServerBuilder::state).
    #[must_use]
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<State<T>> {
        self.inner.state.get::<T>().map(State)
    }
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("request_id", &self.inner.request_id)
            .field("protocol_version", &self.inner.meta.protocol_version)
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// Shared application state registered with
/// [`ServerBuilder::state`](crate::ServerBuilder::state). Declare a
/// `State<T>` parameter in a tool to receive it; it is excluded from the
/// tool's input schema. A tool that needs a state type that was never
/// registered makes [`ServerBuilder::build`](crate::ServerBuilder::build) fail.
pub struct State<T: ?Sized>(Arc<T>);

impl<T: ?Sized> State<T> {
    /// The shared pointer to the state.
    #[must_use]
    pub fn arc(&self) -> &Arc<T> {
        &self.0
    }

    /// Unwraps into the shared pointer.
    #[must_use]
    pub fn into_arc(self) -> Arc<T> {
        self.0
    }
}

impl<T: ?Sized> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: ?Sized> Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("State").field(&self.0).finish()
    }
}
