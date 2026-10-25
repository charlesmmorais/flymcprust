//! Multi round-trip requests (MRTR): handlers ask the client for input
//! (elicitation, sampling, roots) by returning an `InputRequiredResult`; the
//! client retries the original request with the answers.
//!
//! The SDK runs handlers in *replay* style: every round runs the handler from
//! the start. [`Ctx::elicit`] (and its siblings) returns the answer when the
//! client already sent it, in this round or an earlier one; otherwise it
//! records the request and fails with an internal "input required" error that
//! the runtime turns into the `InputRequiredResult`. Code before an input call
//! therefore runs once per round, so it should be free of side effects (or
//! idempotent).
//!
//! Answers from earlier rounds travel in `requestState`, which the client
//! must echo back. The state is HMAC-SHA256 signed and bound to the method,
//! the target (tool or prompt name, resource URI), a digest of the arguments
//! and an expiry, so a client cannot forge answers, replay them on another
//! request, or use them after the expiry. Answers the client sends in this
//! round's `inputResponses` are client input like the arguments themselves.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit as _, Mac as _};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::context::Ctx;
use crate::error::McpError;
use crate::spec::{self, methods};
use crate::tool::{ToolError, schema_for};

type HmacSha256 = Hmac<Sha256>;

/// Minimum length of a `requestState` signing key.
pub const MIN_REQUEST_STATE_KEY_BYTES: usize = 32;

/// Version of the `requestState` payload format.
const STATE_VERSION: u8 = 1;

/// Domain separation for the signature.
const SIGNATURE_CONTEXT: &[u8] = b"sdkmcprust/requestState/v1\0";

/// Signs and verifies `requestState`.
pub(crate) struct StateCodec {
    key: Vec<u8>,
    ttl: Duration,
}

impl std::fmt::Debug for StateCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateCodec")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

/// The signed content of `requestState`.
#[derive(Serialize, Deserialize)]
struct Payload {
    v: u8,
    /// Method of the original request.
    m: String,
    /// Tool or prompt name, or resource URI.
    t: String,
    /// Digest of the arguments.
    a: String,
    /// Expiry (seconds since the Unix epoch).
    e: u64,
    /// Rounds completed so far.
    n: u32,
    /// Answers collected in earlier rounds.
    r: BTreeMap<String, Value>,
}

impl StateCodec {
    pub(crate) fn new(key: Vec<u8>, ttl: Duration) -> Self {
        Self { key, ttl }
    }

    fn mac(&self, data: &[u8]) -> Result<HmacSha256, McpError> {
        let mut mac = HmacSha256::new_from_slice(&self.key).map_err(|_| McpError::internal())?;
        mac.update(SIGNATURE_CONTEXT);
        mac.update(data);
        Ok(mac)
    }

    fn seal(&self, payload: &Payload) -> Result<String, McpError> {
        let json = serde_json::to_vec(payload).map_err(|_| McpError::internal())?;
        let body = URL_SAFE_NO_PAD.encode(json);
        let tag = self.mac(body.as_bytes())?.finalize().into_bytes();
        Ok(format!("{body}.{}", URL_SAFE_NO_PAD.encode(tag)))
    }

    fn open(&self, token: &str, round: &RoundKey<'_>) -> Result<Payload, McpError> {
        let digest = round.args_digest();
        let invalid = || McpError::invalid_params("Invalid or expired requestState");
        let (body, tag) = token.rsplit_once('.').ok_or_else(invalid)?;
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid())?;
        self.mac(body.as_bytes())?
            .verify_slice(&tag)
            .map_err(|_| invalid())?;
        let json = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
        let payload: Payload = serde_json::from_slice(&json).map_err(|_| invalid())?;
        if payload.v != STATE_VERSION
            || payload.m != round.method
            || payload.t != round.target
            || payload.a != digest
            || payload.e < now_secs()
        {
            return Err(invalid());
        }
        Ok(payload)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What identifies the original request across rounds. Digests are only
/// computed when MRTR is actually used.
pub(crate) struct RoundKey<'a> {
    pub(crate) method: &'static str,
    pub(crate) target: &'a str,
    pub(crate) arguments: Args<'a>,
}

