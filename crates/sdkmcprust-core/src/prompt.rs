//! Prompt definitions, arguments and messages.
//!
//! Prompts are usually declared with the `#[prompt]` macro, which implements
//! [`Prompt`] for a marker type. [`PromptDef`] can also be built by hand.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use crate::completion::{CompleteFn, Completer};
use crate::context::Ctx;
use crate::spec::{
    self, AudioContent, ContentBlock, EmbeddedResource, GetPromptResult, ImageContent,
    PromptMessage, ResourceLink, Role, TextContent,
};
use crate::tool::{IntoToolError, StateRequirement, ToolError};

/// The future returned by a prompt.
pub type PromptFuture =
    Pin<Box<dyn Future<Output = Result<GetPromptResult, ToolError>> + Send + 'static>>;

/// Entry point of a prompt: receives the (string) arguments of `prompts/get`.
pub type GetFn = fn(arguments: &PromptArguments, ctx: Ctx) -> Result<PromptFuture, ToolError>;

/// A prompt that can be registered with
/// [`ServerBuilder::prompt`](crate::ServerBuilder::prompt).
pub trait Prompt: 'static {
    /// Builds the prompt definition. Called once, at registration.
    fn definition() -> PromptDef;
}

/// A registered prompt: its protocol description plus its entry point.
#[derive(Debug, Clone)]
pub struct PromptDef {
    pub(crate) prompt: spec::Prompt,
    pub(crate) get: GetFn,
    pub(crate) required_state: Vec<StateRequirement>,
    pub(crate) completers: Vec<Completer>,
}

impl PromptDef {
    /// Creates a definition from the protocol description and entry point.
    #[must_use]
    pub fn new(prompt: spec::Prompt, get: GetFn) -> Self {
        Self {
            prompt,
            get,
            required_state: Vec::new(),
            completers: Vec::new(),
        }
    }

    /// Declares that the prompt needs shared state of type `T`.
    #[must_use]
    pub fn require_state<T: Send + Sync + 'static>(mut self) -> Self {
        self.required_state.push(StateRequirement::of::<T>());
        self
    }

    /// Attaches a completion provider to an argument.
    #[must_use]
    pub fn complete(mut self, argument: impl Into<String>, complete: CompleteFn) -> Self {
        self.completers.push(Completer {
            argument: argument.into(),
            complete,
        });
        self
    }

    /// The protocol description.
    #[must_use]
    pub fn prompt(&self) -> &spec::Prompt {
        &self.prompt
    }
}

/// The arguments of a `prompts/get` request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptArguments(BTreeMap<String, String>);

impl PromptArguments {
    pub(crate) fn new(arguments: BTreeMap<String, String>) -> Self {
        Self(arguments)
    }

    /// The raw value of an argument.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Parses a required argument.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument error (`-32602`) if the argument is missing
    /// or does not parse.
    pub fn required<T: FromStr>(&self, name: &str) -> Result<T, ToolError> {
        let value = self
            .get(name)
            .ok_or_else(|| ToolError::invalid_argument(format!("missing argument `{name}`")))?;
        parse(name, value)
    }

    /// Parses an optional argument.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument error (`-32602`) if the argument is present
    /// but does not parse.
    pub fn optional<T: FromStr>(&self, name: &str) -> Result<Option<T>, ToolError> {
        self.get(name).map(|v| parse(name, v)).transpose()
    }

    /// All arguments.
    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

fn parse<T: FromStr>(name: &str, value: &str) -> Result<T, ToolError> {
    value
        .parse()
        .map_err(|_| ToolError::invalid_argument(format!("argument `{name}` is not valid")))
}

/// Content that can appear in a message: `&str`, `String` (as text) and the
/// specification's content types.
pub trait IntoContent {
    /// Performs the conversion.
    fn into_content(self) -> ContentBlock;
}

impl IntoContent for &str {
    fn into_content(self) -> ContentBlock {
        TextContent::new(self.to_owned()).into()
    }
}

impl IntoContent for String {
    fn into_content(self) -> ContentBlock {
        TextContent::new(self).into()
    }
}

macro_rules! content_types {
    ($($t:ty),* $(,)?) => {
        $(impl IntoContent for $t {
            fn into_content(self) -> ContentBlock {
                self.into()
            }
        })*
    };
}

content_types!(
    TextContent,
    ImageContent,
    AudioContent,
    EmbeddedResource,
    ResourceLink
);

impl IntoContent for ContentBlock {
    fn into_content(self) -> ContentBlock {
        self
    }
}

/// Builders for prompt messages.
pub mod message {
    use super::{IntoContent, PromptMessage, Role};

    /// A message from the user.
    pub fn user(content: impl IntoContent) -> PromptMessage {
        PromptMessage::new(content.into_content(), Role::User)
    }

    /// A message from the assistant.
    pub fn assistant(content: impl IntoContent) -> PromptMessage {
        PromptMessage::new(content.into_content(), Role::Assistant)
    }
}

/// Conversion of a prompt's return value into a result.
///
/// | Return type | Result |
/// | --- | --- |
/// | `String`, `&'static str` | One user text message |
/// | `PromptMessage`, `Vec<PromptMessage>` | The messages |
/// | `GetPromptResult` | As given |
/// | `Result<T, E>` | `T` as above; `E` as for tools |
pub trait IntoPromptResult {
    /// Performs the conversion.
    ///
    /// # Errors
    ///
    /// Returns the handler's error.
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError>;
}

fn messages(messages: Vec<PromptMessage>) -> GetPromptResult {
    GetPromptResult::new(messages, "complete".to_owned())
}

impl IntoPromptResult for String {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        Ok(messages(vec![message::user(self)]))
    }
}

impl IntoPromptResult for &'static str {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        Ok(messages(vec![message::user(self)]))
    }
}

impl IntoPromptResult for PromptMessage {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        Ok(messages(vec![self]))
    }
}

impl IntoPromptResult for Vec<PromptMessage> {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        Ok(messages(self))
    }
}

impl IntoPromptResult for GetPromptResult {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        Ok(self)
    }
}

impl<T: IntoPromptResult, E: IntoToolError> IntoPromptResult for Result<T, E> {
    fn into_prompt_result(self) -> Result<GetPromptResult, ToolError> {
        match self {
            Ok(value) => value.into_prompt_result(),
            Err(error) => Err(error.into_tool_error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_parse() {
        let args = PromptArguments::new(BTreeMap::from([
            ("n".to_owned(), "3".to_owned()),
            ("s".to_owned(), "x".to_owned()),
        ]));
        assert_eq!(args.required::<u8>("n").unwrap(), 3);
        assert_eq!(args.optional::<u8>("missing").unwrap(), None);
        assert!(args.required::<u8>("s").is_err());
        assert!(args.required::<String>("missing").is_err());
    }

    #[test]
    fn string_becomes_a_user_message() {
        let r = "hello".into_prompt_result().unwrap();
        assert_eq!(
            serde_json::to_value(&r.messages).unwrap(),
            serde_json::json!([{ "role": "user", "content": { "type": "text", "text": "hello" } }])
        );
    }
}
