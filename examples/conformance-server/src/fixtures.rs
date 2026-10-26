//! Fixtures for resources, prompts, completion and multi round-trip
//! requests.

// Sampling and roots are deprecated (SEP-2577) but still tested.
#![allow(deprecated)]

use flymcprust::prelude::*;
use flymcprust::spec::{
    self, BlobResourceContents, ContentBlock, PromptMessage, Role, SamplingMessage,
    SamplingMessageContent, TextContent,
};
use flymcprust::{ResourceContents, ServerBuilder};
use serde::Deserialize;

use crate::{PNG, image, text_resource};

/// A static text resource.
#[resource("test://static-text", mime_type = "text/plain")]
fn static_text() -> &'static str {
    "This is the content of the static text resource."
}

/// A static binary resource.
#[resource("test://static-binary", mime_type = "image/png")]
fn static_binary() -> ResourceContents {
    let mut blob = BlobResourceContents::new(PNG.to_owned(), "test://static-binary".to_owned());
    blob.mime_type = Some("image/png".to_owned());
    ResourceContents::Blob(blob)
}

/// A resource that clients may watch.
#[resource("test://watched-resource", mime_type = "text/plain")]
fn watched_resource() -> &'static str {
    "Watched resource content."
}

/// A parameterized resource.
#[resource(template = "test://template/{id}/data", mime_type = "application/json")]
fn template_data(#[complete(values = ["123", "456"])] id: String) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": id,
        "templateTest": true,
        "data": format!("Data for ID: {id}"),
    }))
}

/// A prompt without arguments.
#[prompt]
fn test_simple_prompt() -> &'static str {
    "This is a simple prompt for testing."
}

fn arg1_values(prefix: &str) -> Vec<&'static str> {
    ["paris", "park", "party", "test-value"]
        .into_iter()
        .filter(|v| v.starts_with(prefix))
        .collect()
}

/// A prompt with arguments.
///
/// # Arguments
/// * `arg1` - First test argument
/// * `arg2` - Second test argument
#[prompt]
fn test_prompt_with_arguments(#[complete(arg1_values)] arg1: String, arg2: String) -> String {
    format!("Prompt with arguments: arg1='{arg1}', arg2='{arg2}'")
}

/// A prompt with an embedded resource.
///
/// # Arguments
/// * `resourceUri` - URI of the resource to embed
#[prompt]
#[allow(non_snake_case)]
fn test_prompt_with_embedded_resource(resourceUri: String) -> Vec<PromptMessage> {
    vec![
        message::user(text_resource(
            &resourceUri,
            "text/plain",
            "Embedded resource content for testing.",
        )),
        message::user("Please process the embedded resource above."),
    ]
}

/// A prompt with an image.
#[prompt]
fn test_prompt_with_image() -> Vec<PromptMessage> {
    vec![
        message::user(image()),
        message::user("Please analyze the image above."),
    ]
}

