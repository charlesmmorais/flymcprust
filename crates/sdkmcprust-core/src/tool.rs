//! Tool definitions, outputs and errors.
//!
//! Tools are usually declared with the `#[tool]` macro, which implements
//! [`Tool`] for a marker type. The trait can also be implemented by hand.

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::context::Ctx;
use crate::spec::{self, CallToolResult, ContentBlock, TextContent};

/// The future returned by a tool invocation.
pub type ToolFuture = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;

/// The outcome of a tool invocation.
pub type ToolResult = Result<ToolOutput, ToolError>;

/// Entry point of a tool: deserializes the arguments synchronously (borrowing
/// the request buffer) and returns the future that runs the tool.
pub type CallFn = fn(arguments: Option<&RawValue>, ctx: Ctx) -> Result<ToolFuture, ToolError>;

/// A tool that can be registered with
/// [`ServerBuilder::tool`](crate::ServerBuilder::tool).
pub trait Tool: 'static {
    /// Builds the tool definition. Called once, at registration.
    fn definition() -> ToolDef;
}

/// A shared-state type a tool needs.
#[derive(Debug, Clone, Copy)]
pub struct StateRequirement {
    pub(crate) type_id: TypeId,
    pub(crate) type_name: &'static str,
}

/// A registered tool: its protocol description plus its entry point.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub(crate) tool: spec::Tool,
    pub(crate) call: CallFn,
    pub(crate) required_state: Vec<StateRequirement>,
}

impl ToolDef {
    /// Creates a definition from the protocol description and entry point.
    #[must_use]
    pub fn new(tool: spec::Tool, call: CallFn) -> Self {
        Self {
            tool,
            call,
            required_state: Vec::new(),
        }
    }

    /// Declares that the tool needs shared state of type `T`.
    #[must_use]
    pub fn require_state<T: Send + Sync + 'static>(mut self) -> Self {
        self.required_state.push(StateRequirement {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
        });
        self
    }

    /// The protocol description.
    #[must_use]
    pub fn tool(&self) -> &spec::Tool {
        &self.tool
    }
}

/// What a tool produced.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolOutput {
    /// A final result (`resultType: "complete"`).
    Complete(CallToolResult),
}

/// An error raised by a tool.
///
/// - [`ToolError::user`] errors are shown to the model (`isError: true`) so
///   it can correct itself. Use them for invalid input or business-rule
///   violations.
/// - Any other error (`?` on a `std::error::Error`) is logged with full
///   detail on the server, and the client receives a generic message.
pub struct ToolError {
    kind: ToolErrorKind,
}

enum ToolErrorKind {
    User(Cow<'static, str>),
    InvalidArguments(String),
    Internal(Box<dyn std::error::Error + Send + Sync + 'static>),
    Timeout,
    Panicked,
}

impl ToolError {
    /// An error whose message is shown to the model.
    pub fn user(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind: ToolErrorKind::User(message.into()),
        }
    }

    /// An internal error: logged on the server, hidden from the client.
    pub fn internal(error: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        Self {
            kind: ToolErrorKind::Internal(error.into()),
        }
    }

    /// The arguments did not match the tool's input schema. The message comes
    /// from the deserializer and only describes the client's own input.
    #[must_use]
    pub fn invalid_arguments(error: &serde_json::Error) -> Self {
        Self {
            kind: ToolErrorKind::InvalidArguments(error.to_string()),
        }
    }

    pub(crate) fn timeout() -> Self {
        Self {
            kind: ToolErrorKind::Timeout,
        }
    }

    pub(crate) fn panicked() -> Self {
        Self {
            kind: ToolErrorKind::Panicked,
        }
    }

    /// Converts into an `isError` result, logging internal details.
    pub(crate) fn into_call_result(self, tool: &str) -> CallToolResult {
        let message: Cow<'static, str> = match self.kind {
            ToolErrorKind::User(message) => message,
            ToolErrorKind::InvalidArguments(details) => {
                Cow::Owned(format!("Invalid arguments: {details}"))
            }
            ToolErrorKind::Internal(error) => {
                tracing::error!(tool, error = %error, "tool failed");
                Cow::Borrowed("The tool failed because of an internal error.")
            }
            ToolErrorKind::Timeout => {
                tracing::warn!(tool, "tool timed out");
                Cow::Borrowed("The tool did not finish within the time limit.")
            }
            ToolErrorKind::Panicked => {
                tracing::error!(tool, "tool panicked");
                Cow::Borrowed("The tool failed because of an internal error.")
            }
        };
        let mut result = CallToolResult::new(
            vec![ContentBlock::from(TextContent::new(message.into_owned()))],
            "complete".to_owned(),
        );
        result.is_error = Some(true);
        result
    }
}

impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ToolErrorKind::User(m) => f.debug_tuple("User").field(m).finish(),
            ToolErrorKind::InvalidArguments(m) => {
                f.debug_tuple("InvalidArguments").field(m).finish()
            }
            ToolErrorKind::Internal(e) => f.debug_tuple("Internal").field(e).finish(),
            ToolErrorKind::Timeout => f.write_str("Timeout"),
            ToolErrorKind::Panicked => f.write_str("Panicked"),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ToolErrorKind::User(m) => f.write_str(m),
            ToolErrorKind::InvalidArguments(m) => write!(f, "invalid arguments: {m}"),
            ToolErrorKind::Internal(e) => write!(f, "internal error: {e}"),
            ToolErrorKind::Timeout => f.write_str("timed out"),
            ToolErrorKind::Panicked => f.write_str("panicked"),
        }
    }
}

/// Conversion of tool errors. Implemented for [`ToolError`] and for every
/// `std::error::Error` (treated as internal).
pub trait IntoToolError {
    /// Performs the conversion.
    fn into_tool_error(self) -> ToolError;
}

impl IntoToolError for ToolError {
    fn into_tool_error(self) -> ToolError {
        self
    }
}

impl<E: std::error::Error + Send + Sync + 'static> IntoToolError for E {
    fn into_tool_error(self) -> ToolError {
        ToolError::internal(self)
    }
}

/// Wraps a value returned as structured content. The tool's `outputSchema`
/// is generated from `T`, and the result carries both `structuredContent`
/// and the serialized JSON as text, as the specification recommends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Json<T>(pub T);

/// Conversion of a tool's return value into a result.
pub trait IntoToolResult {
    /// Performs the conversion.
    fn into_tool_result(self) -> ToolResult;

    /// The `outputSchema` advertised for this return type, if any.
    fn output_schema() -> Option<Map<String, Value>> {
        None
    }
}

fn text_result(text: String) -> ToolResult {
    Ok(ToolOutput::Complete(CallToolResult::new(
        vec![ContentBlock::from(TextContent::new(text))],
        "complete".to_owned(),
    )))
}

impl IntoToolResult for String {
    fn into_tool_result(self) -> ToolResult {
        text_result(self)
    }
}

impl IntoToolResult for &'static str {
    fn into_tool_result(self) -> ToolResult {
        text_result(self.to_owned())
    }
}

impl IntoToolResult for Cow<'static, str> {
    fn into_tool_result(self) -> ToolResult {
        text_result(self.into_owned())
    }
}

macro_rules! display_results {
    ($($t:ty),* $(,)?) => {
        $(impl IntoToolResult for $t {
            fn into_tool_result(self) -> ToolResult {
                text_result(self.to_string())
            }
        })*
    };
}

display_results!(
    bool, char, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
);

impl IntoToolResult for () {
    fn into_tool_result(self) -> ToolResult {
        Ok(ToolOutput::Complete(CallToolResult::new(
            Vec::new(),
            "complete".to_owned(),
        )))
    }
}

impl IntoToolResult for CallToolResult {
    fn into_tool_result(self) -> ToolResult {
        Ok(ToolOutput::Complete(self))
    }
}

impl IntoToolResult for ContentBlock {
    fn into_tool_result(self) -> ToolResult {
        Ok(ToolOutput::Complete(CallToolResult::new(
            vec![self],
            "complete".to_owned(),
        )))
    }
}

impl IntoToolResult for Vec<ContentBlock> {
    fn into_tool_result(self) -> ToolResult {
        Ok(ToolOutput::Complete(CallToolResult::new(
            self,
            "complete".to_owned(),
        )))
    }
}

impl<T: Serialize + JsonSchema> IntoToolResult for Json<T> {
    fn into_tool_result(self) -> ToolResult {
        let value = serde_json::to_value(&self.0).map_err(ToolError::internal)?;
        let text = serde_json::to_string(&value).map_err(ToolError::internal)?;
        let mut result = CallToolResult::new(
            vec![ContentBlock::from(TextContent::new(text))],
            "complete".to_owned(),
        );
        result.structured_content = Some(value);
        Ok(ToolOutput::Complete(result))
    }

    fn output_schema() -> Option<Map<String, Value>> {
        Some(schema_for::<T>())
    }
}

impl<T: IntoToolResult, E: IntoToolError> IntoToolResult for Result<T, E> {
    fn into_tool_result(self) -> ToolResult {
        match self {
            Ok(value) => value.into_tool_result(),
            Err(error) => Err(error.into_tool_error()),
        }
    }

