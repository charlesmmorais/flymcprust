//! `ServerBuilder::auto_tools` collects every `#[tool]` in the binary.
#![cfg(feature = "auto-register")]
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use flymcprust::prelude::*;

/// Registered automatically.
#[tool]
fn auto_one() -> &'static str {
    "one"
}

/// Also registered automatically.
#[tool]
async fn auto_two(x: i64) -> i64 {
    x * 2
}

#[test]
fn collects_all_tools() {
    let server = Server::builder("auto", "1").auto_tools().build().unwrap();
    assert!(server.mirrored_params("auto_one").is_some());
    assert!(server.mirrored_params("auto_two").is_some());
}

#[test]
fn explicit_registration_of_the_same_tool_is_a_duplicate() {
    let result = Server::builder("auto", "1")
        .auto_tools()
        .tool::<auto_one>()
        .build();
    assert!(matches!(
        result,
        Err(flymcprust::BuildError::DuplicateTool(_))
    ));
}