/// The arguments of the original request.
#[derive(Clone, Copy)]
pub(crate) enum Args<'a> {
    None,
    /// Raw JSON (`tools/call`).
    Raw(&'a str),
    /// String arguments (`prompts/get`).
    Map(&'a BTreeMap<String, String>),
}

impl RoundKey<'_> {
    /// Digest of the arguments, insensitive to whitespace and key order.
    fn args_digest(&self) -> String {
        let canonical = match self.arguments {
            Args::None => Vec::new(),
            Args::Raw(raw) => serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|mut v| {
                    v.sort_all_objects();
                    serde_json::to_vec(&v).ok()
                })
                .unwrap_or_default(),
            Args::Map(map) => serde_json::to_vec(map).unwrap_or_default(),
        };
        URL_SAFE_NO_PAD.encode(Sha256::digest(&canonical))
    }
}

/// MRTR data of one round, carried by [`Ctx`].
#[derive(Debug, Default)]
pub(crate) struct Round {
    /// Rounds completed before this one.
    number: u32,
    /// Answers from earlier rounds (from `requestState`) and from this
    /// round's `inputResponses`.
    responses: BTreeMap<String, Value>,
    /// Requests issued in this round that have no answer yet.
    pending: Mutex<Vec<(String, Value)>>,
}

impl Round {
    /// Collects the answers of a request, verifying its `requestState`.
    pub(crate) fn start(
        codec: &StateCodec,
        key: &RoundKey<'_>,
        input_responses: Option<Value>,
        request_state: Option<&str>,
    ) -> Result<Self, McpError> {
        let (number, mut responses) = match request_state {
            Some(token) => {
                let payload = codec.open(token, key)?;
                (payload.n, payload.r)
            }
            None => (0, BTreeMap::new()),
        };
        match input_responses {
            None | Some(Value::Null) => {}
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    responses.entry(k).or_insert(v);
                }
            }
            Some(_) => {
                return Err(McpError::invalid_params(
                    "`inputResponses` must be an object",
                ));
            }
        }
        Ok(Self {
            number,
            responses,
            pending: Mutex::new(Vec::new()),
        })
    }

    /// Builds the `InputRequiredResult` for the requests issued this round.
    pub(crate) fn input_required(
        &self,
        codec: &StateCodec,
        key: &RoundKey<'_>,
        meta: Option<&spec::ResultMetaObject>,
    ) -> Result<Value, McpError> {
        let pending =
            std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner));
        let expiry = now_secs().saturating_add(codec.ttl.as_secs());
        let state = codec.seal(&Payload {
            v: STATE_VERSION,
            m: key.method.to_owned(),
            t: key.target.to_owned(),
            a: key.args_digest(),
            e: expiry,
            n: self.number.saturating_add(1),
            r: self.responses.clone(),
        })?;
        let mut result = Map::new();
        result.insert("resultType".to_owned(), Value::from("input_required"));
        if !pending.is_empty() {
            result.insert(
                "inputRequests".to_owned(),
                Value::Object(pending.into_iter().collect()),
            );
        }
        result.insert("requestState".to_owned(), Value::from(state));
        if let Some(meta) = meta {
            let meta = serde_json::to_value(meta).map_err(|_| McpError::internal())?;
            result.insert("_meta".to_owned(), meta);
        }
        Ok(Value::Object(result))
    }

    fn response(&self, key: &str) -> Option<&Value> {
        self.responses.get(key)
    }

    fn push(&self, key: &str, request: Value) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if !pending.iter().any(|(k, _)| k == key) {
            pending.push((key.to_owned(), request));
        }
    }
}

/// The outcome of an elicitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elicitation<T> {
    /// The user submitted the form.
    Accepted(T),
    /// The user explicitly declined.
    Declined,
    /// The user dismissed the request without choosing.
    Cancelled,
}

