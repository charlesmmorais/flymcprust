//! Handlers whose parameter is named like the function (`fn user(user:
//! User)`) compile and work: the macros call the function through an
//! internal binding, so the argument cannot shadow it.
#![cfg(feature = "test-client")]
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use flymcprust::prelude::*;
use flymcprust::testing::TestClient;
use serde_json::json;

/// A user.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct User {
    name: String,
}

/// Greets a user.
///
/// # Arguments
///
/// * `user` - The user to greet.
#[tool]
fn user(user: User) -> String {
    format!("hello {}", user.name)
}

/// Greets someone, asynchronously.
///
/// # Arguments
///
/// * `greet` - Who to greet.
#[tool]
async fn greet(greet: String) -> String {
    format!("hi {greet}")
}

/// A document by id.
#[resource(template = "docs://{doc}")]
fn doc(doc: String) -> String {
    format!("document {doc}")
}

/// A prompt about a topic.
///
/// # Arguments
///
/// * `topic` - What to write about.
#[prompt]
fn topic(topic: String) -> String {
    format!("Write about {topic}.")
}

#[tokio::test]
async fn arguments_named_like_their_handler() {
    let server = Server::builder("shadowing", "1.0.0")
        .tool::<user>()
        .tool::<greet>()
        .resource::<doc>()
        .prompt::<topic>()
        .build()
        .unwrap();
    let client = TestClient::new(server);

    let result = client
        .call_tool("user", json!({ "user": { "name": "Ana" } }))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&result.content)
            .unwrap()
            .contains("hello Ana")
    );
    let result = client
        .call_tool("greet", json!({ "greet": "Bia" }))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&result.content)
            .unwrap()
            .contains("hi Bia")
    );
    let read = client.read_resource("docs://7").await.unwrap();
    assert!(
        serde_json::to_string(&read.contents)
            .unwrap()
            .contains("document 7")
    );
    let prompt = client
        .get_prompt("topic", json!({ "topic": "Rust" }))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&prompt.messages)
            .unwrap()
            .contains("about Rust")
    );
}
