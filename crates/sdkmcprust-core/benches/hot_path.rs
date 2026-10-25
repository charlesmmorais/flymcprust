//! Hot-path benchmarks: message parsing, dispatch and response
//! serialization. Run with `cargo bench -p sdkmcprust-core`.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use schemars::JsonSchema;
use sdkmcprust_core::jsonrpc::{self, Response};
use sdkmcprust_core::{
    Ctx, IntoToolResult, Limits, Server, ToolDef, ToolError, ToolFuture, TransportContext,
    input_schema_for, parse_arguments, spec,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio_util::sync::CancellationToken;

const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"bench","version":"1.0.0"}}"#;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

fn add(arguments: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    let args: AddArgs = parse_arguments(arguments)?;
    Ok(Box::pin(
        async move { (args.a + args.b).into_tool_result() },
    ))
}

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

fn echo(arguments: Option<&RawValue>, _: Ctx) -> Result<ToolFuture, ToolError> {
    let args: EchoArgs = parse_arguments(arguments)?;
    Ok(Box::pin(async move { args.text.into_tool_result() }))
}

fn server() -> Server {
    let mut builder = Server::builder("bench", "1.0.0")
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<AddArgs>(), "add".to_owned()),
            add,
        ))
        .tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<EchoArgs>(), "echo".to_owned()),
            echo,
        ));
    // A realistic catalog size for tools/list.
    for i in 0..30 {
        builder = builder.tool_def(ToolDef::new(
            spec::Tool::new(input_schema_for::<AddArgs>(), format!("tool_{i:02}")),
            add,
        ));
    }
    builder.build().unwrap()
}

fn messages() -> Vec<(&'static str, String)> {
    vec![
        (
            "tools_list",
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META}}}}}"#),
        ),
        (
            "tools_call_add",
            format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"add","arguments":{{"a":1.5,"b":2.25}},{META}}}}}"#
            ),
        ),
        (
            "tools_call_64KiB",
            format!(
                r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"echo","arguments":{{"text":"{}"}},{META}}}}}"#,
                "x".repeat(64 * 1024)
            ),
        ),
    ]
}

fn parse(c: &mut Criterion) {
    let limits = Limits::default();
    let mut group = c.benchmark_group("parse");
    for (name, message) in messages() {
        group.throughput(Throughput::Bytes(message.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &message, |b, m| {
            b.iter(|| jsonrpc::parse(black_box(m.as_bytes()), &limits).unwrap());
        });
    }
    group.finish();
}

fn dispatch(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = server();
    let mut group = c.benchmark_group("dispatch");
    for (name, message) in messages() {
        group.throughput(Throughput::Bytes(message.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &message, |b, m| {
            b.to_async(&runtime).iter(|| async {
                let response = server
                    .handle_bytes(
                        black_box(m.as_bytes()),
                        TransportContext::default(),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                black_box(response.to_vec())
            });
        });
    }
    group.finish();
}

fn serialize(c: &mut Criterion) {
    let result = RawValue::from_string(
        r#"{"resultType":"complete","content":[{"type":"text","text":"3.75"}],"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"bench","version":"1.0.0"}}}"#
            .to_owned(),
    )
    .unwrap();
    let response = Response::Result {
        id: spec::RequestId::Integer(42),
        result,
    };
    c.bench_function("serialize/call_result", |b| {
        b.iter(|| black_box(&response).to_vec());
    });
}

criterion_group!(benches, parse, dispatch, serialize);
criterion_main!(benches);