#[derive(Deserialize)]
struct ElicitAnswer {
    action: String,
    #[serde(default)]
    content: Option<Value>,
}

/// Which client capability an input request needs.
#[derive(Clone, Copy)]
enum Needs {
    Elicitation,
    Sampling,
    Roots,
}

impl Ctx {
    /// The number of input rounds completed before this attempt: `0` on the
    /// first call, `1` after the client answered once, and so on.
    #[must_use]
    pub fn input_round(&self) -> u32 {
        self.round().map_or(0, |r| r.number)
    }

    /// The client's answer for `key`, decoded as `T`, if it sent one in this
    /// round or an earlier one.
    ///
    /// # Errors
    ///
    /// Returns an invalid-params error (`-32602`) if the answer does not
    /// decode as `T`.
    pub fn input_response<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, ToolError> {
        let Some(value) = self.round().and_then(|r| r.response(key)) else {
            return Ok(None);
        };
        T::deserialize(value)
            .map(Some)
            .map_err(|_| invalid_response(key))
    }

    /// Asks the user for structured input with a form generated from `T`
    /// (a flat struct of strings, numbers, booleans and enums). Declining or
    /// cancelling becomes a [`ToolError::user`]; use
    /// [`elicit_with_action`](Self::elicit_with_action) to handle them.
    ///
    /// `key` identifies the question across rounds and must be unique within
    /// the handler. Several questions can be asked in one round by awaiting
    /// them together (for example with `tokio::join!`).
    ///
    /// # Errors
    ///
    /// Fails with an "input required" error when the answer is not available
    /// yet (propagate it with `?`), `-32021` if the client does not support
    /// form elicitation, and `-32602` if the answer is malformed.
    pub async fn elicit<T: DeserializeOwned + JsonSchema>(
        &self,
        key: &str,
        message: impl Into<String>,
    ) -> Result<T, ToolError> {
        match self.elicit_with_action::<T>(key, message).await? {
            Elicitation::Accepted(value) => Ok(value),
            Elicitation::Declined => Err(ToolError::user(
                "The user declined to provide the requested information.",
            )),
            Elicitation::Cancelled => Err(ToolError::user("The user cancelled the request.")),
        }
    }

    /// Like [`elicit`](Self::elicit), but returns declined and cancelled
    /// answers instead of failing.
    ///
    /// # Errors
    ///
    /// See [`elicit`](Self::elicit).
    pub async fn elicit_with_action<T: DeserializeOwned + JsonSchema>(
        &self,
        key: &str,
        message: impl Into<String>,
    ) -> Result<Elicitation<T>, ToolError> {
        if let Some(answer) = self.input_response::<ElicitAnswer>(key)? {
            return match answer.action.as_str() {
                "accept" => {
                    let content = answer.content.unwrap_or_else(|| json!({}));
                    T::deserialize(&content)
                        .map(Elicitation::Accepted)
                        .map_err(|_| invalid_response(key))
                }
                "decline" => Ok(Elicitation::Declined),
                "cancel" => Ok(Elicitation::Cancelled),
                _ => Err(invalid_response(key)),
            };
        }
        let schema = elicitation_schema::<T>()?;
        let request = json!({
            "method": methods::ELICITATION_CREATE,
            "params": { "message": message.into(), "requestedSchema": schema },
        });
        Err(self.ask(key, request, Needs::Elicitation))
    }

    /// Asks the client to sample from a language model.
    ///
    /// # Errors
    ///
    /// Fails with an "input required" error when the answer is not available
    /// yet (propagate it with `?`), `-32021` if the client does not support
    /// sampling, and `-32602` if the answer is malformed.
    #[deprecated(note = "Sampling is deprecated as of protocol version 2026-07-28 (SEP-2577).")]
    #[allow(deprecated)]
    pub async fn sample(
        &self,
        key: &str,
        params: spec::CreateMessageRequestParams,
    ) -> Result<spec::CreateMessageResult, ToolError> {
        if let Some(answer) = self.input_response(key)? {
            return Ok(answer);
        }
        let request = json!({ "method": methods::SAMPLING_CREATE_MESSAGE, "params": params });
        Err(self.ask(key, request, Needs::Sampling))
    }