#[derive(Deserialize, schemars::JsonSchema)]
struct UserName {
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Confirmation {
    ok: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FavoriteColor {
    color: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct PromptContext {
    context: String,
}

fn sampling(text: &str, max_tokens: i64) -> spec::CreateMessageRequestParams {
    spec::CreateMessageRequestParams::new(
        max_tokens,
        vec![SamplingMessage::new(
            SamplingMessageContent::TextContent(TextContent::new(text.to_owned())),
            Role::User,
        )],
    )
}

fn sampled_text(result: &spec::CreateMessageResult) -> String {
    match &result.content {
        spec::CreateMessageResultContent::TextContent(t) => t.text.clone(),
        _ => "(non-text sample)".to_owned(),
    }
}

/// Asks for the user's name.
#[tool]
async fn test_input_required_result_elicitation(ctx: Ctx) -> Result<String, ToolError> {
    let user: UserName = ctx.elicit("user_name", "What is your name?").await?;
    Ok(format!("Hello, {}!", user.name))
}

/// Asks the client to sample.
#[tool]
async fn test_input_required_result_sampling(ctx: Ctx) -> Result<String, ToolError> {
    let answer = ctx
        .sample(
            "capital_question",
            sampling("What is the capital of France?", 100),
        )
        .await?;
    Ok(sampled_text(&answer))
}

/// Asks for the client's roots.
#[tool]
async fn test_input_required_result_list_roots(ctx: Ctx) -> Result<String, ToolError> {
    let roots = ctx.list_roots("client_roots").await?;
    let uris: Vec<&str> = roots.roots.iter().map(|r| r.uri.as_str()).collect();
    Ok(format!("Roots: {}", uris.join(", ")))
}

/// Confirms that the signed request state was received and verified.
#[tool]
async fn test_input_required_result_request_state(ctx: Ctx) -> Result<String, ToolError> {
    let confirmation: Confirmation = ctx.elicit("confirm", "Please confirm").await?;
    let verified = if ctx.input_round() > 0 {
        "state-ok"
    } else {
        "no-state"
    };
    Ok(format!("{verified}: ok={}", confirmation.ok))
}

/// Asks for three kinds of input in one round.
#[tool]
async fn test_input_required_result_multiple_inputs(ctx: Ctx) -> Result<String, ToolError> {
    let (user, greeting, roots) = tokio::join!(
        ctx.elicit::<UserName>("user_name", "What is your name?"),
        ctx.sample("greeting", sampling("Generate a greeting", 50)),
        ctx.list_roots("client_roots"),
    );
    let (user, greeting, roots) = (user?, greeting?, roots?);
    Ok(format!(
        "{} for {} with {} root(s)",
        sampled_text(&greeting),
        user.name,
        roots.roots.len()
    ))
}

/// Asks two questions in two rounds.
#[tool]
async fn test_input_required_result_multi_round(ctx: Ctx) -> Result<String, ToolError> {
    let user: UserName = ctx.elicit("step1", "Step 1: What is your name?").await?;
    let color: FavoriteColor = ctx
        .elicit("step2", "Step 2: What is your favorite color?")
        .await?;
    Ok(format!(
        "Multi-round complete for {} who likes {}",
        user.name, color.color
    ))
}

/// Uses signed request state, so tampering is detected.
#[tool]
async fn test_input_required_result_tampered_state(ctx: Ctx) -> Result<String, ToolError> {
    let confirmation: Confirmation = ctx.elicit("confirm", "Please confirm").await?;
    Ok(format!("ok={}", confirmation.ok))
}

/// Only asks for input the client declared support for.
#[tool]
async fn test_input_required_result_capabilities(ctx: Ctx) -> Result<String, ToolError> {
    let caps = ctx.client_capabilities();
    let wants_elicitation = caps.elicitation.is_some();
    let wants_sampling = caps.sampling.is_some();
    let (user, sample) = tokio::join!(
        async {
            if wants_elicitation {
                ctx.elicit::<UserName>("user_name", "What is your name?")
                    .await
                    .map(|u| Some(u.name))
            } else {
                Ok(None)
            }
        },
        async {
            if wants_sampling {
                ctx.sample(
                    "capital_question",
                    sampling("What is the capital of France?", 100),
                )
                .await
                .map(|r| Some(sampled_text(&r)))
            } else {
                Ok(None)
            }
        },
    );
    Ok(format!("name={:?} sample={:?}", user?, sample?))
}

/// A prompt that needs context from the user.
#[prompt]
async fn test_input_required_result_prompt(ctx: Ctx) -> Result<Vec<PromptMessage>, ToolError> {
    let context: PromptContext = ctx
        .elicit("user_context", "What context should the prompt use?")
        .await?;
    Ok(vec![message::user(ContentBlock::from(TextContent::new(
        format!("Use this context: {}", context.context),
    )))])
}

/// Registers every fixture of this module.
pub(crate) fn register(builder: ServerBuilder) -> ServerBuilder {
    builder
        .resource::<static_text>()
        .resource::<static_binary>()
        .resource::<watched_resource>()
        .resource::<template_data>()
        .prompt::<test_simple_prompt>()
        .prompt::<test_prompt_with_arguments>()
        .prompt::<test_prompt_with_embedded_resource>()
        .prompt::<test_prompt_with_image>()
        .prompt::<test_input_required_result_prompt>()
        .tool::<test_input_required_result_elicitation>()
        .tool::<test_input_required_result_sampling>()
        .tool::<test_input_required_result_list_roots>()
        .tool::<test_input_required_result_request_state>()
        .tool::<test_input_required_result_multiple_inputs>()
        .tool::<test_input_required_result_multi_round>()
        .tool::<test_input_required_result_tampered_state>()
        .tool::<test_input_required_result_capabilities>()
}

// ---------------------------------------------------------------------------
// Fixtures of the 2025-11-25 scenarios. Legacy clients receive these input
// requests as server-to-client requests; the handlers are the same.

fn describe(
    answer: &flymcprust::Elicitation<serde_json::Map<String, serde_json::Value>>,
) -> String {
    match answer {
        flymcprust::Elicitation::Accepted(content) => {
            format!(
                "action=accept, content={}",
                serde_json::Value::Object(content.clone())
            )
        }
        flymcprust::Elicitation::Declined => "action=decline, content={}".to_owned(),
        flymcprust::Elicitation::Cancelled => "action=cancel, content={}".to_owned(),
    }
}

/// Requests LLM sampling from the client.
///
/// # Arguments
/// * `prompt` - The prompt to send to the LLM
#[tool]
async fn test_sampling(ctx: Ctx, prompt: String) -> Result<String, ToolError> {
    let answer = ctx.sample("sampling", sampling(&prompt, 100)).await?;
    Ok(format!("LLM response: {}", sampled_text(&answer)))
}

/// Requests user input from the client.
///
/// # Arguments
/// * `message` - The message to show the user
#[tool]
async fn test_elicitation(ctx: Ctx, message: String) -> Result<String, ToolError> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "username": { "type": "string", "description": "User's response" },
            "email": { "type": "string", "description": "User's email address" }
        },
        "required": ["username", "email"]
    });
    let answer = ctx
        .elicit_with_schema("elicitation", message, schema)
        .await?;
    Ok(format!("User response: {}", describe(&answer)))
}

