//! `scopes = [..]` on `#[tool]`, `#[resource]` and `#[prompt]`. Enforcement
//! over HTTP is tested in `sdkmcprust-core/tests/oauth.rs`.
#![forbid(unsafe_code)]
// Integration tests may panic on failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sdkmcprust::prelude::*;
use sdkmcprust::{Prompt as _, Resource as _, Tool as _};

/// Deletes a file.
///
/// # Arguments
///
/// * `path` - The file to delete.
#[tool(destructive, scopes = ["files:write", "files:read"])]
fn delete_file(path: String) -> String {
    path
}

/// Unrestricted.
#[tool]
fn ping() -> &'static str {
    "pong"
}

/// Private notes.
#[resource("notes://private", scopes = ["notes"])]
fn notes() -> String {
    String::new()
}

/// An audit summary.
#[prompt(scopes = ["audit"])]
fn audit() -> String {
    "Summarize the audit log.".to_owned()
}

#[test]
fn macros_declare_scopes() {
    assert_eq!(
        delete_file::definition().required_scopes(),
        ["files:write", "files:read"]
    );
    assert!(ping::definition().required_scopes().is_empty());
    assert_eq!(notes::definition().required_scopes(), ["notes"]);
    assert_eq!(audit::definition().required_scopes(), ["audit"]);
    // The functions stay callable.
    assert_eq!(delete_file("a".to_owned()), "a");
}

#[test]
fn scoped_servers_build() {
    Server::builder("scopes", "1.0.0")
        .tool::<delete_file>()
        .tool::<ping>()
        .resource::<notes>()
        .prompt::<audit>()
        .build()
        .unwrap();
}
