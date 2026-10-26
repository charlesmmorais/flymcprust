//! `#[resource]`, `#[prompt]` and completion end to end, through the
//! in-process test client.
#![cfg(feature = "test-client")]
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mcpsdkrust::prelude::*;
use mcpsdkrust::spec::{self, PromptMessage};
use mcpsdkrust::testing::{ClientError, TestClient};
use serde_json::{Value, json};

struct Db {
    users: Vec<(u64, &'static str)>,
}

/// The application configuration.
#[resource(
    "config://app",
    mime_type = "application/json",
    ttl_ms = 60000,
    cache = "public"
)]
fn app_config() -> Json<Value> {
    Json(json!({ "theme": "dark" }))
}

/// A 1×1 PNG.
#[resource(
    "images://pixel",
    name = "pixel",
    title = "Pixel",
    mime_type = "image/png"
)]
async fn pixel_png() -> Vec<u8> {
    vec![0x89, b'P', b'N', b'G']
}

fn user_ids(prefix: &str) -> Vec<String> {
    ["1", "12", "2"]
        .iter()
        .filter(|id| id.starts_with(prefix))
        .map(|id| (*id).to_owned())
        .collect()
}

/// A user's profile.
#[resource(template = "users://{id}/profile{?fields}", mime_type = "text/plain")]
async fn user_profile(
    #[complete(user_ids)] id: u64,
    fields: Option<String>,
    db: State<Db>,
) -> Result<String, ToolError> {
    let (_, name) = db
        .users
        .iter()
        .find(|(uid, _)| *uid == id)
        .ok_or_else(ToolError::not_found)?;
    Ok(format!("{name} [{}]", fields.unwrap_or_default()))
}

/// A file below the workspace root.
#[resource(template = "file:///{+path}")]
fn file(path: String) -> Result<String, ToolError> {
    if path.split('/').any(|segment| segment == "..") {
        return Err(ToolError::user("path traversal is not allowed"));
    }
    Ok(format!("contents of {path}"))
}

/// Reviews a piece of code.
///
/// # Arguments
/// * `code` - The code to review.
#[prompt(title = "Code review")]
fn code_review(
    code: String,
    /// Its programming language.
    #[complete(values = ["rust", "ruby", "python"])]
    language: Option<String>,
) -> Vec<PromptMessage> {
    vec![
        message::user(format!(
            "Review this {} code:\n{code}",
            language.as_deref().unwrap_or("source")
        )),
        message::assistant("Sure, here is my review."),
    ]
}

/// Repeats a word.
#[prompt(name = "repeat")]
async fn repeat_word(word: String, times: u8) -> String {
    word.repeat(usize::from(times))
}

fn server() -> Server {
    Server::builder("res", "1.0.0")
        .resource::<app_config>()
        .resource::<pixel_png>()
        .resource::<user_profile>()
        .resource::<file>()
        .prompt::<code_review>()
        .prompt::<repeat_word>()
        .state(Db {
            users: vec![(1, "Ada"), (2, "Linus")],
        })
        .build()
        .unwrap()
}

