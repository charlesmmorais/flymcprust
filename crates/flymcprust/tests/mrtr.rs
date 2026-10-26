//! Multi round-trip requests end to end: elicitation through `Ctx`, signed
//! `requestState`, capability checks, and MRTR on tools, prompts and
//! resources.
#![cfg(feature = "test-client")]
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicU32, Ordering};

use flymcprust::prelude::*;
use flymcprust::testing::{ClientError, TestClient};
use flymcprust::{Elicitation, spec};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, schemars::JsonSchema)]
struct Confirm {
    /// Whether to proceed.
    confirmed: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Name {
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Color {
    color: String,
}

/// Executions of `counted`, to check the replay model.
static COUNTED_RUNS: AtomicU32 = AtomicU32::new(0);

/// Counts its executions.
#[tool]
async fn counted(ctx: Ctx) -> Result<String, ToolError> {
    COUNTED_RUNS.fetch_add(1, Ordering::Relaxed);
    let name: Name = ctx.elicit("a", "A?").await?;
    let color: Color = ctx.elicit("b", "B?").await?;
    Ok(format!("{} {}", name.name, color.color))
}

/// Deploys after confirmation.
#[tool]
async fn deploy(ctx: Ctx, env: String) -> Result<String, ToolError> {
    let answer: Confirm = ctx.elicit("confirm", format!("Deploy to {env}?")).await?;
    if !answer.confirmed {
        return Err(ToolError::user("not confirmed"));
    }
    Ok(format!(
        "deployed to {env} after {} round(s)",
        ctx.input_round()
    ))
}

/// Asks two questions in the same round.
#[tool]
async fn profile(ctx: Ctx) -> Result<String, ToolError> {
    let (name, color) = tokio::join!(
        ctx.elicit::<Name>("name", "Your name?"),
        ctx.elicit::<Color>("color", "Favorite color?"),
    );
    Ok(format!("{} likes {}", name?.name, color?.color))
}

/// Asks two questions in two rounds.
#[tool]
async fn wizard(ctx: Ctx) -> Result<String, ToolError> {
    let name: Name = ctx.elicit("step1", "Step 1: your name?").await?;
    let color: Color = ctx.elicit("step2", "Step 2: favorite color?").await?;
    Ok(format!("{} likes {}", name.name, color.color))
}

/// Reports the user's choice instead of failing.
#[tool]
async fn survey(ctx: Ctx) -> Result<String, ToolError> {
    Ok(match ctx.elicit_with_action::<Name>("q", "Name?").await? {
        Elicitation::Accepted(n) => format!("hello {}", n.name),
        Elicitation::Declined => "declined".to_owned(),
        Elicitation::Cancelled => "cancelled".to_owned(),
    })
}

/// A prompt that needs context from the user.
#[prompt]
async fn contextual(ctx: Ctx, topic: String) -> Result<String, ToolError> {
    #[derive(Deserialize, schemars::JsonSchema)]
    struct Context {
        context: String,
    }
    let c: Context = ctx.elicit("ctx", "Which context?").await?;
    Ok(format!("Write about {topic} for {}", c.context))
}

/// A resource behind a confirmation.
#[resource("secret://note")]
async fn secret(ctx: Ctx) -> Result<String, ToolError> {
    let ok: Confirm = ctx.elicit("reveal", "Reveal the note?").await?;
    Ok(if ok.confirmed { "the note" } else { "hidden" }.to_owned())
}

fn builder() -> flymcprust::ServerBuilder {
    Server::builder("mrtr", "1.0.0")
        .tool::<deploy>()
        .tool::<profile>()
        .tool::<wizard>()
        .tool::<survey>()
        .prompt::<contextual>()
        .resource::<secret>()
        .tool::<counted>()
}

fn client(server: Server) -> TestClient {
    TestClient::new(server).with_meta(json!({
        "io.modelcontextprotocol/protocolVersion": spec::PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": {} },
    }))
}

async fn call(client: &TestClient, params: Value) -> Result<Value, ClientError> {
    client.request("tools/call", params).await
}

fn accept(content: Value) -> Value {
    json!({ "action": "accept", "content": content })
}

fn code(result: Result<Value, ClientError>) -> i64 {
    match result.unwrap_err() {
        ClientError::Rpc { code, .. } => code,
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

#[tokio::test]
async fn elicitation_round_trip() {
    let client = client(builder().build().unwrap());
    let args = json!({ "env": "prod" });

    let first = call(&client, json!({ "name": "deploy", "arguments": args }))
        .await
        .unwrap();
    assert_eq!(first["resultType"], "input_required");
    assert_eq!(
        first["inputRequests"],
        json!({ "confirm": {
            "method": "elicitation/create",
            "params": {
                "message": "Deploy to prod?",
                "requestedSchema": {
                    "type": "object",
                    "properties": { "confirmed": { "type": "boolean", "description": "Whether to proceed." } },
                    "required": ["confirmed"],
                },
            },
        } })
    );
    assert_eq!(
        first["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "mrtr"
    );
    let state = first["requestState"].as_str().unwrap();

    let done = call(
        &client,
        json!({
            "name": "deploy",
            "arguments": args,
            "inputResponses": { "confirm": accept(json!({ "confirmed": true })) },
            "requestState": state,
        }),
    )
    .await
    .unwrap();
    assert_eq!(done["resultType"], "complete");
    assert_eq!(
        done["content"][0]["text"],
        "deployed to prod after 1 round(s)"
    );

    let refused = call(
        &client,
        json!({
            "name": "deploy",
            "arguments": args,
            "inputResponses": { "confirm": accept(json!({ "confirmed": false })) },
            "requestState": state,
        }),
    )
    .await
    .unwrap();
    assert_eq!(refused["isError"], true);
    assert_eq!(refused["content"][0]["text"], "not confirmed");
}

#[tokio::test]
async fn answers_without_state_and_extra_keys_are_accepted() {
    let client = client(builder().build().unwrap());
    let done = call(
        &client,
        json!({
            "name": "deploy",
            "arguments": { "env": "dev" },
            "inputResponses": {
                "confirm": accept(json!({ "confirmed": true })),
                "unrelated": { "action": "accept", "content": { "x": 1 } },
            },
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        done["content"][0]["text"],
        "deployed to dev after 0 round(s)"
    );
}

#[tokio::test]
async fn questions_awaited_together_share_a_round() {
    let client = client(builder().build().unwrap());
    let first = call(&client, json!({ "name": "profile" })).await.unwrap();
    let mut keys: Vec<_> = first["inputRequests"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(keys, ["color", "name"]);
    let done = call(
        &client,
        json!({
            "name": "profile",
            "inputResponses": {
                "name": accept(json!({ "name": "Ada" })),
                "color": accept(json!({ "color": "green" })),
            },
            "requestState": first["requestState"],
        }),
    )
    .await
    .unwrap();
    assert_eq!(done["content"][0]["text"], "Ada likes green");
}

#[tokio::test]
async fn sequential_questions_take_several_rounds() {
    let client = client(builder().build().unwrap());
    let r1 = call(&client, json!({ "name": "wizard" })).await.unwrap();
    assert!(r1["inputRequests"]["step1"].is_object());

    let r2 = call(
        &client,
        json!({
            "name": "wizard",
            "inputResponses": { "step1": accept(json!({ "name": "Ada" })) },
            "requestState": r1["requestState"],
        }),
    )
    .await
    .unwrap();
    assert_eq!(r2["resultType"], "input_required");
    assert!(r2["inputRequests"]["step2"].is_object());
    assert!(r2["inputRequests"].get("step1").is_none());
    assert_ne!(r1["requestState"], r2["requestState"]);

    // Round 3 only sends step2: step1 comes back from the signed state.
    let r3 = call(
        &client,
        json!({
            "name": "wizard",
            "inputResponses": { "step2": accept(json!({ "color": "blue" })) },
            "requestState": r2["requestState"],
        }),
    )
    .await
    .unwrap();
    assert_eq!(r3["content"][0]["text"], "Ada likes blue");
}

#[tokio::test]
async fn missing_answers_are_requested_again() {
    let client = client(builder().build().unwrap());
    let retry = call(
        &client,
        json!({
            "name": "deploy",
            "arguments": { "env": "x" },
            "inputResponses": { "wrong_key": accept(json!({})) },
        }),
    )
    .await
    .unwrap();
    assert_eq!(retry["resultType"], "input_required");
    assert!(retry["inputRequests"]["confirm"].is_object());
}

#[tokio::test]
async fn declined_and_cancelled_answers() {
    let client = client(builder().build().unwrap());
    for (action, expected) in [("decline", "declined"), ("cancel", "cancelled")] {
        let r = call(
            &client,
            json!({ "name": "survey", "inputResponses": { "q": { "action": action } } }),
        )
        .await
        .unwrap();
        assert_eq!(r["content"][0]["text"], expected);
    }
    // With `elicit`, declining is a tool error the model can see.
    let r = call(
        &client,
        json!({
            "name": "deploy",
            "arguments": { "env": "x" },
            "inputResponses": { "confirm": { "action": "decline" } },
        }),
    )
    .await
    .unwrap();
    assert_eq!(r["isError"], true);
}

#[tokio::test]
async fn invalid_or_foreign_state_is_rejected() {
    let client = client(builder().build().unwrap());
    let first = call(
        &client,
        json!({ "name": "deploy", "arguments": { "env": "a" } }),
    )
    .await
    .unwrap();
    let state = first["requestState"].as_str().unwrap();
    let answer = json!({ "confirm": accept(json!({ "confirmed": true })) });

    // Tampered.
    let r = call(
        &client,
        json!({ "name": "deploy", "arguments": { "env": "a" }, "inputResponses": answer, "requestState": format!("{state}-TAMPERED") }),
    )
    .await;
    assert_eq!(code(r), -32602);
    // Replayed with other arguments.
    let r = call(
        &client,
        json!({ "name": "deploy", "arguments": { "env": "b" }, "inputResponses": answer, "requestState": state }),
    )
    .await;
    assert_eq!(code(r), -32602);
    // Replayed on another tool.
    let r = call(
        &client,
        json!({ "name": "survey", "inputResponses": answer, "requestState": state }),
    )
    .await;
    assert_eq!(code(r), -32602);
    // From another server (another random key).
    let other = client_for_other_server();
    let r = call(
        &other,
        json!({ "name": "deploy", "arguments": { "env": "a" }, "inputResponses": answer, "requestState": state }),
    )
    .await;
    assert_eq!(code(r), -32602);
}

fn client_for_other_server() -> TestClient {
    client(builder().build().unwrap())
}

#[tokio::test]
async fn instances_sharing_a_key_accept_each_others_state() {
    let key = [42u8; 32];
    let a = client(builder().request_state_key(key).build().unwrap());
    let b = client(builder().request_state_key(key).build().unwrap());
    let first = call(&a, json!({ "name": "wizard" })).await.unwrap();
    let second = call(
        &b,
        json!({
            "name": "wizard",
            "inputResponses": { "step1": accept(json!({ "name": "Ada" })) },
            "requestState": first["requestState"],
        }),
    )
    .await
    .unwrap();
    assert!(second["inputRequests"]["step2"].is_object());
}

#[tokio::test]
async fn malformed_answers_are_invalid_params() {
    let client = client(builder().build().unwrap());
    for responses in [
        json!(12345),
        json!({ "confirm": 12345 }),
        json!({ "confirm": { "action": "accept", "content": { "confirmed": "yes" } } }),
        json!({ "confirm": { "action": "maybe" } }),
    ] {
        let r = call(
            &client,
            json!({ "name": "deploy", "arguments": { "env": "x" }, "inputResponses": responses }),
        )
        .await;
        assert_eq!(code(r), -32602, "{responses}");
    }
    // `null` means no answers yet.
    let r = call(
        &client,
        json!({ "name": "deploy", "arguments": { "env": "x" }, "inputResponses": null }),
    )
    .await
    .unwrap();
    assert_eq!(r["resultType"], "input_required");
}

#[tokio::test]
async fn elicitation_needs_the_client_capability() {
    let client = TestClient::new(builder().build().unwrap());
    let r = client
        .request::<Value>(
            "tools/call",
            json!({ "name": "deploy", "arguments": { "env": "x" } }),
        )
        .await;
    match r.unwrap_err() {
        ClientError::Rpc { code, data, .. } => {
            assert_eq!(code, -32021);
            assert_eq!(
                data,
                Some(json!({ "requiredCapabilities": { "elicitation": { "form": {} } } }))
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn handlers_replay_from_the_start() {
    let client = client(builder().build().unwrap());
    let r1 = call(&client, json!({ "name": "counted" })).await.unwrap();
    let r2 = call(
        &client,
        json!({
            "name": "counted",
            "inputResponses": { "a": accept(json!({ "name": "x" })) },
            "requestState": r1["requestState"],
        }),
    )
    .await
    .unwrap();
    let r3 = call(
        &client,
        json!({
            "name": "counted",
            "inputResponses": { "b": accept(json!({ "color": "y" })) },
            "requestState": r2["requestState"],
        }),
    )
    .await
    .unwrap();
    assert_eq!(r3["content"][0]["text"], "x y");
    // One run per round.
    assert_eq!(COUNTED_RUNS.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn prompts_and_resources_support_mrtr() {
    let client = client(builder().build().unwrap());
    let first: Value = client
        .request(
            "prompts/get",
            json!({ "name": "contextual", "arguments": { "topic": "Rust" } }),
        )
        .await
        .unwrap();
    assert_eq!(first["resultType"], "input_required");
    let done: Value = client
        .request(
            "prompts/get",
            json!({
                "name": "contextual",
                "arguments": { "topic": "Rust" },
                "inputResponses": { "ctx": accept(json!({ "context": "beginners" })) },
                "requestState": first["requestState"],
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        done["messages"][0]["content"]["text"],
        "Write about Rust for beginners"
    );

    let first: Value = client
        .request("resources/read", json!({ "uri": "secret://note" }))
        .await
        .unwrap();
    assert_eq!(first["resultType"], "input_required");
    assert!(
        first.get("ttlMs").is_none(),
        "input-required results are not cacheable"
    );
    let done: Value = client
        .request(
            "resources/read",
            json!({
                "uri": "secret://note",
                "inputResponses": { "reveal": accept(json!({ "confirmed": true })) },
                "requestState": first["requestState"],
            }),
        )
        .await
        .unwrap();
    assert_eq!(done["contents"][0]["text"], "the note");
}

#[test]
fn weak_keys_are_rejected() {
    let r = builder().request_state_key(vec![1; 16]).build();
    assert!(matches!(
        r,
        Err(flymcprust::BuildError::WeakRequestStateKey)
    ));
}