/// Requests elicitation with default values for every primitive type.
#[tool]
async fn test_elicitation_sep1034_defaults(ctx: Ctx) -> Result<String, ToolError> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "default": "John Doe" },
            "age": { "type": "integer", "default": 30 },
            "score": { "type": "number", "default": 95.5 },
            "status": { "type": "string", "enum": ["active", "inactive", "pending"], "default": "active" },
            "verified": { "type": "boolean", "default": true }
        }
    });
    let answer = ctx
        .elicit_with_schema("defaults", "Please review the defaults", schema)
        .await?;
    Ok(format!("Elicitation completed: {}", describe(&answer)))
}

/// Requests elicitation with the five enum variants.
#[tool]
async fn test_elicitation_sep1330_enums(ctx: Ctx) -> Result<String, ToolError> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "untitledSingle": { "type": "string", "enum": ["option1", "option2", "option3"] },
            "titledSingle": { "type": "string", "oneOf": [
                { "const": "value1", "title": "First Option" },
                { "const": "value2", "title": "Second Option" },
                { "const": "value3", "title": "Third Option" }
            ] },
            "legacyEnum": { "type": "string", "enum": ["opt1", "opt2", "opt3"],
                "enumNames": ["Option One", "Option Two", "Option Three"] },
            "untitledMulti": { "type": "array", "items": { "type": "string", "enum": ["option1", "option2", "option3"] } },
            "titledMulti": { "type": "array", "items": { "anyOf": [
                { "const": "value1", "title": "First Choice" },
                { "const": "value2", "title": "Second Choice" },
                { "const": "value3", "title": "Third Choice" }
            ] } }
        }
    });
    let answer = ctx
        .elicit_with_schema("enums", "Please choose", schema)
        .await?;
    Ok(format!("Elicitation completed: {}", describe(&answer)))
}

/// Registers the 2025-11-25 fixtures.
pub(crate) fn register_legacy(builder: ServerBuilder) -> ServerBuilder {
    builder
        .tool::<test_sampling>()
        .tool::<test_elicitation>()
        .tool::<test_elicitation_sep1034_defaults>()
        .tool::<test_elicitation_sep1330_enums>()
}

// ---------------------------------------------------------------------------
// Progress and logging.

/// Reports progress 0, 50 and 100 out of 100.
#[tool]
async fn test_tool_with_progress(ctx: Ctx) -> String {
    for (i, step) in [0.0, 50.0, 100.0].into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        ctx.progress(step, 100.0);
    }
    "Progress reported".to_owned()
}

/// Sends three log messages while it runs.
#[tool]
async fn test_tool_with_logging(ctx: Ctx) -> String {
    use flymcprust::spec::LoggingLevel;
    ctx.log(LoggingLevel::Info, "Tool execution started");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    ctx.log(LoggingLevel::Info, "Tool processing data");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    ctx.log(LoggingLevel::Info, "Tool execution completed");
    "Logging completed".to_owned()
}

/// Logs (only delivered when the request sets a log level).
#[tool]
async fn test_logging_tool(ctx: Ctx) -> String {
    ctx.log(
        flymcprust::spec::LoggingLevel::Info,
        "diagnostic log message",
    );
    "logged".to_owned()
}

/// Asks for input; on 2026-07-28 that is an input-required result, never a
/// request on the response stream.
#[tool]
async fn test_streaming_elicitation(ctx: Ctx) -> Result<String, ToolError> {
    ctx.progress(1.0, 2.0);
    let user: UserName = ctx.elicit("user_name", "What is your name?").await?;
    Ok(format!("Hello, {}!", user.name))
}

/// Registers the progress and logging fixtures.
pub(crate) fn register_notifications(builder: ServerBuilder) -> ServerBuilder {
    builder
        .tool::<test_tool_with_progress>()
        .tool::<test_tool_with_logging>()
        .tool::<test_logging_tool>()
        .tool::<test_streaming_elicitation>()
}