fn rpc_code(err: ClientError) -> (i64, Option<Value>) {
    match err {
        ClientError::Rpc { code, data, .. } => (code, data),
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

#[tokio::test]
async fn discover_declares_the_capabilities() {
    let client = TestClient::new(server());
    let caps = client.discover().await.unwrap().capabilities;
    assert!(caps.resources.is_some());
    assert!(caps.prompts.is_some());
    assert!(caps.completions.is_some());
}

#[tokio::test]
async fn lists_are_sorted_and_described() {
    let client = TestClient::new(server());
    let resources = client.list_resources().await.unwrap();
    assert_eq!(
        serde_json::to_value(&resources.resources).unwrap(),
        json!([
            { "name": "app_config", "uri": "config://app", "description": "The application configuration.", "mimeType": "application/json" },
            { "name": "pixel", "uri": "images://pixel", "title": "Pixel", "description": "A 1×1 PNG.", "mimeType": "image/png" },
        ])
    );
    assert_eq!(resources.cache_scope, spec::CacheScope::Public);

    let templates = client.list_resource_templates().await.unwrap();
    let names: Vec<_> = templates
        .resource_templates
        .iter()
        .map(|t| (t.name.as_str(), t.uri_template.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("file", "file:///{+path}"),
            ("user_profile", "users://{id}/profile{?fields}")
        ]
    );

    let prompts = client.list_prompts().await.unwrap();
    assert_eq!(
        serde_json::to_value(&prompts.prompts).unwrap(),
        json!([
            {
                "name": "code_review",
                "title": "Code review",
                "description": "Reviews a piece of code.",
                "arguments": [
                    { "name": "code", "description": "The code to review.", "required": true },
                    { "name": "language", "description": "Its programming language.", "required": false },
                ],
            },
            {
                "name": "repeat",
                "description": "Repeats a word.",
                "arguments": [
                    { "name": "word", "required": true },
                    { "name": "times", "required": true },
                ],
            },
        ])
    );
}

#[tokio::test]
async fn reads_static_resources() {
    let client = TestClient::new(server());
    let config = client.read_resource("config://app").await.unwrap();
    assert_eq!(
        serde_json::to_value(&config.contents).unwrap(),
        json!([{ "uri": "config://app", "mimeType": "application/json", "text": "{\"theme\":\"dark\"}" }])
    );
    assert_eq!(config.ttl_ms, 60_000);
    assert_eq!(config.cache_scope, spec::CacheScope::Public);
    assert_eq!(config.result_type, "complete");

    let pixel = client.read_resource("images://pixel").await.unwrap();
    assert_eq!(
        serde_json::to_value(&pixel.contents).unwrap(),
        json!([{ "uri": "images://pixel", "mimeType": "image/png", "blob": "iVBORw==" }])
    );
    // Defaults: always stale, private.
    assert_eq!(pixel.ttl_ms, 0);
    assert_eq!(pixel.cache_scope, spec::CacheScope::Private);
}

#[tokio::test]
async fn reads_templates() {
    let client = TestClient::new(server());
    let profile = client
        .read_resource("users://2/profile?fields=name%2Cemail")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&profile.contents).unwrap(),
        json!([{ "uri": "users://2/profile?fields=name%2Cemail", "mimeType": "text/plain", "text": "Linus [name,email]" }])
    );

    let file = client.read_resource("file:///src/main.rs").await.unwrap();
    assert_eq!(
        serde_json::to_value(&file.contents).unwrap(),
        json!([{ "uri": "file:///src/main.rs", "text": "contents of src/main.rs" }])
    );
}

#[tokio::test]
async fn missing_resources_are_invalid_params_with_the_uri() {
    let client = TestClient::new(server());
    for uri in [
        "nothing://here",
        // The template matches but the handler says the user does not exist.
        "users://99/profile",
        // The variable does not parse as u64.
        "users://abc/profile",
    ] {
        let (code, data) = rpc_code(client.read_resource(uri).await.unwrap_err());
        assert_eq!(code, -32602, "{uri}");
        assert_eq!(data, Some(json!({ "uri": uri })), "{uri}");
    }
    let (code, _) = rpc_code(
        client
            .read_resource("file:///../etc/passwd")
            .await
            .unwrap_err(),
    );
    assert_eq!(code, -32602);
}

#[tokio::test]
async fn gets_prompts() {
    let client = TestClient::new(server());
    let review = client
        .get_prompt(
            "code_review",
            json!({ "code": "fn main() {}", "language": "rust" }),
        )
        .await
        .unwrap();
    assert_eq!(
        review.description.as_deref(),
        Some("Reviews a piece of code.")
    );
    assert_eq!(
        serde_json::to_value(&review.messages).unwrap(),
        json!([
            { "role": "user", "content": { "type": "text", "text": "Review this rust code:\nfn main() {}" } },
            { "role": "assistant", "content": { "type": "text", "text": "Sure, here is my review." } },
        ])
    );

    let repeated = client
        .get_prompt("repeat", json!({ "word": "ab", "times": "3" }))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&repeated.messages).unwrap(),
        json!([{ "role": "user", "content": { "type": "text", "text": "ababab" } }])
    );
}

