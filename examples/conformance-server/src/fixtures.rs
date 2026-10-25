//! Fixtures for resources, prompts, completion and multi round-trip
//! requests.

// Sampling and roots are deprecated (SEP-2577) but still tested.
#![allow(deprecated)]

use sdkmcprust::prelude::*;
use sdkmcprust::spec::{
    self, BlobResourceContents, ContentBlock, PromptMessage, Role, SamplingMessage,
    SamplingMessageContent, TextContent,
};
use sdkmcprust::{ResourceContents, ServerBuilder};
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
