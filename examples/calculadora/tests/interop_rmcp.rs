//! Interoperability: the official Rust SDK (`rmcp`) client talks to the
//! calculator binary over stdio using the stateless 2026-07-28 lifecycle.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientLifecycleMode, ClientServiceExt as _};

#[tokio::test]
async fn rmcp_client_over_stdio() {
    let command = tokio::process::Command::new(env!("CARGO_BIN_EXE_calculadora"));
    let transport = TokioChildProcess::new(command).expect("spawn the calculator");
    let client = ()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("discover lifecycle");

    let tools = client.list_all_tools().await.expect("tools/list");
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        ["add", "divide", "multiply", "statistics", "subtract"]
    );

    let arguments = serde_json::json!({ "a": 20, "b": 22 });
    let result = client
        .call_tool(
            CallToolRequestParams::new("add")
                .with_arguments(arguments.as_object().cloned().unwrap()),
        )
        .await
        .expect("tools/call");
    let rendered = serde_json::to_value(&result).unwrap();
    assert_eq!(rendered["content"][0]["text"], "42", "{rendered}");

    client.cancel().await.expect("shut down");
}