    /// Asks the client for its filesystem roots.
    ///
    /// # Errors
    ///
    /// Fails with an "input required" error when the answer is not available
    /// yet (propagate it with `?`), `-32021` if the client does not support
    /// roots, and `-32602` if the answer is malformed.
    #[deprecated(note = "Roots are deprecated as of protocol version 2026-07-28 (SEP-2577).")]
    #[allow(deprecated)]
    pub async fn list_roots(&self, key: &str) -> Result<spec::ListRootsResult, ToolError> {
        if let Some(answer) = self.input_response(key)? {
            return Ok(answer);
        }
        let request = json!({ "method": methods::ROOTS_LIST, "params": {} });
        Err(self.ask(key, request, Needs::Roots))
    }

    /// Records an input request and returns the error that ends this round.
    fn ask(&self, key: &str, request: Value, needs: Needs) -> ToolError {
        let Some(round) = self.round() else {
            return ToolError::internal(std::io::Error::other(
                "input requests are only available in tools/call, resources/read and prompts/get",
            ));
        };
        #[allow(deprecated)]
        let caps = self.client_capabilities();
        #[allow(deprecated)]
        let (supported, required) = match needs {
            Needs::Elicitation => (
                caps.elicitation
                    .as_ref()
                    .is_some_and(|e| e.form.is_some() || e.url.is_none()),
                spec::ClientCapabilities {
                    elicitation: Some(spec::ClientCapabilitiesElicitation {
                        form: Some(Map::new()),
                        url: None,
                    }),
                    ..spec::ClientCapabilities::default()
                },
            ),
            Needs::Sampling => (
                caps.sampling.is_some(),
                spec::ClientCapabilities {
                    sampling: Some(spec::ClientCapabilitiesSampling::default()),
                    ..spec::ClientCapabilities::default()
                },
            ),
            Needs::Roots => (
                caps.roots.is_some(),
                spec::ClientCapabilities {
                    roots: Some(spec::ClientCapabilitiesRoots::default()),
                    ..spec::ClientCapabilities::default()
                },
            ),
        };
        if !supported {
            return ToolError::missing_client_capability(&required);
        }
        round.push(key, request);
        ToolError::input_required()
    }
}

fn invalid_response(key: &str) -> ToolError {
    ToolError::protocol(McpError::invalid_params(format!(
        "Invalid input response for `{}`",
        crate::error::truncate(key)
    )))
}