#[tokio::test]
async fn prompt_errors_are_invalid_params() {
    let client = TestClient::new(server());
    for (name, args) in [
        ("missing", json!({})),
        ("code_review", json!({})),
        ("repeat", json!({ "word": "x", "times": "many" })),
    ] {
        let (code, _) = rpc_code(client.get_prompt(name, args).await.unwrap_err());
        assert_eq!(code, -32602, "{name}");
    }
}

#[tokio::test]
async fn completes_arguments_and_variables() {
    let client = TestClient::new(server());
    let langs = client
        .complete_prompt("code_review", "language", "RU")
        .await
        .unwrap();
    assert_eq!(langs.completion.values, ["rust", "ruby"]);

    let none = client
        .complete_prompt("code_review", "code", "f")
        .await
        .unwrap();
    assert!(none.completion.values.is_empty());

    let ids = client
        .complete_resource("users://{id}/profile{?fields}", "id", "1")
        .await
        .unwrap();
    assert_eq!(ids.completion.values, ["1", "12"]);

    let (code, _) = rpc_code(
        client
            .complete_prompt("missing", "x", "")
            .await
            .unwrap_err(),
    );
    assert_eq!(code, -32602);
}

#[tokio::test]
async fn servers_without_resources_or_prompts_do_not_answer_their_methods() {
    let client = TestClient::new(Server::builder("bare", "1").build().unwrap());
    let caps = client.discover().await.unwrap().capabilities;
    assert!(caps.resources.is_none() && caps.prompts.is_none() && caps.completions.is_none());
    for result in [
        client.list_resources().await.map(|_| ()),
        client.list_resource_templates().await.map(|_| ()),
        client.read_resource("config://app").await.map(|_| ()),
        client.list_prompts().await.map(|_| ()),
        client.get_prompt("p", json!({})).await.map(|_| ()),
        client.complete_prompt("p", "a", "").await.map(|_| ()),
    ] {
        assert_eq!(rpc_code(result.unwrap_err()).0, -32601);
    }
}

#[test]
fn build_validates_resources_and_prompts() {
    let dup = Server::builder("s", "1")
        .resource::<app_config>()
        .resource::<app_config>()
        .build();
    assert!(matches!(
        dup,
        Err(mcpsdkrust::BuildError::DuplicateResource(_))
    ));

    let dup = Server::builder("s", "1")
        .prompt::<repeat_word>()
        .prompt::<repeat_word>()
        .build();
    assert!(matches!(
        dup,
        Err(mcpsdkrust::BuildError::DuplicatePrompt(_))
    ));

    let missing = Server::builder("s", "1").resource::<user_profile>().build();
    assert!(matches!(
        missing,
        Err(mcpsdkrust::BuildError::MissingState { .. })
    ));

    let bad = Server::builder("s", "1")
        .resource_def(mcpsdkrust::ResourceDef::template(
            spec::ResourceTemplate::new("t".into(), "x://{a}{b}".into()),
            |_, _, _| Err(ToolError::not_found()),
        ))
        .build();
    assert!(matches!(
        bad,
        Err(mcpsdkrust::BuildError::InvalidResource { .. })
    ));

    let unknown_arg = Server::builder("s", "1")
        .prompt_def(
            mcpsdkrust::PromptDef::new(spec::Prompt::new("p".into()), |_, _| {
                Err(ToolError::not_found())
            })
            .complete("nope", |_| mcpsdkrust::Completion::default()),
        )
        .build();
    assert!(matches!(
        unknown_arg,
        Err(mcpsdkrust::BuildError::InvalidCompletion { .. })
    ));
}