    fn output_schema() -> Option<Map<String, Value>> {
        T::output_schema()
    }
}

/// Generates a JSON Schema (2020-12) for `T`, as used in `inputSchema` and
/// `outputSchema`. The root `$schema` and `title` keys are removed: 2020-12
/// is the default dialect, and the title would expose a Rust type name.
#[must_use]
pub fn schema_for<T: JsonSchema>() -> Map<String, Value> {
    let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut map = match schema.to_value() {
        Value::Object(map) => map,
        // `true`/`false` schemas: accept anything / nothing.
        Value::Bool(true) => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("not".to_owned(), Value::Object(Map::new()));
            tracing::warn!(schema = %other, "unexpected schema shape");
            map
        }
    };
    map.remove("$schema");
    map.remove("title");
    for value in map.values_mut() {
        strip_nonstandard_formats(value);
    }
    map
}

/// Numeric `format` values emitted by schemars that JSON Schema 2020-12 does
/// not define. Validators ignore (and some warn about) them, and they cost
/// model context; range keywords such as `minimum` carry the useful part.
const NONSTANDARD_FORMATS: &[&str] = &[
    "int", "int8", "int16", "int32", "int64", "int128", "uint", "uint8", "uint16", "uint32",
    "uint64", "uint128", "float", "double",
];

fn strip_nonstandard_formats(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|f| NONSTANDARD_FORMATS.contains(&f))
            {
                map.remove("format");
            }
            map.values_mut().for_each(strip_nonstandard_formats);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nonstandard_formats),
        _ => {}
    }
}

/// Builds a tool `inputSchema` from the JSON Schema of `T`, which must
/// describe an object (tool arguments are always a JSON object).
#[must_use]
pub fn input_schema_for<T: JsonSchema>() -> spec::ToolInputSchema {
    let mut map = schema_for::<T>();
    // `type: "object"` is carried by the dedicated marker field.
    map.remove("type");
    spec::ToolInputSchema {
        schema: None,
        r#type: spec::ObjectType::Object,
        extra: map,
    }
}

/// Wraps a generated schema as a tool `outputSchema`.
#[must_use]
pub fn output_schema_from(schema: Map<String, Value>) -> spec::ToolOutputSchema {
    spec::ToolOutputSchema {
        schema: None,
        extra: schema,
    }
}

/// Deserializes tool arguments. A missing `arguments` member is treated as
/// an empty object.
///
/// # Errors
///
/// Returns [`ToolError::invalid_arguments`] when the arguments do not match.
pub fn parse_arguments<T: DeserializeOwned>(arguments: Option<&RawValue>) -> Result<T, ToolError> {
    let raw = arguments.map_or("{}", RawValue::get);
    serde_json::from_str(raw).map_err(|e| ToolError::invalid_arguments(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, JsonSchema)]
    struct Point {
        x: i32,
        y: i32,
    }

    fn complete(r: ToolResult) -> CallToolResult {
        match r {
            Ok(ToolOutput::Complete(c)) => c,
            Err(e) => panic!("unexpected error {e:?}"),
        }
    }

    #[test]
    fn json_output_has_structured_content_and_schema() {
        let result = complete(Json(Point { x: 1, y: 2 }).into_tool_result());
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({"x": 1, "y": 2}))
        );
        let schema = <Json<Point> as IntoToolResult>::output_schema().unwrap();
        assert_eq!(schema.get("type"), Some(&Value::from("object")));
        assert!(!schema.contains_key("title"));
        assert!(!schema.contains_key("$schema"));
        // `format: "int32"` is not a JSON Schema 2020-12 format.
        assert_eq!(
            schema["properties"]["x"],
            serde_json::json!({ "type": "integer" })
        );
    }

    #[test]
    fn errors_hide_internal_details() {
        let io = std::io::Error::other("secret path /etc/shadow");
        let result = Err::<String, _>(io)
            .into_tool_result()
            .unwrap_err()
            .into_call_result("t");
        assert_eq!(result.is_error, Some(true));
        let text = serde_json::to_string(&result).unwrap();
        assert!(!text.contains("shadow"));

        let user = ToolError::user("b must not be zero").into_call_result("t");
        assert!(
            serde_json::to_string(&user)
                .unwrap()
                .contains("b must not be zero")
        );
    }

    #[test]
    fn missing_arguments_are_an_empty_object() {
        #[derive(serde::Deserialize)]
        struct NoArgs {}
        assert!(parse_arguments::<NoArgs>(None).is_ok());
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct NeedsA {
            a: i32,
        }
        let err = parse_arguments::<NeedsA>(None).unwrap_err();
        assert!(err.to_string().contains("missing field `a`"));
    }
}