/// The `requestedSchema` of an elicitation: the JSON Schema of `T`, which
/// must be an object of primitive fields. `Option<X>` fields (`"type":
/// ["x", "null"]`) become plain optional fields.
fn elicitation_schema<T: JsonSchema>() -> Result<Value, ToolError> {
    let mut schema = schema_for::<T>();
    let unsupported = || {
        ToolError::internal(std::io::Error::other(format!(
            "`{}` cannot be elicited: use a struct of string, number, integer, boolean or enum fields",
            std::any::type_name::<T>()
        )))
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(unsupported());
    }
    schema.remove("additionalProperties");
    let Some(Value::Object(properties)) = schema.get_mut("properties") else {
        return Err(unsupported());
    };
    for property in properties.values_mut() {
        let Value::Object(property) = property else {
            return Err(unsupported());
        };
        if let Some(Value::Array(types)) = property.get("type") {
            let non_null: Vec<&Value> = types
                .iter()
                .filter(|t| t.as_str() != Some("null"))
                .collect();
            if let [single] = non_null.as_slice() {
                let single = (*single).clone();
                property.insert("type".to_owned(), single);
            }
        }
        let primitive = matches!(
            property.get("type").and_then(Value::as_str),
            Some("string" | "number" | "integer" | "boolean" | "array")
        ) || property.contains_key("enum")
            || property.contains_key("oneOf")
            || property.contains_key("anyOf");
        if !primitive {
            return Err(unsupported());
        }
    }
    Ok(Value::Object(schema))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> StateCodec {
        StateCodec::new(vec![7; 32], Duration::from_secs(600))
    }

    fn key(target: &str) -> RoundKey<'_> {
        RoundKey {
            method: "tools/call",
            target,
            arguments: Args::Raw(r#"{"a": 1}"#),
        }
    }

    fn round_trip(codec: &StateCodec, round: &Round) -> String {
        round.input_required(codec, &key("t"), None).unwrap()["requestState"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn state_round_trips_and_accumulates_answers() {
        let codec = codec();
        let first = Round::start(&codec, &key("t"), Some(json!({ "q1": 1 })), None).unwrap();
        let token = round_trip(&codec, &first);
        let second =
            Round::start(&codec, &key("t"), Some(json!({ "q2": 2 })), Some(&token)).unwrap();
        assert_eq!(second.number, 1);
        assert_eq!(second.response("q1"), Some(&json!(1)));
        assert_eq!(second.response("q2"), Some(&json!(2)));
    }

    #[test]
    fn tampered_or_rebound_state_is_rejected() {
        let codec = codec();
        let round = Round::start(&codec, &key("t"), None, None).unwrap();
        let token = round_trip(&codec, &round);
        let reject = |token: &str, key: RoundKey<'_>| {
            Round::start(&codec, &key, None, Some(token))
                .unwrap_err()
                .code()
        };
        assert_eq!(reject(&format!("{token}-TAMPERED"), key("t")), -32602);
        assert_eq!(reject("garbage", key("t")), -32602);
        // Another tool, other arguments, or another key.
        assert_eq!(reject(&token, key("other")), -32602);
        let mut other_args = key("t");
        other_args.arguments = Args::Raw(r#"{"a":2}"#);
        assert_eq!(reject(&token, other_args), -32602);
        let other_key = StateCodec::new(vec![8; 32], Duration::from_secs(600));
        assert!(Round::start(&other_key, &key("t"), None, Some(&token)).is_err());
    }

    #[test]
    fn expired_state_is_rejected() {
        let codec = StateCodec::new(vec![7; 32], Duration::ZERO);
        let round = Round::start(&codec, &key("t"), None, None).unwrap();
        let mut payload: Payload = {
            let token = round_trip(&codec, &round);
            let body = token.split('.').next().unwrap();
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).unwrap()).unwrap()
        };
        payload.e = now_secs() - 1;
        let token = codec.seal(&payload).unwrap();
        assert!(Round::start(&codec, &key("t"), None, Some(&token)).is_err());
    }

    #[test]
    fn argument_digest_ignores_whitespace() {
        let digest = |raw| {
            RoundKey {
                method: "tools/call",
                target: "t",
                arguments: Args::Raw(raw),
            }
            .args_digest()
        };
        assert_eq!(digest(r#"{"a":1}"#), digest(r#"{ "a" : 1 }"#));
        assert_eq!(digest(r#"{"a":1,"b":{"y":2,"x":1}}"#), digest(r#"{"b":{"x":1,"y":2},"a":1}"#));
        assert_ne!(digest(r#"{"a":1}"#), digest(r#"{"a":2}"#));
    }

    #[test]
    fn invalid_input_responses_are_rejected() {
        let err = Round::start(&codec(), &key("t"), Some(json!(5)), None).unwrap_err();
        assert_eq!(err.code(), -32602);
    }

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Form {
        name: String,
        age: Option<u32>,
        subscribe: bool,
    }

    #[test]
    fn elicitation_schema_is_flat() {
        let schema = elicitation_schema::<Form>().unwrap();
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer", "minimum": 0 },
                    "subscribe": { "type": "boolean" },
                },
                "required": ["name", "subscribe"],
            })
        );
        #[derive(JsonSchema)]
        #[allow(dead_code)]
        struct Nested {
            inner: Form,
        }
        assert!(elicitation_schema::<Nested>().is_err());
    }
}
